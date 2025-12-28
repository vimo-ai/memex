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
use crate::domain::to_local_time;

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

/// MCP 工具定义
fn get_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "search_history",
            "description": "搜索 Claude Code 历史对话，支持全文搜索、向量语义搜索和混合搜索",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "搜索关键词" },
                    "mode": {
                        "type": "string",
                        "enum": ["fts", "vector", "hybrid"],
                        "description": "搜索模式：fts (全文搜索) / vector (语义搜索) / hybrid (混合搜索，默认)"
                    },
                    "cwd": { "type": "string", "description": "当前工作目录，用于匹配项目并过滤结果" },
                    "limit": { "type": "number", "description": "返回数量，默认 10" }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "get_session",
            "description": "获取会话详情，支持分页和会话内搜索。返回会话基本信息和消息列表。注意：limit > 5 时内容会被截断（最多 500 字符），如需完整内容请设置 limit ≤ 5 或分多次获取",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string", "description": "会话 ID（完整 UUID 或前缀）" },
                    "offset": { "type": "number", "description": "从第几条消息开始，默认 0" },
                    "limit": { "type": "number", "description": "返回消息数量，默认 10" },
                    "order": { "type": "string", "enum": ["asc", "desc"], "description": "排序方式：asc (从头开始，默认) / desc (从尾部开始，获取最新消息)" },
                    "search": { "type": "string", "description": "会话内搜索关键词，自动定位到匹配位置并返回匹配消息" }
                },
                "required": ["sessionId"]
            }
        }),
        json!({
            "name": "get_recent_sessions",
            "description": "获取最近的会话列表，按更新时间倒序排列。可选择性过滤指定项目的会话",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cwd": { "type": "string", "description": "当前工作目录，用于匹配项目并过滤结果" },
                    "limit": { "type": "number", "description": "返回数量，默认 5" }
                }
            }
        }),
        json!({
            "name": "list_projects",
            "description": "列出所有项目，包括项目名称、路径、会话数量等信息",
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

        "tools/list" => MCPResponse::success(
            id,
            json!({ "tools": get_tools() }),
        ),

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
                Err(e) => MCPResponse::error(id, -32602, "Tool execution failed", Some(json!(e))),
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

/// 搜索历史对话
async fn search_history(state: &AppState, args: Value) -> Result<Value, String> {
    let query = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
    let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(10) as usize;
    let cwd = args.get("cwd").and_then(|c| c.as_str());

    if query.is_empty() {
        return Ok(json!({ "results": [], "total": 0 }));
    }

    // 根据 cwd 查找项目 ID
    let project_id = if let Some(cwd_path) = cwd {
        find_project_by_cwd(state, cwd_path)
    } else {
        None
    };

    // 执行 FTS 搜索
    let results = state.db.search(query, limit, project_id)
        .map_err(|e| e.to_string())?;

    let formatted: Vec<Value> = results.iter().map(|r| {
        json!({
            "messageId": r.message_id,
            "sessionId": r.session_id,
            "projectId": r.project_id,
            "projectName": r.project_name,
            "type": r.r#type,
            "content": truncate_str(&r.content, 500),
            "snippet": r.snippet,
            "score": r.score,
            "timestamp": to_local_time(r.timestamp.as_deref())
        })
    }).collect();

    Ok(json!({
        "results": formatted,
        "total": formatted.len()
    }))
}

/// 获取会话详情
async fn get_session(state: &AppState, args: Value) -> Result<Value, String> {
    let session_id_input = args.get("sessionId").and_then(|s| s.as_str())
        .ok_or("sessionId is required")?;
    let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(10) as usize;
    let order = args.get("order").and_then(|o| o.as_str()).unwrap_or("asc");
    let search = args.get("search").and_then(|s| s.as_str());
    let desc = order == "desc";

    // 支持前缀匹配：先解析完整 session ID
    let session_id = state.db.resolve_session_id(session_id_input)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Session not found: {}", session_id_input))?;

    // 获取消息总数
    let total_count = state.db.get_session_message_count(&session_id)
        .map_err(|e| e.to_string())? as usize;

    if total_count == 0 {
        return Err(format!("Session not found: {}", session_id));
    }

    // 如果有搜索词，需要获取全部消息来定位
    let messages: Vec<Value> = if let Some(keyword) = search {
        let all_messages = state.db.get_messages(&session_id)
            .map_err(|e| e.to_string())?;

        let keyword_lower = keyword.to_lowercase();
        let offset = all_messages.iter()
            .position(|m| m.content.to_lowercase().contains(&keyword_lower))
            .unwrap_or(0);

        all_messages.iter()
            .skip(offset)
            .take(limit)
            .enumerate()
            .map(|(idx, m)| {
                let content = if limit > 5 {
                    truncate_str(&m.content, 500)
                } else {
                    m.content.clone()
                };
                json!({
                    "id": m.id,
                    "uuid": m.uuid,
                    "type": m.r#type,
                    "content": content,
                    "timestamp": to_local_time(m.timestamp.as_deref()),
                    "index": offset + idx
                })
            })
            .collect()
    } else {
        // 直接使用数据库排序和分页
        let db_messages = state.db.get_messages_with_options(&session_id, Some(limit), desc)
            .map_err(|e| e.to_string())?;

        db_messages.iter()
            .enumerate()
            .map(|(idx, m)| {
                let content = if limit > 5 {
                    truncate_str(&m.content, 500)
                } else {
                    m.content.clone()
                };
                let index = if desc { total_count - 1 - idx } else { idx };
                json!({
                    "id": m.id,
                    "uuid": m.uuid,
                    "type": m.r#type,
                    "content": content,
                    "timestamp": to_local_time(m.timestamp.as_deref()),
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
        find_project_by_cwd(state, cwd_path)
    } else {
        None
    };

    let sessions = state.db.get_sessions(project_id, limit)
        .map_err(|e| e.to_string())?;

    let formatted: Vec<Value> = sessions.iter().map(|s| {
        json!({
            "id": s.id,
            "projectId": s.project_id,
            "projectName": s.project_name,
            "messageCount": s.message_count,
            "firstMessage": to_local_time(s.first_message.as_deref()),
            "lastMessage": to_local_time(s.last_message.as_deref())
        })
    }).collect();

    Ok(json!({
        "sessions": formatted,
        "total": formatted.len()
    }))
}

/// 列出所有项目
async fn list_projects(state: &AppState, _args: Value) -> Result<Value, String> {
    let projects = state.db.get_projects()
        .map_err(|e| e.to_string())?;

    let formatted: Vec<Value> = projects.iter().map(|p| {
        json!({
            "id": p.id,
            "name": p.name,
            "path": p.path,
            "sessionCount": p.session_count,
            "messageCount": p.message_count,
            "lastActive": to_local_time(p.last_active.as_deref())
        })
    }).collect();

    Ok(json!({
        "projects": formatted,
        "total": formatted.len()
    }))
}

/// 根据 cwd 查找项目
fn find_project_by_cwd(state: &AppState, cwd: &str) -> Option<i64> {
    let projects = state.db.get_projects().ok()?;

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
