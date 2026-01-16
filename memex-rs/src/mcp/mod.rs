//! MCP (Model Context Protocol) HTTP 传输协议实现
//!
//! 实现 JSON-RPC 2.0 协议，提供 4 个 MCP 工具：
//! - search_history: 搜索历史对话
//! - get_session: 获取会话详情
//! - get_recent_sessions: 获取最近会话
//! - list_projects: 列出项目

#![allow(dead_code)] // JSON-RPC 字段由 serde 使用

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::api::AppState;

/// 毫秒时间戳转本地时间字符串
fn ms_to_local_time(ts: Option<i64>) -> Option<String> {
    ts.map(|ms| {
        use chrono::{Local, TimeZone};
        Local
            .timestamp_millis_opt(ms)
            .single()
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| format!("{}", ms))
    })
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
            "description": "Search Claude Code conversation history. Supports full-text search, vector semantic search, and hybrid search",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search keywords" },
                    "mode": {
                        "type": "string",
                        "enum": ["fts", "vector", "hybrid"],
                        "description": "Search mode: fts (full-text) / vector (semantic) / hybrid (default)"
                    },
                    "orderBy": {
                        "type": "string",
                        "enum": ["score", "time_desc", "time_asc"],
                        "description": "Sort order: score (relevance, default) / time_desc (newest first) / time_asc (oldest first). Note: time sorting auto-degrades to FTS-only mode"
                    },
                    "startDate": { "type": "string", "description": "Start date filter (YYYY-MM-DD format, inclusive)" },
                    "endDate": { "type": "string", "description": "End date filter (YYYY-MM-DD format, inclusive)" },
                    "cwd": { "type": "string", "description": "Current working directory, used to match project and filter results" },
                    "limit": { "type": "number", "description": "Number of results to return, default 10" }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "get_session",
            "description": "Get session details with pagination and in-session search. Returns session info and message list. Note: content is truncated (max 500 chars) when limit > 5. Set limit <= 5 or paginate for full content",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string", "description": "Session ID (full UUID or prefix)" },
                    "offset": { "type": "number", "description": "Message offset, default 0" },
                    "limit": { "type": "number", "description": "Number of messages to return, default 10" },
                    "order": { "type": "string", "enum": ["asc", "desc"], "description": "Sort order: asc (from start, default) / desc (from end, get latest messages)" },
                    "search": { "type": "string", "description": "In-session search keyword, auto-locates matching position" }
                },
                "required": ["sessionId"]
            }
        }),
        json!({
            "name": "get_recent_sessions",
            "description": "Get recent sessions sorted by update time (descending). Optionally filter by project",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cwd": { "type": "string", "description": "Current working directory, used to match project and filter results" },
                    "limit": { "type": "number", "description": "Number of results to return, default 5" }
                }
            }
        }),
        json!({
            "name": "list_projects",
            "description": "List all projects with name, path, and session count",
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

/// 搜索历史对话
async fn search_history(state: &AppState, args: Value) -> Result<Value, String> {
    let query = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
    let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(10) as usize;
    let cwd = args.get("cwd").and_then(|c| c.as_str());
    let order_by_str = args
        .get("orderBy")
        .or_else(|| args.get("order_by"))
        .and_then(|o| o.as_str())
        .unwrap_or("score");

    // 日期范围参数（格式：YYYY-MM-DD）
    let start_date = args
        .get("startDate")
        .or_else(|| args.get("start_date"))
        .and_then(|d| d.as_str());
    let end_date = args
        .get("endDate")
        .or_else(|| args.get("end_date"))
        .and_then(|d| d.as_str());

    // 解析排序方式
    let order_by = match order_by_str {
        "time_desc" => claude_session_db::SearchOrderBy::TimeDesc,
        "time_asc" => claude_session_db::SearchOrderBy::TimeAsc,
        _ => claude_session_db::SearchOrderBy::Score,
    };

    if query.is_empty() {
        return Ok(json!({ "results": [], "total": 0 }));
    }

    // 根据 cwd 查找项目 ID
    let project_id = if let Some(cwd_path) = cwd {
        find_project_by_cwd(state, cwd_path).await
    } else {
        None
    };

    // 转换日期为时间戳
    let start_ts = start_date.and_then(|d| date_to_timestamp(d, true));
    let end_ts = end_date.and_then(|d| date_to_timestamp(d, false));

    // 执行 FTS 搜索（使用 SharedDbAdapter，日期过滤在 SQL 层完成）
    let results = state
        .db
        .search_fts_full(query, limit, project_id, order_by, start_ts, end_ts)
        .await
        .map_err(|e| e.to_string())?;

    let formatted: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "messageId": r.message_id,
                "sessionId": r.session_id,
                "projectId": r.project_id,
                "projectName": r.project_name,
                "type": r.r#type,
                "content": truncate_str(&r.content_full, 500),
                "snippet": r.snippet,
                "score": r.score,
                "timestamp": ms_to_local_time(r.timestamp)
            })
        })
        .collect();

    Ok(json!({
        "results": formatted,
        "total": formatted.len()
    }))
}

/// 获取会话详情
async fn get_session(state: &AppState, args: Value) -> Result<Value, String> {
    // 兼容 camelCase 和 snake_case 两种命名风格
    let session_id_input = args
        .get("sessionId")
        .or_else(|| args.get("session_id"))
        .and_then(|s| s.as_str())
        .ok_or("sessionId is required (also accepts session_id)")?;
    let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(10) as usize;
    let order = args.get("order").and_then(|o| o.as_str()).unwrap_or("asc");
    let search = args.get("search").and_then(|s| s.as_str());
    let desc = order == "desc";

    // 支持前缀匹配：先解析完整 session ID
    let session_id = state
        .db
        .resolve_session_id(session_id_input)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Session not found: {}", session_id_input))?;

    // 获取消息总数
    let total_count = state
        .db
        .get_session_message_count(&session_id)
        .await
        .map_err(|e| e.to_string())? as usize;

    if total_count == 0 {
        return Err(format!("Session not found: {}", session_id));
    }

    // 如果有搜索词，需要获取全部消息来定位
    let messages: Vec<Value> = if let Some(keyword) = search {
        let all_messages = state
            .db
            .get_messages(&session_id)
            .await
            .map_err(|e| e.to_string())?;

        let keyword_lower = keyword.to_lowercase();
        let offset = all_messages
            .iter()
            .position(|m| m.content_full.to_lowercase().contains(&keyword_lower))
            .unwrap_or(0);

        all_messages
            .iter()
            .skip(offset)
            .take(limit)
            .enumerate()
            .map(|(idx, m)| {
                let content = if limit > 5 {
                    truncate_str(&m.content_full, 500)
                } else {
                    m.content_full.clone()
                };
                json!({
                    "id": m.id,
                    "uuid": m.uuid,
                    "type": format!("{:?}", m.r#type),
                    "content": content,
                    "timestamp": ms_to_local_time(Some(m.timestamp)),
                    "index": offset + idx
                })
            })
            .collect()
    } else {
        // 直接使用数据库排序和分页
        let db_messages = state
            .db
            .get_messages_with_options(&session_id, Some(limit), desc)
            .await
            .map_err(|e| e.to_string())?;

        db_messages
            .iter()
            .enumerate()
            .map(|(idx, m)| {
                let content = if limit > 5 {
                    truncate_str(&m.content_full, 500)
                } else {
                    m.content_full.clone()
                };
                let index = if desc { total_count - 1 - idx } else { idx };
                json!({
                    "id": m.id,
                    "uuid": m.uuid,
                    "type": format!("{:?}", m.r#type),
                    "content": content,
                    "timestamp": ms_to_local_time(Some(m.timestamp)),
                    "index": index
                })
            })
            .collect()
    };

    Ok(json!({
        "session": {
            "id": session_id,
            "messageCount": total_count
        },
        "messages": messages,
        "pagination": {
            "order": order,
            "limit": limit,
            "total": total_count
        }
    }))
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

    let formatted: Vec<Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "id": s.session_id,
                "projectId": s.project_id,
                "messageCount": s.message_count,
                "lastMessage": ms_to_local_time(s.last_message_at)
            })
        })
        .collect();

    Ok(json!({
        "sessions": formatted,
        "total": formatted.len()
    }))
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
                "id": p.id,
                "name": p.name,
                "path": p.path,
                "sessionCount": p.session_count,
                "messageCount": p.message_count,
                "lastActive": ms_to_local_time(p.last_active)
            })
        })
        .collect();

    Ok(json!({
        "projects": formatted,
        "total": formatted.len()
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
