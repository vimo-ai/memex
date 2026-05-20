//! MCP (Model Context Protocol) HTTP 传输协议实现
//!
//! 实现 JSON-RPC 2.0 协议，提供 4 个 MCP 工具：
//! - search_history: 搜索历史对话
//! - get_session: 获取会话详情
//! - get_recent_sessions: 获取最近会话
//! - list_projects: 列出项目
//!
//! ## 设计原则
//! - 精简输出：只返回 AI 需要的字段，减少 token 消耗
//! - 渐进披露：列表返回摘要，详情返回完整内容
//! - 位置导航：使用 `at` 字段定位消息，支持 `around` 模式获取上下文

#![allow(dead_code)] // JSON-RPC 字段由 serde 使用

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use chrono::{Duration, Local, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::api::AppState;

/// 毫秒时间戳转本地时间字符串
fn ms_to_local_time(ts: Option<i64>) -> Option<String> {
    ts.map(|ms| {
        Local
            .timestamp_millis_opt(ms)
            .single()
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| format!("{}", ms))
    })
}

/// 毫秒时间戳转相对时间（如 "2h ago", "3d ago"）
fn ms_to_relative_time(ts: Option<i64>) -> Option<String> {
    ts.map(|ms| {
        let now = Local::now().timestamp_millis();
        let diff_ms = now - ms;
        let diff_secs = diff_ms / 1000;

        if diff_secs < 60 {
            "just now".to_string()
        } else if diff_secs < 3600 {
            format!("{}m ago", diff_secs / 60)
        } else if diff_secs < 86400 {
            format!("{}h ago", diff_secs / 3600)
        } else if diff_secs < 86400 * 30 {
            format!("{}d ago", diff_secs / 86400)
        } else {
            // 超过30天显示具体日期
            Local
                .timestamp_millis_opt(ms)
                .single()
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| format!("{}", ms))
        }
    })
}

/// 解析时间快捷方式（如 "3d", "1w", "1m"）返回起始时间戳
fn parse_time_shortcut(shortcut: &str) -> Option<i64> {
    let shortcut = shortcut.trim().to_lowercase();
    let (num_str, unit) = shortcut.split_at(shortcut.len().saturating_sub(1));
    let num: i64 = num_str.parse().ok()?;

    let duration = match unit {
        "h" => Duration::hours(num),
        "d" => Duration::days(num),
        "w" => Duration::weeks(num),
        "m" => Duration::days(num * 30), // 近似月
        _ => return None,
    };

    let start = Local::now() - duration;
    Some(start.timestamp_millis())
}

/// 安全截断字符串（按字符数，非字节数）
fn truncate_str(s: &str, max_chars: usize) -> String {
    let truncated: String = s.chars().take(max_chars).collect();
    if truncated.len() < s.len() {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

/// 判断是否为 system 噪声消息
///
/// 噪声消息定义：type 为 system 且 content_text 为空
/// 这类消息包括 stop_hook_summary、turn_duration 等运行时元数据，对 AI 消费无价值
///
/// 注意：`[Summary]` 类型的 system 消息 content_text 非空，不会被过滤
fn is_system_noise(m: &ai_cli_session_db::Message) -> bool {
    m.r#type == ai_cli_session_db::MessageType::System && m.content_text.is_empty()
}

/// MCP GET 请求参数
#[derive(Debug, Deserialize)]
pub struct MCPGetQuery {
    method: Option<String>,
    id: Option<String>,
}

/// JSON-RPC 请求
#[derive(Debug, Deserialize)]
pub struct MCPRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

impl MCPRequest {
    /// 判断是否为 notification（无 id 的请求，不需要响应）
    fn is_notification(&self) -> bool {
        self.id.is_none() || self.method.starts_with("notifications/")
    }
}

/// JSON-RPC 响应
#[derive(Debug, Serialize)]
pub struct MCPResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<MCPError>,
}

#[derive(Debug, Serialize)]
pub struct MCPError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl MCPResponse {
    fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<Value>, code: i32, message: &str, data: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(MCPError {
                code,
                message: message.to_string(),
                data,
            }),
        }
    }
}

/// MCP tool definitions
fn get_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "search_history",
            "description": "Search Claude Code conversation history with progressive disclosure. Level controls detail: 'sessions' (L3 summaries, default) -> 'talks' (L2 per-prompt summaries) -> 'raw' (L0 original messages). Use get_session for full context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search keywords" },
                    "level": { "type": "string", "enum": ["sessions", "talks", "raw"], "description": "Detail level: sessions (L3 summary, recommended), talks (L2 per-prompt), raw (L0 original). Default: sessions" },
                    "cwd": { "oneOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }], "description": "Filter to specific project(s). Supports: exact path, prefix, or glob patterns (e.g. '*/ETerm*')" },
                    "exclude_cwd": { "oneOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }], "description": "Exclude specific project(s). Supports: exact path, prefix, or glob patterns (e.g. '*memex*')" },
                    "time": { "type": "string", "description": "Time shortcut: 3h/1d/3d/1w/1m (forces raw level for accuracy)" },
                    "from": { "type": "string", "description": "Start date YYYY-MM-DD (forces raw level for accuracy)" },
                    "to": { "type": "string", "description": "End date YYYY-MM-DD (forces raw level for accuracy)" },
                    "limit": { "type": "number", "description": "Max results, default 5" },
                    "sessionId": { "oneOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }], "description": "Filter to specific session(s). Supports prefix matching (e.g. 'abc12' matches 'abc12345-...')" },
                    "source": { "type": "string", "enum": ["local", "remote", "all"], "description": "Search source: local (default), remote (sync server only), all (local + remote merge)" }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "get_session",
            "description": "Get session messages. Three modes: (1) at: get single message with full content, (2) around+context: get context around position (truncated), (3) limit+order: pagination (truncated when limit>5). Use source='remote' to read sessions from sync server (admin key required).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string", "description": "Session ID (full or prefix)" },
                    "at": { "type": "number", "description": "Get single message by position (from search_history 'at'). Returns full content, no truncation" },
                    "around": { "type": "number", "description": "Get context around this position (from search_history 'at'). Content may be truncated" },
                    "context": { "type": "number", "description": "Messages before/after 'around', default 5, max 20" },
                    "limit": { "type": "number", "description": "Messages to return, default 10 (ignored if at/around is set)" },
                    "offset": { "type": "number", "description": "Skip N messages before applying limit. For asc: skip from start; for desc: skip from end. Default 0 (ignored if at/around is set)" },
                    "order": { "type": "string", "enum": ["asc", "desc"], "description": "asc (from start) / desc (from end), default asc (ignored if at/around is set)" },
                    "source": { "type": "string", "enum": ["local", "remote"], "description": "Data source: local (default) or remote (sync server, admin key required)" }
                },
                "required": ["sessionId"]
            }
        }),
        json!({
            "name": "get_recent_sessions",
            "description": "Get recent sessions. Returns: ref (session ID), project, messages (count), time. Optionally include L3 summary.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cwd": { "oneOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }], "description": "Filter to specific project(s). Supports: exact path, prefix, or glob patterns (e.g. '*/ETerm*')" },
                    "exclude_cwd": { "oneOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }], "description": "Exclude specific project(s). Supports: exact path, prefix, or glob patterns (e.g. '*memex*')" },
                    "limit": { "type": "number", "description": "Max results, default 5" },
                    "include_summary": { "type": "boolean", "description": "Include L3 session summary if available. Default: false" }
                }
            }
        }),
        json!({
            "name": "list_projects",
            "description": "List all projects. Returns: name, path, sessions (count), time",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "extract_knowledge",
            "description": "Extract L4 knowledge nodes from sessions. Requires knowledge.enabled=true in config and a strong chat model (e.g. qwen3:14b).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project path or glob pattern (e.g. '*ETerm*')" },
                    "limit": { "type": "number", "description": "Max sessions to process (default 10)" },
                    "dry_run": { "type": "boolean", "description": "Preview only, don't write DB" },
                    "desc": { "type": "string", "description": "Project description for LLM prompts" }
                }
            }
        }),
    ]
}

/// 处理 MCP POST 请求
pub async fn handle_mcp(
    State(state): State<Arc<AppState>>,
    Json(request): Json<MCPRequest>,
) -> impl IntoResponse {
    // Notifications 不需要响应
    if request.is_notification() {
        return Json(None);
    }
    let response = process_mcp_request(&state, request).await;
    Json(Some(response))
}

/// 处理 MCP GET 请求 (通过 query 参数)
pub async fn handle_mcp_get(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MCPGetQuery>,
) -> impl IntoResponse {
    let request = MCPRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(query.id.unwrap_or_else(|| "1".to_string()))),
        method: query.method.unwrap_or_else(|| "tools/list".to_string()),
        params: None,
    };
    let response = process_mcp_request(&state, request).await;
    Json(response)
}

/// 获取 MCP 服务信息
pub async fn get_mcp_info() -> impl IntoResponse {
    Json(json!({
        "server": {
            "name": "memex-mcp-server",
            "version": "1.0.0",
            "protocolVersion": "2024-11-05"
        },
        "capabilities": {
            "tools": {}
        },
        "endpoints": {
            "mcp": "/api/mcp",
            "info": "/api/mcp/info"
        },
        "tools": get_tools(),
        "usage": {
            "post": "Send MCP JSON-RPC requests to /api/mcp",
            "get": "Use query parameters: ?method=tools/list&id=1"
        }
    }))
}

async fn process_mcp_request(state: &AppState, request: MCPRequest) -> MCPResponse {
    let id = request.id.clone();

    match request.method.as_str() {
        "initialize" => MCPResponse::success(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "memex-mcp-server",
                    "version": "1.0.0"
                }
            }),
        ),

        "tools/list" => MCPResponse::success(id, json!({ "tools": get_tools() })),

        "tools/call" => {
            let params = match request.params {
                Some(p) => p,
                None => return MCPResponse::error(id, -32602, "Invalid params", None),
            };

            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));

            match call_tool(state, name, args).await {
                Ok(result) => MCPResponse::success(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&result).unwrap_or_default()
                        }]
                    }),
                ),
                Err(e) => {
                    MCPResponse::error(id, -32603, &format!("Tool execution failed: {}", e), None)
                }
            }
        }

        _ => MCPResponse::error(
            id,
            -32601,
            "Method not found",
            Some(json!(format!("Unknown method: {}", request.method))),
        ),
    }
}

/// 调用 MCP 工具
async fn call_tool(state: &AppState, name: &str, args: Value) -> Result<Value, String> {
    match name {
        "search_history" => search_history(state, args).await,
        "get_session" => get_session(state, args).await,
        "get_recent_sessions" => get_recent_sessions(state, args).await,
        "list_projects" => list_projects(state, args).await,
        "extract_knowledge" => extract_knowledge(state, args).await,
        _ => Err(format!("Unknown tool: {}", name)),
    }
}

/// 将日期字符串 (YYYY-MM-DD) 转换为时间戳（毫秒）
fn date_to_timestamp(date: &str, is_start: bool) -> Option<i64> {
    use chrono::{Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};

    let parsed = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let time = if is_start {
        NaiveTime::from_hms_opt(0, 0, 0)?
    } else {
        NaiveTime::from_hms_milli_opt(23, 59, 59, 999)?
    };
    let datetime = NaiveDateTime::new(parsed, time);

    let local_dt = Local.from_local_datetime(&datetime).single()?;
    Some(local_dt.timestamp_millis())
}

/// 搜索历史对话（渐进式披露）
///
/// level 参数控制披露层级：
/// - sessions (L3): 会话摘要，最省 token，默认
/// - talks (L2): 对话摘要（每个 prompt 一个）
/// - raw (L0): 原始消息
async fn search_history(state: &AppState, args: Value) -> Result<Value, String> {
    let query = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
    let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(5) as usize;
    let level = args
        .get("level")
        .and_then(|l| l.as_str())
        .unwrap_or("sessions");

    // 解析 cwd 参数（支持 string 或 array）
    let include_cwds = parse_cwd_param(args.get("cwd"));
    let exclude_cwds = parse_cwd_param(args.get("exclude_cwd"));

    // 解析 sessionId 参数（支持 string 或 array，过滤空字符串）
    let session_ids: Vec<String> = parse_cwd_param(args.get("sessionId"))
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect();

    // 时间参数：time 快捷方式 与 from/to 互斥
    let time_shortcut = args.get("time").and_then(|t| t.as_str());
    let from_date = args.get("from").and_then(|d| d.as_str());
    let to_date = args.get("to").and_then(|d| d.as_str());

    // 互斥检查
    if time_shortcut.is_some() && (from_date.is_some() || to_date.is_some()) {
        return Err("'time' and 'from'/'to' are mutually exclusive".to_string());
    }

    let source = args
        .get("source")
        .and_then(|s| s.as_str())
        .unwrap_or("local");

    if query.is_empty() {
        return Ok(json!({ "results": [], "total": 0, "level": level }));
    }

    // remote-only: 直接查 sync server
    if source == "remote" {
        return match &state.remote_search {
            Some(remote) => {
                let remote_results = remote.search(query, limit, level).await;
                let formatted = crate::search::remote::format_remote_results(
                    remote_results,
                    &std::collections::HashSet::new(),
                );
                Ok(json!({
                    "results": formatted,
                    "total": formatted.len(),
                    "level": level,
                    "source": "remote"
                }))
            }
            None => Err(
                "Remote search not configured (missing sync_server/sync_api_key in config)"
                    .to_string(),
            ),
        };
    }

    // 构建项目过滤器
    let filter = ProjectFilter {
        include: find_project_ids_by_cwds(state, &include_cwds).await,
        exclude: find_project_ids_by_cwds(state, &exclude_cwds).await,
    };

    // 解析时间范围（无效日期报错而非静默忽略）
    let (_start_ts, _end_ts) = if let Some(shortcut) = time_shortcut {
        let start = parse_time_shortcut(shortcut)
            .ok_or_else(|| format!("Invalid time shortcut: {}. Use 1d/3d/1w/1m", shortcut))?;
        (Some(start), None)
    } else {
        let start = if let Some(d) = from_date {
            Some(
                date_to_timestamp(d, true)
                    .ok_or_else(|| format!("Invalid from date: {}. Use YYYY-MM-DD", d))?,
            )
        } else {
            None
        };
        let end = if let Some(d) = to_date {
            Some(
                date_to_timestamp(d, false)
                    .ok_or_else(|| format!("Invalid to date: {}. Use YYYY-MM-DD", d))?,
            )
        } else {
            None
        };
        (start, end)
    };

    // 时间过滤只在消息层面精确，有时间参数时强制使用 raw 模式
    let has_time_filter = _start_ts.is_some() || _end_ts.is_some();
    let effective_level = if has_time_filter { "raw" } else { level };

    // 根据 level 选择搜索方式
    let mut result = match effective_level {
        "sessions" => search_session_summaries(state, query, limit, &filter, &session_ids).await,
        "talks" => search_talk_summaries(state, query, limit, &filter, &session_ids).await,
        "raw" => {
            search_raw_messages(
                state,
                query,
                limit,
                &filter,
                _start_ts,
                _end_ts,
                &session_ids,
            )
            .await
        }
        _ => Err(format!("Invalid level: {}. Use sessions/talks/raw", level)),
    }?;

    // source=all 或默认 fallback：本地结果不足时查 sync server 补充
    if source == "all" || source == "local" {
        if let Some(ref remote) = state.remote_search {
            let local_results = result["results"].as_array().map_or(0, |a| a.len());
            if local_results < limit {
                let remaining = if source == "all" {
                    limit
                } else {
                    limit - local_results
                };
                let remote_results = remote.search(query, remaining, effective_level).await;
                if !remote_results.is_empty() {
                    let local_sessions: std::collections::HashSet<String> = result["results"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| r["session"].as_str().or(r["sessionId"].as_str()))
                                .map(String::from)
                                .collect()
                        })
                        .unwrap_or_default();

                    let mut merged = crate::search::remote::format_remote_results(
                        remote_results,
                        &local_sessions,
                    );

                    if !merged.is_empty() {
                        if let Some(arr) = result.get_mut("results").and_then(|v| v.as_array_mut())
                        {
                            arr.append(&mut merged);
                            let new_total = arr.len();
                            result["total"] = json!(new_total);
                        }
                    }
                }
            }
        }
    }

    Ok(result)
}

/// L3: 搜索会话摘要
async fn search_session_summaries(
    state: &AppState,
    query: &str,
    limit: usize,
    filter: &ProjectFilter,
    session_ids: &[String],
) -> Result<Value, String> {
    // 如果 compact_db 不可用，fallback 到 talks
    let compact_db = match &state.compact_db {
        Some(db) => db,
        None => return search_talk_summaries(state, query, limit, filter, session_ids).await,
    };

    // 多取一些结果用于过滤
    let search_limit = if filter.is_empty() { limit } else { limit * 3 };
    let mut results = compact_db
        .search_session_summaries_with_sessions(query, search_limit, session_ids)
        .await
        .map_err(|e| e.to_string())?;

    // L4 knowledge search: find sessions via knowledge nodes
    let l4_session_ids = search_knowledge_sessions(state, query, limit * 2).await;
    let l3_session_set: std::collections::HashSet<String> =
        results.iter().map(|s| s.session_id.clone()).collect();
    let mut l4_extras = Vec::new();
    for sid in &l4_session_ids {
        if !l3_session_set.contains(sid) {
            if let Ok(Some(summary)) = compact_db.get_session_summary(sid).await {
                l4_extras.push(summary);
            }
        }
    }
    results.extend(l4_extras);

    // 如果没有结果，fallback 到 talks
    if results.is_empty() {
        return search_talk_summaries(state, query, limit, filter, session_ids).await;
    }

    // 获取 session 到 project_id 的映射（用于过滤）
    let all_sids: Vec<String> = results.iter().map(|s| s.session_id.clone()).collect();
    let session_project_map = if !filter.is_empty() {
        get_session_project_map(state, &all_sids).await
    } else {
        std::collections::HashMap::new()
    };

    let filtered: Vec<_> = results
        .iter()
        .filter(|s| {
            if filter.is_empty() {
                return true;
            }
            if let Some(&project_id) = session_project_map.get(&s.session_id) {
                filter.matches(project_id)
            } else {
                true
            }
        })
        .take(limit)
        .collect();

    // L4 knowledge enrichment
    let session_ids_for_l4: Vec<String> = filtered.iter().map(|s| s.session_id.clone()).collect();
    let knowledge_map = enrich_with_knowledge(state, &session_ids_for_l4).await;

    let formatted: Vec<Value> = filtered
        .iter()
        .map(|s| {
            let mut entry = json!({
                "session": &s.session_id,
                "summary": &s.summary,
                "keyPoints": &s.key_points,
                "files": &s.files_involved,
                "technologies": &s.technologies,
                "time": &s.updated_at
            });
            if let Some(nodes) = knowledge_map.get(&s.session_id) {
                entry["knowledge"] = json!(nodes);
            }
            entry
        })
        .collect();

    Ok(json!({
        "results": formatted,
        "total": formatted.len(),
        "level": "sessions",
        "hint": "Use level='talks' for per-prompt details, or get_session for full context"
    }))
}

/// Batch-load L4 knowledge nodes for a set of sessions, grouped by session_id.
/// Search L4 knowledge nodes and return matching session IDs.
async fn search_knowledge_sessions(state: &AppState, query: &str, limit: usize) -> Vec<String> {
    let nodes = match state.knowledge_store.search_nodes(query, limit).await {
        Ok(n) => n,
        Err(_) => return Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    nodes
        .into_iter()
        .filter_map(|n| {
            if seen.insert(n.session_id.clone()) {
                Some(n.session_id)
            } else {
                None
            }
        })
        .collect()
}

async fn enrich_with_knowledge(
    state: &AppState,
    session_ids: &[String],
) -> std::collections::HashMap<String, Vec<Value>> {
    let nodes = match state
        .knowledge_store
        .get_nodes_by_sessions(session_ids)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            tracing::debug!("L4 knowledge lookup skipped: {e}");
            return std::collections::HashMap::new();
        }
    };
    let mut map: std::collections::HashMap<String, Vec<Value>> = std::collections::HashMap::new();
    for n in nodes {
        map.entry(n.session_id.clone()).or_default().push(json!({
            "topic": n.topic,
            "conclusion": n.conclusion,
            "confidence": n.confidence,
            "type": n.node_type,
            "cluster": n.cluster_id,
        }));
    }
    map
}

/// L2: 搜索对话摘要
async fn search_talk_summaries(
    state: &AppState,
    query: &str,
    limit: usize,
    filter: &ProjectFilter,
    session_ids: &[String],
) -> Result<Value, String> {
    // 如果 compact_db 不可用，fallback 到 raw
    let compact_db = match &state.compact_db {
        Some(db) => db,
        None => {
            return search_raw_messages(state, query, limit, filter, None, None, session_ids).await
        }
    };

    // 多取一些结果用于过滤
    let search_limit = if filter.is_empty() { limit } else { limit * 3 };
    let results = compact_db
        .search_talk_summaries_with_sessions(query, search_limit, session_ids)
        .await
        .map_err(|e| e.to_string())?;

    // 如果没有结果，fallback 到 raw
    if results.is_empty() {
        return search_raw_messages(state, query, limit, filter, None, None, session_ids).await;
    }

    // 获取 session 到 project_id 的映射（用于过滤）
    let session_project_map = if !filter.is_empty() {
        get_session_project_map(
            state,
            &results
                .iter()
                .map(|t| t.session_id.clone())
                .collect::<Vec<_>>(),
        )
        .await
    } else {
        std::collections::HashMap::new()
    };

    let formatted: Vec<Value> = results
        .iter()
        .filter(|t| {
            if filter.is_empty() {
                return true;
            }
            if let Some(&project_id) = session_project_map.get(&t.session_id) {
                filter.matches(project_id)
            } else {
                true // 如果找不到映射，不过滤
            }
        })
        .take(limit)
        .map(|t| {
            json!({
                "session": &t.session_id,
                "prompt": t.prompt_number,
                "request": &t.user_request,
                "summary": &t.summary,
                "completed": &t.completed,
                "files": &t.files_involved,
                "time": &t.created_at
            })
        })
        .collect();

    Ok(json!({
        "results": formatted,
        "total": formatted.len(),
        "level": "talks",
        "hint": "Use level='raw' for original messages, or get_session for full context"
    }))
}

/// L0: 搜索原始消息
async fn search_raw_messages(
    state: &AppState,
    query: &str,
    limit: usize,
    filter: &ProjectFilter,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
    session_ids: &[String],
) -> Result<Value, String> {
    // 对于单个 include 项目，使用 SQL 层过滤（向后兼容，更高效）
    // 对于复杂过滤（多 include 或 exclude），使用内存过滤
    let sql_project_id = filter.single_include();
    let need_memory_filter = !filter.is_empty() && sql_project_id.is_none();

    // 执行 FTS 搜索，多取一些用于计算 sessionMatches 和过滤
    let search_limit = if need_memory_filter {
        (limit * 5).max(50)
    } else {
        (limit * 3).max(30)
    };
    let results = state
        .db
        .search_fts_full_with_sessions(
            query,
            search_limit,
            sql_project_id,
            ai_cli_session_db::SearchOrderBy::Score,
            start_ts,
            end_ts,
            session_ids,
        )
        .await
        .map_err(|e| e.to_string())?;

    // 计算每个 session 的匹配数
    let mut session_match_counts: HashMap<String, usize> = HashMap::new();
    for r in &results {
        *session_match_counts
            .entry(r.session_id.clone())
            .or_insert(0) += 1;
    }

    // 去重：每个 session 只保留最高分的一条
    let mut seen_sessions: HashMap<String, bool> = HashMap::new();
    let formatted: Vec<Value> = results
        .iter()
        .filter(|r| {
            // 过滤 system 噪声消息（type=system 且 content_full 以 [system: 开头）
            // 这类消息是运行时元数据，对 AI 消费无价值
            if r.r#type == "system" && r.content_full.starts_with("[system:") {
                return false;
            }
            // 去重
            if seen_sessions.contains_key(&r.session_id) {
                return false;
            }
            // 内存过滤
            if need_memory_filter && !filter.matches(r.project_id) {
                return false;
            }
            seen_sessions.insert(r.session_id.clone(), true);
            true
        })
        .take(limit)
        .map(|r| {
            let session_matches = session_match_counts
                .get(&r.session_id)
                .copied()
                .unwrap_or(1);
            json!({
                "session": &r.session_id,
                "role": &r.r#type,
                "snippet": &r.snippet,
                "at": r.message_id, // 消息位置（用于 get_session around）
                "sessionMatches": session_matches,
                "time": ms_to_relative_time(r.timestamp)
            })
        })
        .collect();

    Ok(json!({
        "results": formatted,
        "total": formatted.len(),
        "level": "raw",
        "hint": "Use get_session with 'around' parameter for context"
    }))
}

/// 获取会话详情
async fn get_session(state: &AppState, args: Value) -> Result<Value, String> {
    let session_id_input = args
        .get("sessionId")
        .and_then(|s| s.as_str())
        .ok_or("sessionId is required")?;

    let source = args
        .get("source")
        .and_then(|s| s.as_str())
        .unwrap_or("local");

    if source == "remote" {
        let remote = state
            .remote_search
            .as_ref()
            .ok_or("Remote not configured (missing sync_server/sync_api_key in config)")?;
        let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(50) as usize;
        let offset = args.get("offset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
        let order = args.get("order").and_then(|o| o.as_str()).unwrap_or("asc");
        let result = remote
            .get_session_messages(session_id_input, limit, offset, order)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(result);
    }

    // at 模式：获取单条消息完整内容
    let at = args.get("at").and_then(|a| a.as_i64());

    // around 模式参数
    let around = args.get("around").and_then(|a| a.as_i64());
    let context = args
        .get("context")
        .and_then(|c| c.as_u64())
        .unwrap_or(5)
        .min(20) as usize; // 最大 20

    // 传统分页参数（at/around 模式下忽略）
    let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(10) as usize;
    let offset = args.get("offset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
    let order = args.get("order").and_then(|o| o.as_str()).unwrap_or("asc");

    // 解析 session ID（支持前缀匹配）
    let session_id = state
        .db
        .resolve_session_id(session_id_input)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Session not found: {}", session_id_input))?;

    // 获取会话信息（type, source, 关系）
    let session_info = state
        .db
        .get_session(&session_id)
        .await
        .map_err(|e| e.to_string())?;
    let parent_relation = state
        .db
        .get_parent_session(&session_id)
        .await
        .unwrap_or(None);
    let children = state
        .db
        .get_children_sessions(&session_id)
        .await
        .unwrap_or_default();

    // 获取所有消息（需要完整列表来计算位置）
    let all_messages = state
        .db
        .get_messages(&session_id)
        .await
        .map_err(|e| e.to_string())?;

    if all_messages.is_empty() {
        return Err(format!("Session not found: {}", session_id));
    }

    // 构建 session 元信息
    let mut session_meta = json!({});
    if let Some(ref info) = session_info {
        if let Some(ref t) = info.session_type {
            session_meta["type"] = json!(t);
        }
        if let Some(ref src) = info.source {
            session_meta["source"] = json!(src);
        }
    }
    if let Some(ref parent) = parent_relation {
        session_meta["parent"] = json!(parent.parent_session_id);
    }
    if !children.is_empty() {
        session_meta["children"] = json!(children.len());
    }

    // at 模式：返回单条消息完整内容（不过滤，用户明确要查看这条消息）
    if let Some(target_message_id) = at {
        let message = all_messages
            .iter()
            .find(|m| m.id == target_message_id)
            .ok_or_else(|| format!("Message not found: {}", target_message_id))?;

        // 过滤后的总数（用于显示）
        let filtered_total = all_messages.iter().filter(|m| !is_system_noise(m)).count();

        let mut result = json!({
            "total": filtered_total,
            "message": {
                "role": format!("{:?}", message.r#type).to_lowercase(),
                "text": message.content_full,  // 完整内容，不截断
                "at": message.id
            }
        });
        if !session_meta.as_object().unwrap().is_empty() {
            result["session"] = session_meta;
        }
        return Ok(result);
    }

    // around 模式：基于 message_id 定位，但过滤上下文中的 system 噪声
    if let Some(target_message_id) = around {
        let target_pos = all_messages
            .iter()
            .position(|m| m.id == target_message_id)
            .ok_or_else(|| format!("Message not found: {}", target_message_id))?;

        // 取原始切片，然后过滤（保留目标消息即使它是 system）
        let from = target_pos.saturating_sub(context);
        let to = (target_pos + context + 1).min(all_messages.len());

        let messages: Vec<Value> = all_messages[from..to]
            .iter()
            .filter(|m| m.id == target_message_id || !is_system_noise(m))
            .map(|m| {
                let content = if to - from > 5 {
                    truncate_str(&m.content_full, 500)
                } else {
                    m.content_full.clone()
                };
                json!({
                    "role": format!("{:?}", m.r#type).to_lowercase(),
                    "text": content,
                    "at": m.id
                })
            })
            .collect();

        // 过滤后的总数
        let filtered_total = all_messages.iter().filter(|m| !is_system_noise(m)).count();

        // range 使用实际返回的消息的 ID
        let first_msg_id = messages.first().and_then(|m| m["at"].as_i64()).unwrap_or(0);
        let last_msg_id = messages.last().and_then(|m| m["at"].as_i64()).unwrap_or(0);

        let mut result = json!({
            "total": filtered_total,
            "messages": messages,
            "range": { "from": first_msg_id, "to": last_msg_id, "target": target_message_id }
        });
        if !session_meta.as_object().unwrap().is_empty() {
            result["session"] = session_meta;
        }
        return Ok(result);
    }

    // 传统分页模式：先过滤 system 噪声，再切片
    let filtered_messages: Vec<_> = all_messages
        .iter()
        .filter(|m| !is_system_noise(m))
        .collect();

    let filtered_total = filtered_messages.len();
    if filtered_total == 0 {
        return Ok(json!({
            "total": 0,
            "messages": [],
            "range": { "from": 0, "to": 0 }
        }));
    }

    let desc = order == "desc";
    let (from_idx, to_idx) = if desc {
        // desc: 从尾部倒数，offset 表示从尾部跳过的条数
        let end = filtered_total.saturating_sub(offset);
        let from = end.saturating_sub(limit);
        (from, end)
    } else {
        // asc: offset 表示从头部跳过的条数
        let from = offset.min(filtered_total);
        let to = (from + limit).min(filtered_total);
        (from, to)
    };

    let effective_limit = to_idx - from_idx;
    let messages: Vec<Value> = filtered_messages[from_idx..to_idx]
        .iter()
        .map(|m| {
            let content = if effective_limit > 5 {
                truncate_str(&m.content_full, 500)
            } else {
                m.content_full.clone()
            };
            json!({
                "role": format!("{:?}", m.r#type).to_lowercase(),
                "text": content,
                "at": m.id
            })
        })
        .collect();

    // range 使用实际返回的消息的 ID
    let first_msg_id = filtered_messages.get(from_idx).map(|m| m.id).unwrap_or(0);
    let last_msg_id = filtered_messages
        .get(to_idx.saturating_sub(1))
        .map(|m| m.id)
        .unwrap_or(0);

    let mut result = json!({
        "total": filtered_total,
        "messages": messages,
        "range": { "from": first_msg_id, "to": last_msg_id },
        "pagination": { "offset": from_idx, "limit": effective_limit, "has_more": to_idx < filtered_total }
    });
    if !session_meta.as_object().unwrap().is_empty() {
        result["session"] = session_meta;
    }
    Ok(result)
}

/// 获取最近会话
async fn get_recent_sessions(state: &AppState, args: Value) -> Result<Value, String> {
    let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(5) as usize;
    let include_summary = args
        .get("include_summary")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 解析 cwd 参数（支持 string 或 array）
    let include_cwds = parse_cwd_param(args.get("cwd"));
    let exclude_cwds = parse_cwd_param(args.get("exclude_cwd"));

    // 构建项目过滤器
    let filter = ProjectFilter {
        include: find_project_ids_by_cwds(state, &include_cwds).await,
        exclude: find_project_ids_by_cwds(state, &exclude_cwds).await,
    };

    // 对于单个 include 项目，使用 SQL 层过滤
    let sql_project_id = filter.single_include();
    let need_memory_filter = !filter.is_empty() && sql_project_id.is_none();

    // 多取一些用于过滤
    let fetch_limit = if need_memory_filter { limit * 3 } else { limit };
    let sessions = state
        .db
        .get_sessions(sql_project_id, fetch_limit)
        .await
        .map_err(|e| e.to_string())?;

    // 获取项目名称映射
    let projects = state
        .db
        .list_projects_with_stats(1000, 0)
        .await
        .unwrap_or_default();
    let project_names: HashMap<i64, String> =
        projects.into_iter().map(|p| (p.id, p.name)).collect();

    // 过滤会话
    let filtered_sessions: Vec<_> = sessions
        .iter()
        .filter(|s| {
            if need_memory_filter {
                filter.matches(s.project_id)
            } else {
                true
            }
        })
        .take(limit)
        .collect();

    // 如果需要 summary，批量获取（L3 优先，fallback 到 L1）
    let summaries: HashMap<String, Value> = if include_summary {
        if let Some(compact_db) = &state.compact_db {
            let mut map = HashMap::new();
            for s in &filtered_sessions {
                // 优先获取 L3 session summary
                if let Ok(Some(summary)) = compact_db.get_session_summary(&s.session_id).await {
                    map.insert(
                        s.session_id.clone(),
                        json!({
                            "summary": summary.summary,
                            "keyPoints": summary.key_points,
                            "files": summary.files_involved,
                            "technologies": summary.technologies
                        }),
                    );
                } else {
                    // Fallback: 获取最近的 observations
                    if let Ok(observations) = compact_db.get_observations(&s.session_id).await {
                        if !observations.is_empty() {
                            // 取最后几个 observation（最近的操作）
                            let recent: Vec<_> = observations.iter().rev().take(5).collect();
                            let titles: Vec<&str> =
                                recent.iter().map(|o| o.title.as_str()).collect();
                            let files: Vec<String> = recent
                                .iter()
                                .filter_map(|o| o.files_modified.clone())
                                .flatten()
                                .collect();
                            map.insert(
                                s.session_id.clone(),
                                json!({
                                    "recentActions": titles,
                                    "files": if files.is_empty() { None } else { Some(files) }
                                }),
                            );
                        }
                    }
                }
            }
            map
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };

    let formatted: Vec<Value> = filtered_sessions
        .iter()
        .map(|s| {
            let project_name = project_names
                .get(&s.project_id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            let mut obj = json!({
                "ref": &s.session_id,
                "project": project_name,
                "messages": s.message_count,
                "time": ms_to_relative_time(s.last_message_at)
            });
            // 附加 session_type 和 source（如果有）
            if let Some(ref t) = s.session_type {
                obj["type"] = json!(t);
            }
            if let Some(ref src) = s.source {
                obj["source"] = json!(src);
            }
            // 附加 summary（如果有）
            if let Some(summary) = summaries.get(&s.session_id) {
                obj["summary"] = summary.clone();
            }
            obj
        })
        .collect();

    Ok(json!({ "sessions": formatted }))
}

/// 列出所有项目
async fn list_projects(state: &AppState, _args: Value) -> Result<Value, String> {
    let projects = state
        .db
        .list_projects_with_stats(1000, 0)
        .await
        .map_err(|e| e.to_string())?;

    let formatted: Vec<Value> = projects
        .iter()
        .map(|p| {
            json!({
                "name": &p.name,
                "path": &p.path,
                "sessions": p.session_count,
                "time": ms_to_relative_time(p.last_active)
            })
        })
        .collect();

    Ok(json!({ "projects": formatted }))
}

async fn extract_knowledge(state: &AppState, args: Value) -> Result<Value, String> {
    let service = state
        .knowledge
        .clone()
        .ok_or("knowledge extraction not enabled — set knowledge.enabled=true in config")?;

    // Check status / get current job
    if args
        .get("status")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let job = state.knowledge_job.read().await;
        return Ok(json!({ "job": *job }));
    }

    // Check if already running
    {
        let job = state.knowledge_job.read().await;
        if let Some(ref j) = *job {
            if j.status == "running" {
                return Ok(json!({
                    "status": "already_running",
                    "job": j,
                }));
            }
        }
    }

    let project_filter = args.get("project").and_then(|v| v.as_str());
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(usize::MAX);
    let project_desc = args
        .get("desc")
        .and_then(|v| v.as_str())
        .unwrap_or("a software engineering project")
        .to_string();

    let projects = state.db.list_projects().await.map_err(|e| e.to_string())?;
    let project_ids: Vec<i64> = if let Some(filter) = project_filter {
        projects
            .iter()
            .filter(|p| p.path.contains(filter) || p.name.contains(filter))
            .map(|p| p.id)
            .collect()
    } else {
        projects.iter().map(|p| p.id).collect()
    };

    if project_ids.is_empty() {
        return Ok(json!({"error": "no projects matched", "filter": project_filter}));
    }

    let pending = service
        .count_unprocessed(&project_ids, limit)
        .await
        .map_err(|e| e.to_string())?;

    if pending == 0 {
        return Ok(json!({ "status": "no_pending", "totalSessions": 0 }));
    }

    let now = chrono::Local::now().format("%H:%M:%S").to_string();
    {
        let mut job = state.knowledge_job.write().await;
        *job = Some(crate::api::KnowledgeJobProgress {
            project_id: project_ids[0],
            total_sessions: pending,
            processed: 0,
            failed: 0,
            nodes_extracted: 0,
            status: "running".to_string(),
            error: None,
            started_at: now,
        });
    }

    state
        .knowledge_cancel
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let job_ref = state.knowledge_job.clone();
    let cancel_flag = state.knowledge_cancel.clone();
    tokio::spawn(async move {
        let job_ref2 = job_ref.clone();
        let result = service
            .process_project_with_progress(
                &project_ids,
                limit,
                &project_desc,
                move |_done, r| {
                    if let Ok(mut job) = job_ref2.try_write() {
                        if let Some(ref mut j) = *job {
                            j.processed = r.sessions_processed;
                            j.failed = r.sessions_failed;
                            j.nodes_extracted = r.nodes_extracted;
                        }
                    }
                },
                Some(cancel_flag),
            )
            .await;

        let mut job = job_ref.write().await;
        if let Some(ref mut j) = *job {
            match result {
                Ok(r) => {
                    j.processed = r.sessions_processed;
                    j.failed = r.sessions_failed;
                    j.nodes_extracted = r.nodes_extracted;
                    j.status = "done".to_string();
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg == "cancelled" {
                        j.status = "cancelled".to_string();
                    } else {
                        j.status = "error".to_string();
                        j.error = Some(msg);
                    }
                }
            }
        }
    });

    Ok(json!({
        "status": "started",
        "totalSessions": pending,
    }))
}

/// 根据 cwd 查找项目
async fn find_project_by_cwd(state: &AppState, cwd: &str) -> Option<i64> {
    let projects = state.db.list_projects_with_stats(1000, 0).await.ok()?;

    // 精确匹配
    if let Some(p) = projects.iter().find(|p| p.path == cwd) {
        return Some(p.id);
    }

    // 前缀匹配
    if let Some(p) = projects.iter().find(|p| cwd.starts_with(&p.path)) {
        return Some(p.id);
    }

    None
}

/// 解析 cwd 参数（支持 string 或 array）
fn parse_cwd_param(value: Option<&Value>) -> Vec<String> {
    match value {
        None => vec![],
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => vec![],
    }
}

/// 简单的 glob 通配符匹配
/// 支持 `*` 匹配任意字符序列（包括空字符串）
fn matches_glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (m, n) = (p.len(), t.len());

    // dp[i][j] = pattern[0..i] 是否匹配 text[0..j]
    let mut dp = vec![vec![false; n + 1]; m + 1];
    dp[0][0] = true;

    // 处理 pattern 开头的 * 可以匹配空字符串
    for i in 1..=m {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        } else {
            break;
        }
    }

    for i in 1..=m {
        for j in 1..=n {
            if p[i - 1] == '*' {
                // * 可以匹配空字符串 (dp[i-1][j]) 或匹配一个字符后继续 (dp[i][j-1])
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if p[i - 1] == t[j - 1] {
                // 普通字符匹配
                dp[i][j] = dp[i - 1][j - 1];
            }
            // 否则 dp[i][j] 保持 false
        }
    }

    dp[m][n]
}

/// 检查 cwd 是否包含通配符
fn is_glob_pattern(cwd: &str) -> bool {
    cwd.contains('*')
}

/// 根据多个 cwd 路径查找项目 ID 列表
/// 支持三种匹配模式：
/// 1. 通配符匹配（包含 *）: `*/ETerm*` 匹配所有包含 ETerm 的路径
/// 2. 精确匹配: `/path/to/project` 精确匹配
/// 3. 前缀匹配: `/path/to` 匹配以此开头的项目
async fn find_project_ids_by_cwds(state: &AppState, cwds: &[String]) -> Vec<i64> {
    if cwds.is_empty() {
        return vec![];
    }

    let projects = match state.db.list_projects_with_stats(1000, 0).await {
        Ok(p) => p,
        Err(_) => return vec![],
    };

    let mut result = Vec::new();
    for cwd in cwds {
        if is_glob_pattern(cwd) {
            // 通配符匹配：匹配所有符合模式的项目
            for p in &projects {
                if matches_glob(cwd, &p.path) && !result.contains(&p.id) {
                    result.push(p.id);
                }
            }
        } else {
            // 精确匹配
            if let Some(p) = projects.iter().find(|p| p.path == *cwd) {
                if !result.contains(&p.id) {
                    result.push(p.id);
                }
                continue;
            }
            // 前缀匹配
            if let Some(p) = projects.iter().find(|p| cwd.starts_with(&p.path)) {
                if !result.contains(&p.id) {
                    result.push(p.id);
                }
            }
        }
    }
    result
}

/// 项目过滤器（支持 include 和 exclude）
#[derive(Debug, Default)]
struct ProjectFilter {
    include: Vec<i64>,
    exclude: Vec<i64>,
}

impl ProjectFilter {
    /// 检查项目 ID 是否通过过滤器
    fn matches(&self, project_id: i64) -> bool {
        // 如果有 include 列表，项目必须在其中
        if !self.include.is_empty() && !self.include.contains(&project_id) {
            return false;
        }
        // 如果在 exclude 列表中，排除
        if self.exclude.contains(&project_id) {
            return false;
        }
        true
    }

    /// 是否为空过滤器（不过滤任何项目）
    fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    /// 获取单个 include 项目 ID（用于向后兼容）
    fn single_include(&self) -> Option<i64> {
        if self.include.len() == 1 && self.exclude.is_empty() {
            Some(self.include[0])
        } else {
            None
        }
    }
}

/// 获取 session_id 到 project_id 的映射
async fn get_session_project_map(state: &AppState, session_ids: &[String]) -> HashMap<String, i64> {
    let mut result = HashMap::new();
    for session_id in session_ids {
        if let Ok(Some(session)) = state.db.get_session(session_id).await {
            result.insert(session_id.clone(), session.project_id);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== 辅助函数测试 ====================

    #[test]
    fn test_parse_time_shortcut_days() {
        let ts = parse_time_shortcut("3d").unwrap();
        let now = Local::now().timestamp_millis();
        let three_days_ms = 3 * 24 * 60 * 60 * 1000;
        // 允许 1 秒误差
        assert!((now - ts - three_days_ms).abs() < 1000);
    }

    #[test]
    fn test_parse_time_shortcut_weeks() {
        let ts = parse_time_shortcut("1w").unwrap();
        let now = Local::now().timestamp_millis();
        let one_week_ms = 7 * 24 * 60 * 60 * 1000;
        assert!((now - ts - one_week_ms).abs() < 1000);
    }

    #[test]
    fn test_parse_time_shortcut_months() {
        let ts = parse_time_shortcut("1m").unwrap();
        let now = Local::now().timestamp_millis();
        let one_month_ms = 30 * 24 * 60 * 60 * 1000_i64;
        assert!((now - ts - one_month_ms).abs() < 1000);
    }

    #[test]
    fn test_parse_time_shortcut_invalid() {
        assert!(parse_time_shortcut("abc").is_none());
        assert!(parse_time_shortcut("").is_none());
        assert!(parse_time_shortcut("3x").is_none());
    }

    #[test]
    fn test_ms_to_relative_time() {
        let now = Local::now().timestamp_millis();

        // just now
        assert_eq!(ms_to_relative_time(Some(now)), Some("just now".to_string()));

        // minutes ago
        let five_min_ago = now - 5 * 60 * 1000;
        assert_eq!(
            ms_to_relative_time(Some(five_min_ago)),
            Some("5m ago".to_string())
        );

        // hours ago
        let two_hours_ago = now - 2 * 60 * 60 * 1000;
        assert_eq!(
            ms_to_relative_time(Some(two_hours_ago)),
            Some("2h ago".to_string())
        );

        // days ago
        let three_days_ago = now - 3 * 24 * 60 * 60 * 1000;
        assert_eq!(
            ms_to_relative_time(Some(three_days_ago)),
            Some("3d ago".to_string())
        );
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 5), "hello...");
        assert_eq!(truncate_str("你好世界", 2), "你好...");
    }

    // ==================== cwd 参数解析测试 ====================

    #[test]
    fn test_parse_cwd_param_none() {
        let result = parse_cwd_param(None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_cwd_param_string() {
        let value = json!("/path/to/project");
        let result = parse_cwd_param(Some(&value));
        assert_eq!(result, vec!["/path/to/project"]);
    }

    #[test]
    fn test_parse_cwd_param_array() {
        let value = json!(["/path/a", "/path/b", "/path/c"]);
        let result = parse_cwd_param(Some(&value));
        assert_eq!(result, vec!["/path/a", "/path/b", "/path/c"]);
    }

    #[test]
    fn test_parse_cwd_param_invalid() {
        // 数字
        let value = json!(123);
        let result = parse_cwd_param(Some(&value));
        assert!(result.is_empty());

        // 对象
        let value = json!({"path": "/foo"});
        let result = parse_cwd_param(Some(&value));
        assert!(result.is_empty());
    }

    // ==================== ProjectFilter 测试 ====================

    #[test]
    fn test_project_filter_empty() {
        let filter = ProjectFilter::default();
        assert!(filter.is_empty());
        assert!(filter.matches(1));
        assert!(filter.matches(999));
        assert!(filter.single_include().is_none());
    }

    #[test]
    fn test_project_filter_single_include() {
        let filter = ProjectFilter {
            include: vec![1],
            exclude: vec![],
        };
        assert!(!filter.is_empty());
        assert!(filter.matches(1));
        assert!(!filter.matches(2));
        assert_eq!(filter.single_include(), Some(1));
    }

    #[test]
    fn test_project_filter_multi_include() {
        let filter = ProjectFilter {
            include: vec![1, 2, 3],
            exclude: vec![],
        };
        assert!(filter.matches(1));
        assert!(filter.matches(2));
        assert!(filter.matches(3));
        assert!(!filter.matches(4));
        assert!(filter.single_include().is_none()); // 多个 include 不返回单个
    }

    #[test]
    fn test_project_filter_exclude_only() {
        let filter = ProjectFilter {
            include: vec![],
            exclude: vec![1, 2],
        };
        assert!(!filter.matches(1));
        assert!(!filter.matches(2));
        assert!(filter.matches(3));
        assert!(filter.matches(999));
    }

    #[test]
    fn test_project_filter_include_and_exclude() {
        let filter = ProjectFilter {
            include: vec![1, 2, 3],
            exclude: vec![2],
        };
        assert!(filter.matches(1));
        assert!(!filter.matches(2)); // 在 exclude 中
        assert!(filter.matches(3));
        assert!(!filter.matches(4)); // 不在 include 中
        assert!(filter.single_include().is_none()); // 有 exclude，不返回单个
    }

    // ==================== Glob 通配符测试 ====================

    #[test]
    fn test_matches_glob_exact() {
        assert!(matches_glob("hello", "hello"));
        assert!(!matches_glob("hello", "world"));
        assert!(!matches_glob("hello", "hello world"));
    }

    #[test]
    fn test_matches_glob_star_end() {
        assert!(matches_glob("hello*", "hello"));
        assert!(matches_glob("hello*", "hello world"));
        assert!(matches_glob("hello*", "hellooooo"));
        assert!(!matches_glob("hello*", "hell"));
    }

    #[test]
    fn test_matches_glob_star_start() {
        assert!(matches_glob("*world", "world"));
        assert!(matches_glob("*world", "hello world"));
        assert!(!matches_glob("*world", "world!"));
    }

    #[test]
    fn test_matches_glob_star_middle() {
        assert!(matches_glob("hello*world", "helloworld"));
        assert!(matches_glob("hello*world", "hello world"));
        assert!(matches_glob("hello*world", "hello beautiful world"));
        assert!(!matches_glob("hello*world", "hello worlds"));
    }

    #[test]
    fn test_matches_glob_multiple_stars() {
        assert!(matches_glob("*ETerm*", "/Users/test/ETerm"));
        assert!(matches_glob("*ETerm*", "/Users/test/ETerm/subdir"));
        assert!(matches_glob("*ETerm*", "ETerm"));
        assert!(!matches_glob("*ETerm*", "/Users/test/english"));
    }

    #[test]
    fn test_matches_glob_path_patterns() {
        // 实际路径匹配场景
        assert!(matches_glob(
            "*/ETerm*",
            "/Users/higuaifan/Desktop/vimo/ETerm"
        ));
        assert!(matches_glob(
            "*/ETerm*",
            "/Users/higuaifan/Desktop/vimo/ETerm/memex"
        ));
        assert!(!matches_glob(
            "*memex*",
            "/Users/higuaifan/Desktop/hi/小工具/english"
        ));
        assert!(!matches_glob(
            "*memex*",
            "/Users/higuaifan/Desktop/vimo/ETerm"
        ));
    }

    #[test]
    fn test_matches_glob_edge_cases() {
        // 空字符串
        assert!(matches_glob("", ""));
        assert!(!matches_glob("", "a"));
        assert!(matches_glob("*", ""));
        assert!(matches_glob("*", "anything"));

        // 连续 *
        assert!(matches_glob("**", "anything"));
        assert!(matches_glob("a**b", "ab"));
        assert!(matches_glob("a**b", "aXXXb"));
    }

    #[test]
    fn test_is_glob_pattern() {
        assert!(is_glob_pattern("*ETerm*"));
        assert!(is_glob_pattern("*/vimo/*"));
        assert!(!is_glob_pattern("/Users/test/ETerm"));
        assert!(!is_glob_pattern(""));
    }

    #[test]
    fn test_date_to_timestamp() {
        // 测试日期解析
        let ts = date_to_timestamp("2024-01-15", true);
        assert!(ts.is_some());

        let ts_end = date_to_timestamp("2024-01-15", false);
        assert!(ts_end.is_some());

        // 结束时间应该大于开始时间（同一天）
        assert!(ts_end.unwrap() > ts.unwrap());

        // 无效日期
        assert!(date_to_timestamp("invalid", true).is_none());
    }

    // ==================== 工具 Schema 测试 ====================

    #[test]
    fn test_tools_schema() {
        let tools = get_tools();
        assert_eq!(tools.len(), 5);

        // search_history
        let search = &tools[0];
        assert_eq!(search["name"], "search_history");
        let props = &search["inputSchema"]["properties"];
        assert!(props.get("query").is_some());
        assert!(props.get("time").is_some());
        assert!(props.get("from").is_some());
        assert!(props.get("to").is_some());
        assert!(props.get("cwd").is_some());
        assert!(props.get("sessionId").is_some());

        // get_session
        let session = &tools[1];
        assert_eq!(session["name"], "get_session");
        let props = &session["inputSchema"]["properties"];
        assert!(props.get("sessionId").is_some());
        assert!(props.get("around").is_some());
        assert!(props.get("context").is_some());

        // get_recent_sessions
        let recent = &tools[2];
        assert_eq!(recent["name"], "get_recent_sessions");

        // list_projects
        let projects = &tools[3];
        assert_eq!(projects["name"], "list_projects");
    }

    // ==================== 输出格式测试（需要 mock DB，这里只测试格式逻辑） ====================

    #[test]
    fn test_search_result_format() {
        // 验证 search_history 返回的字段格式
        let result = json!({
            "session": "abc123",
            "role": "user",
            "snippet": "test snippet",
            "at": 42,
            "sessionMatches": 3,
            "time": "2h ago"
        });

        assert!(result.get("session").is_some());
        assert!(result.get("role").is_some());
        assert!(result.get("snippet").is_some());
        assert!(result.get("at").is_some());
        assert!(result.get("sessionMatches").is_some());
        assert!(result.get("time").is_some());

        // 确保没有旧字段
        assert!(result.get("messageId").is_none());
        assert!(result.get("projectId").is_none());
        assert!(result.get("content").is_none());
        assert!(result.get("score").is_none());
    }

    #[test]
    fn test_session_result_format() {
        // 验证 get_session 返回的字段格式
        let message = json!({
            "role": "user",
            "text": "hello",
            "at": 5
        });

        assert!(message.get("role").is_some());
        assert!(message.get("text").is_some());
        assert!(message.get("at").is_some());

        // 确保没有旧字段
        assert!(message.get("id").is_none());
        assert!(message.get("uuid").is_none());
        assert!(message.get("index").is_none());
        assert!(message.get("content").is_none());
    }

    #[test]
    fn test_recent_sessions_format() {
        // 验证 get_recent_sessions 返回的字段格式
        let session = json!({
            "ref": "abc123",
            "project": "ETerm",
            "messages": 42,
            "time": "2h ago"
        });

        assert!(session.get("ref").is_some());
        assert!(session.get("project").is_some());
        assert!(session.get("messages").is_some());
        assert!(session.get("time").is_some());

        // 确保没有旧字段
        assert!(session.get("id").is_none());
        assert!(session.get("projectId").is_none());
        assert!(session.get("messageCount").is_none());
        assert!(session.get("lastMessage").is_none());
    }

    #[test]
    fn test_projects_format() {
        // 验证 list_projects 返回的字段格式
        let project = json!({
            "name": "ETerm",
            "path": "/path/to/project",
            "sessions": 15,
            "time": "3d ago"
        });

        assert!(project.get("name").is_some());
        assert!(project.get("path").is_some());
        assert!(project.get("sessions").is_some());
        assert!(project.get("time").is_some());

        // 确保没有旧字段
        assert!(project.get("id").is_none());
        assert!(project.get("sessionCount").is_none());
        assert!(project.get("messageCount").is_none());
        assert!(project.get("lastActive").is_none());
    }

    // ==================== 集成测试 ====================
    // 以下测试验证跨 API 的语义一致性

    mod integration_tests {
        use super::*;
        use crate::api::AppState;
        use crate::backup::BackupService;
        use crate::config::Config;
        use crate::db_reader::DbReader;
        use crate::rag::RagService;
        use crate::search::HybridSearchService;
        use ai_cli_session_db::db::MessageInput;
        use ai_cli_session_db::{DbConfig, MessageType, SessionDB};
        use std::sync::Arc;
        use tempfile::tempdir;

        /// 创建测试用的 AppState
        async fn create_test_state() -> (Arc<AppState>, tempfile::TempDir) {
            let dir = tempdir().unwrap();
            let db_path = dir.path().join("test.db");
            let backup_dir = dir.path().join("backups");
            std::fs::create_dir_all(&backup_dir).unwrap();

            // 使用 SessionDB 直接写入测试数据
            let config = DbConfig::local(db_path.to_string_lossy().into_owned());
            let session_db = SessionDB::connect(config).unwrap();

            // 创建测试项目
            let project_id = session_db
                .get_or_create_project("TestProject", "/test/project", "test")
                .unwrap();

            // 创建测试会话
            let session_id = "test-session-12345678";
            session_db.upsert_session(session_id, project_id).unwrap();

            // 插入测试消息
            let now = chrono::Local::now().timestamp_millis();
            let messages: Vec<MessageInput> = (0..10)
                .map(|i| MessageInput {
                    uuid: format!("msg-uuid-{}", i),
                    r#type: if i % 2 == 0 {
                        MessageType::User
                    } else {
                        MessageType::Assistant
                    },
                    content_text: format!("消息内容 {}", i),
                    content_full: format!("消息内容 {} 完整版 searchable_keyword_{}", i, i),
                    timestamp: now + i * 1000,
                    sequence: i,
                    source: None,
                    channel: None,
                    model: None,
                    tool_call_id: None,
                    tool_name: None,
                    tool_args: None,
                    raw: None,
                    approval_status: None,
                    approval_resolved_at: None,
                })
                .collect();
            session_db.insert_messages(session_id, &messages).unwrap();

            // 释放 SessionDB，使用 DbReader 读取
            drop(session_db);

            // 创建 DbReader
            let db = Arc::new(DbReader::new(Some(db_path.clone())).unwrap());

            // 创建 AppState
            let config = Config::default();
            let knowledge_store = {
                let conn = rusqlite::Connection::open(&db_path).unwrap();
                crate::knowledge::KnowledgeStore::new(Arc::new(tokio::sync::Mutex::new(conn)))
            };
            let backup = BackupService::new(db_path, backup_dir);
            let hybrid_search = HybridSearchService::new(db.clone(), None, None);
            let rag_service = RagService::new(db.clone(), None, None, None);

            let state = AppState {
                config,
                db,
                backup,
                embedding: None,
                chat: None,
                vector: None,
                indexer: None,
                hybrid_search,
                rag_service,
                compact_db: None,
                compact_queue: None,
                compact_vector: None,
                remote_search: None,
                knowledge_store,
                knowledge: None,
                knowledge_job: Arc::new(tokio::sync::RwLock::new(None)),
                knowledge_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                startup_duration_ms: 0,
            };

            (Arc::new(state), dir)
        }

        /// 核心 round-trip 测试: search_history 返回的 at 可直接用于 get_session around
        #[tokio::test]
        async fn test_search_get_session_roundtrip() {
            let (state, _dir) = create_test_state().await;

            // Step 1: 搜索特定消息
            let search_result = call_tool(
                &state,
                "search_history",
                json!({ "query": "searchable_keyword_5", "limit": 1 }),
            )
            .await
            .unwrap();

            // 验证搜索结果有 at 字段
            let results = search_result["results"].as_array().unwrap();
            assert!(!results.is_empty(), "应该找到搜索结果");
            let first_result = &results[0];
            let at_value = first_result["at"]
                .as_i64()
                .expect("at 应该是 i64 类型的 message_id");

            // Step 2: 使用 at 值获取上下文
            let session_result = call_tool(
                &state,
                "get_session",
                json!({
                    "sessionId": "test-session-12345678",
                    "around": at_value,
                    "context": 2
                }),
            )
            .await
            .unwrap();

            // 验证返回的消息列表
            let messages = session_result["messages"].as_array().unwrap();
            assert!(!messages.is_empty(), "应该返回消息");

            // 关键验证: 返回的消息中应该包含 at_value 对应的消息
            let found_target = messages.iter().any(|m| m["at"].as_i64() == Some(at_value));
            assert!(
                found_target,
                "round-trip: get_session 返回的消息应包含目标 at={} 的消息",
                at_value
            );

            // 验证 range.target 也应该是 at_value
            let target_in_range = session_result["range"]["target"].as_i64();
            assert_eq!(
                target_in_range,
                Some(at_value),
                "range.target 应该与 around 参数一致"
            );
        }

        /// around 找不到消息应该返回错误，而不是静默 fallback
        #[tokio::test]
        async fn test_around_message_not_found_error() {
            let (state, _dir) = create_test_state().await;

            // 使用不存在的 message_id
            let result = call_tool(
                &state,
                "get_session",
                json!({
                    "sessionId": "test-session-12345678",
                    "around": 999999, // 不存在的 message_id
                    "context": 2
                }),
            )
            .await;

            // 应该返回错误
            assert!(
                result.is_err(),
                "around 找不到消息应该返回错误，不应静默 fallback"
            );
            let err = result.unwrap_err();
            assert!(
                err.contains("Message not found"),
                "错误信息应包含 'Message not found': {}",
                err
            );
        }

        /// time 和 from/to 应该互斥
        #[tokio::test]
        async fn test_time_from_mutual_exclusion() {
            let (state, _dir) = create_test_state().await;

            let result = call_tool(
                &state,
                "search_history",
                json!({
                    "query": "test",
                    "time": "1d",
                    "from": "2024-01-01"
                }),
            )
            .await;

            assert!(result.is_err(), "time 和 from/to 同时使用应该报错");
            let err = result.unwrap_err();
            assert!(
                err.contains("mutually exclusive"),
                "错误信息应包含 'mutually exclusive': {}",
                err
            );
        }

        /// 无效日期应该返回明确错误
        #[tokio::test]
        async fn test_invalid_date_error() {
            let (state, _dir) = create_test_state().await;

            let result = call_tool(
                &state,
                "search_history",
                json!({
                    "query": "test",
                    "from": "invalid-date"
                }),
            )
            .await;

            assert!(result.is_err(), "无效日期应该报错");
            let err = result.unwrap_err();
            assert!(
                err.contains("Invalid from date"),
                "错误信息应包含 'Invalid from date': {}",
                err
            );
        }

        /// 无效时间快捷方式应该返回错误
        #[tokio::test]
        async fn test_invalid_time_shortcut_error() {
            let (state, _dir) = create_test_state().await;

            let result = call_tool(
                &state,
                "search_history",
                json!({
                    "query": "test",
                    "time": "abc"
                }),
            )
            .await;

            assert!(result.is_err(), "无效时间快捷方式应该报错");
            let err = result.unwrap_err();
            assert!(
                err.contains("Invalid time shortcut"),
                "错误信息应包含 'Invalid time shortcut': {}",
                err
            );
        }

        /// at 模式：获取单条消息完整内容
        #[tokio::test]
        async fn test_at_mode_single_message() {
            let (state, _dir) = create_test_state().await;

            // 先搜索获取一个 at 值
            let search_result = call_tool(
                &state,
                "search_history",
                json!({ "query": "searchable_keyword_5", "limit": 1 }),
            )
            .await
            .unwrap();

            let at_value = search_result["results"][0]["at"].as_i64().unwrap();

            // 使用 at 模式获取单条消息
            let result = call_tool(
                &state,
                "get_session",
                json!({
                    "sessionId": "test-session-12345678",
                    "at": at_value
                }),
            )
            .await
            .unwrap();

            // 应该返回 message 而不是 messages
            assert!(
                result.get("message").is_some(),
                "at 模式应返回 message 字段"
            );
            assert!(
                result.get("messages").is_none(),
                "at 模式不应返回 messages 字段"
            );

            let message = &result["message"];
            assert_eq!(message["at"].as_i64(), Some(at_value));

            // 验证内容完整，包含 searchable_keyword_5
            let text = message["text"].as_str().unwrap();
            assert!(
                text.contains("searchable_keyword_5"),
                "at 模式应返回完整内容: {}",
                text
            );
            assert!(!text.ends_with("..."), "at 模式不应截断内容");
        }

        /// at 模式：消息不存在时报错
        #[tokio::test]
        async fn test_at_mode_message_not_found() {
            let (state, _dir) = create_test_state().await;

            let result = call_tool(
                &state,
                "get_session",
                json!({
                    "sessionId": "test-session-12345678",
                    "at": 999999
                }),
            )
            .await;

            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.contains("Message not found"));
        }

        /// 创建包含两个 session 的测试 AppState（用于 sessionId 过滤测试）
        async fn create_test_state_two_sessions() -> (Arc<AppState>, tempfile::TempDir) {
            let dir = tempdir().unwrap();
            let db_path = dir.path().join("test.db");
            let backup_dir = dir.path().join("backups");
            std::fs::create_dir_all(&backup_dir).unwrap();

            let config = DbConfig::local(db_path.to_string_lossy().into_owned());
            let session_db = SessionDB::connect(config).unwrap();

            let project_id = session_db
                .get_or_create_project("TestProject", "/test/project", "test")
                .unwrap();

            let now = chrono::Local::now().timestamp_millis();

            // Session A: 包含 "alpha" 关键字
            let session_a = "aaaa-1111-session-alpha";
            session_db.upsert_session(session_a, project_id).unwrap();
            let messages_a: Vec<MessageInput> = (0..5)
                .map(|i| MessageInput {
                    uuid: format!("msg-a-{}", i),
                    r#type: if i % 2 == 0 {
                        MessageType::User
                    } else {
                        MessageType::Assistant
                    },
                    content_text: format!("alpha message {}", i),
                    content_full: format!(
                        "alpha unique_content_{} shared_keyword session_a_only",
                        i
                    ),
                    timestamp: now + i * 1000,
                    sequence: i,
                    source: None,
                    channel: None,
                    model: None,
                    tool_call_id: None,
                    tool_name: None,
                    tool_args: None,
                    raw: None,
                    approval_status: None,
                    approval_resolved_at: None,
                })
                .collect();
            session_db.insert_messages(session_a, &messages_a).unwrap();

            // Session B: 包含 "beta" 关键字
            let session_b = "bbbb-2222-session-beta";
            session_db.upsert_session(session_b, project_id).unwrap();
            let messages_b: Vec<MessageInput> = (0..5)
                .map(|i| MessageInput {
                    uuid: format!("msg-b-{}", i),
                    r#type: if i % 2 == 0 {
                        MessageType::User
                    } else {
                        MessageType::Assistant
                    },
                    content_text: format!("beta message {}", i),
                    content_full: format!(
                        "beta unique_content_{} shared_keyword session_b_only",
                        i
                    ),
                    timestamp: now + (i + 10) * 1000,
                    sequence: i,
                    source: None,
                    channel: None,
                    model: None,
                    tool_call_id: None,
                    tool_name: None,
                    tool_args: None,
                    raw: None,
                    approval_status: None,
                    approval_resolved_at: None,
                })
                .collect();
            session_db.insert_messages(session_b, &messages_b).unwrap();

            drop(session_db);

            let db = Arc::new(DbReader::new(Some(db_path.clone())).unwrap());
            let config = Config::default();
            let backup = BackupService::new(db_path.clone(), backup_dir);
            let hybrid_search = HybridSearchService::new(db.clone(), None, None);
            let rag_service = RagService::new(db.clone(), None, None, None);
            let knowledge_store = {
                let conn = rusqlite::Connection::open(&db_path).unwrap();
                crate::knowledge::KnowledgeStore::new(Arc::new(tokio::sync::Mutex::new(conn)))
            };

            let state = AppState {
                config,
                db,
                backup,
                embedding: None,
                chat: None,
                vector: None,
                indexer: None,
                hybrid_search,
                rag_service,
                compact_db: None,
                compact_queue: None,
                compact_vector: None,
                remote_search: None,
                knowledge_store,
                knowledge: None,
                knowledge_job: Arc::new(tokio::sync::RwLock::new(None)),
                knowledge_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                startup_duration_ms: 0,
            };

            (Arc::new(state), dir)
        }

        /// sessionId 过滤：只返回指定 session 的结果
        #[tokio::test]
        async fn test_session_id_filter() {
            let (state, _dir) = create_test_state_two_sessions().await;

            // 无 sessionId 过滤：搜索 shared_keyword 应返回两个 session 的结果
            let all_results = call_tool(
                &state,
                "search_history",
                json!({ "query": "shared_keyword", "limit": 10 }),
            )
            .await
            .unwrap();
            let all_sessions: Vec<&str> = all_results["results"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|r| r["session"].as_str())
                .collect();
            assert!(
                all_sessions.len() >= 2,
                "无过滤时应返回至少 2 个 session 的结果"
            );

            // 用 sessionId 过滤：只返回 session A 的结果
            let filtered = call_tool(
                &state,
                "search_history",
                json!({
                    "query": "shared_keyword",
                    "sessionId": "aaaa-1111-session-alpha",
                    "limit": 10
                }),
            )
            .await
            .unwrap();
            let filtered_sessions: Vec<&str> = filtered["results"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|r| r["session"].as_str())
                .collect();
            assert_eq!(filtered_sessions.len(), 1, "过滤后应只有 1 个 session");
            assert!(filtered_sessions[0].starts_with("aaaa"), "应该是 session A");
        }

        /// sessionId 前缀匹配
        #[tokio::test]
        async fn test_session_id_prefix_filter() {
            let (state, _dir) = create_test_state_two_sessions().await;

            // 用前缀 "bbbb" 过滤
            let filtered = call_tool(
                &state,
                "search_history",
                json!({
                    "query": "shared_keyword",
                    "sessionId": "bbbb",
                    "limit": 10
                }),
            )
            .await
            .unwrap();
            let results = filtered["results"].as_array().unwrap();
            assert_eq!(results.len(), 1, "前缀过滤后应只有 1 个结果");
            assert!(
                results[0]["session"].as_str().unwrap().starts_with("bbbb"),
                "应该是 session B"
            );
        }

        /// sessionId 数组过滤多个 session
        #[tokio::test]
        async fn test_session_id_array_filter() {
            let (state, _dir) = create_test_state_two_sessions().await;

            // 用数组过滤两个 session
            let filtered = call_tool(
                &state,
                "search_history",
                json!({
                    "query": "shared_keyword",
                    "sessionId": ["aaaa", "bbbb"],
                    "limit": 10
                }),
            )
            .await
            .unwrap();
            let results = filtered["results"].as_array().unwrap();
            assert!(
                results.len() >= 2,
                "数组过滤两个 session 应返回至少 2 个结果"
            );
        }

        /// sessionId 过滤不匹配时返回空结果
        #[tokio::test]
        async fn test_session_id_no_match() {
            let (state, _dir) = create_test_state_two_sessions().await;

            let filtered = call_tool(
                &state,
                "search_history",
                json!({
                    "query": "shared_keyword",
                    "sessionId": "nonexistent-session",
                    "limit": 10
                }),
            )
            .await
            .unwrap();
            let results = filtered["results"].as_array().unwrap();
            assert_eq!(results.len(), 0, "不匹配的 sessionId 应返回空结果");
        }

        /// 验证 at 字段类型一致性
        #[tokio::test]
        async fn test_at_field_type_consistency() {
            let (state, _dir) = create_test_state().await;

            // search_history 返回的 at
            let search_result = call_tool(
                &state,
                "search_history",
                json!({ "query": "searchable_keyword_3", "limit": 1 }),
            )
            .await
            .unwrap();
            let search_at = search_result["results"][0]["at"].as_i64();
            assert!(search_at.is_some(), "search_history 的 at 应该是 i64");

            // get_session 返回的 messages 中的 at
            let session_result = call_tool(
                &state,
                "get_session",
                json!({
                    "sessionId": "test-session-12345678",
                    "limit": 5
                }),
            )
            .await
            .unwrap();

            let messages = session_result["messages"].as_array().unwrap();
            for msg in messages {
                let msg_at = msg["at"].as_i64();
                assert!(
                    msg_at.is_some(),
                    "get_session 消息的 at 应该是 i64: {:?}",
                    msg["at"]
                );
            }
        }
    }
}
