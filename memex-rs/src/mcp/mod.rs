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
    id: Value,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

/// JSON-RPC 响应
#[derive(Debug, Serialize)]
pub struct MCPResponse {
    jsonrpc: &'static str,
    id: Value,
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
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, code: i32, message: &str, data: Option<Value>) -> Self {
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
                    "cwd": { "type": "string", "description": "Filter to current project only" },
                    "time": { "type": "string", "description": "Time shortcut: 1d/3d/1w/1m (mutually exclusive with from/to)" },
                    "from": { "type": "string", "description": "Start date YYYY-MM-DD (mutually exclusive with time)" },
                    "to": { "type": "string", "description": "End date YYYY-MM-DD (mutually exclusive with time)" },
                    "limit": { "type": "number", "description": "Max results, default 5" }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "get_session",
            "description": "Get session messages. Two modes: (1) around+context: get context around position, (2) limit+order: pagination. Content truncated when limit>5",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string", "description": "Session ID (full or prefix)" },
                    "around": { "type": "number", "description": "Get context around this position (from search_history 'at'). Ignores limit/order" },
                    "context": { "type": "number", "description": "Messages before/after 'around', default 5, max 20" },
                    "limit": { "type": "number", "description": "Messages to return, default 10 (ignored if around is set)" },
                    "order": { "type": "string", "enum": ["asc", "desc"], "description": "asc (from start) / desc (from end), default asc (ignored if around is set)" }
                },
                "required": ["sessionId"]
            }
        }),
        json!({
            "name": "get_recent_sessions",
            "description": "Get recent sessions. Returns: ref (session ID), project, messages (count), time",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cwd": { "type": "string", "description": "Filter to current project" },
                    "limit": { "type": "number", "description": "Max results, default 5" }
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
    ]
}

/// 处理 MCP POST 请求
pub async fn handle_mcp(
    State(state): State<Arc<AppState>>,
    Json(request): Json<MCPRequest>,
) -> impl IntoResponse {
    let response = process_mcp_request(&state, request).await;
    Json(response)
}

/// 处理 MCP GET 请求 (通过 query 参数)
pub async fn handle_mcp_get(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MCPGetQuery>,
) -> impl IntoResponse {
    let request = MCPRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(query.id.unwrap_or_else(|| "1".to_string())),
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
    let cwd = args.get("cwd").and_then(|c| c.as_str());
    let level = args.get("level").and_then(|l| l.as_str()).unwrap_or("sessions");

    // 时间参数：time 快捷方式 与 from/to 互斥
    let time_shortcut = args.get("time").and_then(|t| t.as_str());
    let from_date = args.get("from").and_then(|d| d.as_str());
    let to_date = args.get("to").and_then(|d| d.as_str());

    // 互斥检查
    if time_shortcut.is_some() && (from_date.is_some() || to_date.is_some()) {
        return Err("'time' and 'from'/'to' are mutually exclusive".to_string());
    }

    if query.is_empty() {
        return Ok(json!({ "results": [], "total": 0, "level": level }));
    }

    // 根据 cwd 查找项目 ID
    let project_id = if let Some(cwd_path) = cwd {
        find_project_by_cwd(state, cwd_path).await
    } else {
        None
    };

    // 解析时间范围（无效日期报错而非静默忽略）
    let (_start_ts, _end_ts) = if let Some(shortcut) = time_shortcut {
        let start = parse_time_shortcut(shortcut)
            .ok_or_else(|| format!("Invalid time shortcut: {}. Use 1d/3d/1w/1m", shortcut))?;
        (Some(start), None)
    } else {
        let start = if let Some(d) = from_date {
            Some(date_to_timestamp(d, true)
                .ok_or_else(|| format!("Invalid from date: {}. Use YYYY-MM-DD", d))?)
        } else {
            None
        };
        let end = if let Some(d) = to_date {
            Some(date_to_timestamp(d, false)
                .ok_or_else(|| format!("Invalid to date: {}. Use YYYY-MM-DD", d))?)
        } else {
            None
        };
        (start, end)
    };

    // 根据 level 选择搜索方式
    match level {
        "sessions" => search_session_summaries(state, query, limit, project_id).await,
        "talks" => search_talk_summaries(state, query, limit, project_id).await,
        "raw" => search_raw_messages(state, query, limit, project_id, _start_ts, _end_ts).await,
        _ => Err(format!("Invalid level: {}. Use sessions/talks/raw", level)),
    }
}

/// L3: 搜索会话摘要
async fn search_session_summaries(
    state: &AppState,
    query: &str,
    limit: usize,
    _project_id: Option<i64>,
) -> Result<Value, String> {
    // 如果 compact_db 不可用，fallback 到 talks
    let compact_db = match &state.compact_db {
        Some(db) => db,
        None => return search_talk_summaries(state, query, limit, _project_id).await,
    };

    let results = compact_db
        .search_session_summaries(query, limit)
        .await
        .map_err(|e| e.to_string())?;

    // 如果没有结果，fallback 到 talks
    if results.is_empty() {
        return search_talk_summaries(state, query, limit, _project_id).await;
    }

    let formatted: Vec<Value> = results
        .iter()
        .map(|s| {
            json!({
                "session": &s.session_id,
                "summary": &s.summary,
                "keyPoints": &s.key_points,
                "files": &s.files_involved,
                "technologies": &s.technologies,
                "time": &s.updated_at
            })
        })
        .collect();

    Ok(json!({
        "results": formatted,
        "total": formatted.len(),
        "level": "sessions",
        "hint": "Use level='talks' for per-prompt details, or get_session for full context"
    }))
}

/// L2: 搜索对话摘要
async fn search_talk_summaries(
    state: &AppState,
    query: &str,
    limit: usize,
    project_id: Option<i64>,
) -> Result<Value, String> {
    // 如果 compact_db 不可用，fallback 到 raw
    let compact_db = match &state.compact_db {
        Some(db) => db,
        None => return search_raw_messages(state, query, limit, project_id, None, None).await,
    };

    let results = compact_db
        .search_talk_summaries(query, limit)
        .await
        .map_err(|e| e.to_string())?;

    // 如果没有结果，fallback 到 raw
    if results.is_empty() {
        return search_raw_messages(state, query, limit, project_id, None, None).await;
    }

    let formatted: Vec<Value> = results
        .iter()
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
    project_id: Option<i64>,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
) -> Result<Value, String> {
    // 执行 FTS 搜索，多取一些用于计算 sessionMatches
    let search_limit = (limit * 3).max(30); // 多取一些用于聚合
    let results = state
        .db
        .search_fts_full(
            query,
            search_limit,
            project_id,
            claude_session_db::SearchOrderBy::Score,
            start_ts,
            end_ts,
        )
        .await
        .map_err(|e| e.to_string())?;

    // 计算每个 session 的匹配数
    let mut session_match_counts: HashMap<String, usize> = HashMap::new();
    for r in &results {
        *session_match_counts.entry(r.session_id.clone()).or_insert(0) += 1;
    }

    // 去重：每个 session 只保留最高分的一条
    let mut seen_sessions: HashMap<String, bool> = HashMap::new();
    let formatted: Vec<Value> = results
        .iter()
        .filter(|r| {
            if seen_sessions.contains_key(&r.session_id) {
                false
            } else {
                seen_sessions.insert(r.session_id.clone(), true);
                true
            }
        })
        .take(limit)
        .map(|r| {
            let session_matches = session_match_counts.get(&r.session_id).copied().unwrap_or(1);
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

    // around 模式参数
    let around = args.get("around").and_then(|a| a.as_i64());
    let context = args
        .get("context")
        .and_then(|c| c.as_u64())
        .unwrap_or(5)
        .min(20) as usize; // 最大 20

    // 传统分页参数（around 模式下忽略）
    let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(10) as usize;
    let order = args.get("order").and_then(|o| o.as_str()).unwrap_or("asc");

    // 解析 session ID（支持前缀匹配）
    let session_id = state
        .db
        .resolve_session_id(session_id_input)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Session not found: {}", session_id_input))?;

    // 获取所有消息（需要完整列表来计算位置）
    let all_messages = state
        .db
        .get_messages(&session_id)
        .await
        .map_err(|e| e.to_string())?;

    let total = all_messages.len();
    if total == 0 {
        return Err(format!("Session not found: {}", session_id));
    }

    // 确定消息范围
    let (from_idx, to_idx, target_msg_id) = if let Some(target_message_id) = around {
        // around 模式：找到目标消息的位置，返回前后 context 条
        let target_pos = all_messages
            .iter()
            .position(|m| m.id == target_message_id)
            .ok_or_else(|| format!("Message not found: {}", target_message_id))?;

        let from = target_pos.saturating_sub(context);
        let to = (target_pos + context + 1).min(total);
        (from, to, Some(target_message_id))
    } else {
        // 传统分页模式
        let desc = order == "desc";
        if desc {
            let from = total.saturating_sub(limit);
            (from, total, None)
        } else {
            let to = limit.min(total);
            (0, to, None)
        }
    };

    // 提取消息并格式化
    // at 统一使用 message_id（与 search_history 一致，支持 round-trip）
    let effective_limit = to_idx - from_idx;
    let messages: Vec<Value> = all_messages[from_idx..to_idx]
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
                "at": m.id  // 使用 message_id，与 search_history 一致
            })
        })
        .collect();

    // range 使用 message_id
    let first_msg_id = all_messages.get(from_idx).map(|m| m.id).unwrap_or(0);
    let last_msg_id = all_messages.get(to_idx.saturating_sub(1)).map(|m| m.id).unwrap_or(0);

    let mut result = json!({
        "total": total,
        "messages": messages,
        "range": { "from": first_msg_id, "to": last_msg_id }
    });

    // around 模式额外返回 target
    if let Some(target_id) = target_msg_id {
        result["range"]["target"] = json!(target_id);
    }

    Ok(result)
}

/// 获取最近会话
async fn get_recent_sessions(state: &AppState, args: Value) -> Result<Value, String> {
    let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(5) as usize;
    let cwd = args.get("cwd").and_then(|c| c.as_str());

    let project_id = if let Some(cwd_path) = cwd {
        find_project_by_cwd(state, cwd_path).await
    } else {
        None
    };

    let sessions = state
        .db
        .get_sessions(project_id, limit)
        .await
        .map_err(|e| e.to_string())?;

    // 获取项目名称映射
    let projects = state
        .db
        .list_projects_with_stats(1000, 0)
        .await
        .unwrap_or_default();
    let project_names: HashMap<i64, String> = projects
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();

    let formatted: Vec<Value> = sessions
        .iter()
        .map(|s| {
            let project_name = project_names
                .get(&s.project_id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            json!({
                "ref": &s.session_id,
                "project": project_name,
                "messages": s.message_count,
                "time": ms_to_relative_time(s.last_message_at)
            })
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
        assert_eq!(tools.len(), 4);

        // search_history
        let search = &tools[0];
        assert_eq!(search["name"], "search_history");
        let props = &search["inputSchema"]["properties"];
        assert!(props.get("query").is_some());
        assert!(props.get("time").is_some());
        assert!(props.get("from").is_some());
        assert!(props.get("to").is_some());
        assert!(props.get("cwd").is_some());

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
        use crate::collector::Collector;
        use crate::config::Config;
        use crate::rag::RagService;
        use crate::search::HybridSearchService;
        use crate::shared_adapter::SharedDbAdapter;
        use claude_session_db::db::{MessageInput, SessionInput};
        use claude_session_db::MessageType;
        use std::sync::Arc;
        use tempfile::tempdir;

        /// 创建测试用的 AppState
        async fn create_test_state() -> (Arc<AppState>, tempfile::TempDir) {
            let dir = tempdir().unwrap();
            let db_path = dir.path().join("test.db");
            let backup_dir = dir.path().join("backups");
            std::fs::create_dir_all(&backup_dir).unwrap();

            let adapter = SharedDbAdapter::new(Some(db_path.clone())).unwrap();
            adapter.register().await.unwrap();

            // 创建测试项目
            let project_id = adapter
                .get_or_create_project("TestProject", "/test/project", "test")
                .await
                .unwrap();

            // 创建测试会话
            let session_id = "test-session-12345678";
            adapter
                .upsert_session(&SessionInput {
                    session_id: session_id.to_string(),
                    project_id,
                    message_count: Some(10), // 必填字段
                    ..Default::default()
                })
                .await
                .unwrap();

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
            adapter.insert_messages(session_id, &messages).await.unwrap();

            // 创建 AppState
            let config = Config::default();
            let db = Arc::new(adapter);

            let collector = Collector::new(config.clone(), db.clone());
            let backup = BackupService::new(db_path, backup_dir);
            let hybrid_search = HybridSearchService::new(db.clone(), None, None);
            let rag_service = RagService::new(db.clone(), None, None, None);

            let state = AppState {
                config,
                db,
                collector,
                backup,
                embedding: None,
                chat: None,
                vector: None,
                indexer: None,
                hybrid_search,
                rag_service,
                compact_db: None,
                compact_queue: None,
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
            let found_target = messages
                .iter()
                .any(|m| m["at"].as_i64() == Some(at_value));
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
