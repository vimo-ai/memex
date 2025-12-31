//! 领域模型 - API DTO 和类型转换
//!
//! 定义 API 响应格式（DTO），并提供从 claude_session_db 类型的转换

use chrono::{DateTime, Local, Utc};
use claude_session_db::{Message as DbMessage, Project as DbProject, Session as DbSession};
use serde::{Deserialize, Serialize};

/// 将 UTC 时间字符串转换为本地时区 ISO 8601 格式
pub fn to_local_time(ts: Option<&str>) -> Option<String> {
    let ts = ts?;

    // 尝试解析 UTC 格式 (以 Z 结尾)
    if ts.ends_with('Z') {
        if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
            let local: DateTime<Local> = dt.with_timezone(&Local);
            return Some(local.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string());
        }
    }

    // 尝试解析已有时区的格式
    if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
        let local: DateTime<Local> = dt.with_timezone(&Local);
        return Some(local.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string());
    }

    // 尝试解析无时区格式，假设为 UTC
    if let Ok(dt) = ts.parse::<DateTime<Utc>>() {
        let local: DateTime<Local> = dt.with_timezone(&Local);
        return Some(local.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string());
    }

    // 无法解析，返回原值
    Some(ts.to_string())
}

/// 获取当前时间的 ISO 8601 格式
fn now_local_iso() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string()
}

// ==================== API DTO 结构体 ====================

/// 项目响应 DTO (兼容 NestJS 格式)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDto {
    pub id: i64,
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoded_dir_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_count: Option<i64>,
}

/// 项目列表响应
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListDto {
    pub total: usize,
    pub projects: Vec<ProjectDto>,
}

/// 项目详情响应 (包含统计信息)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetailDto {
    pub id: i64,
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoded_dir_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub session_count: i64,
    pub message_count: i64,
}

/// 会话响应 DTO (兼容 NestJS 格式)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub id: String,
    pub project_id: i64,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
    pub message_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// 会话列表响应
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListDto {
    pub total: usize,
    pub sessions: Vec<SessionDto>,
}

/// 会话搜索响应
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchDto {
    pub query: String,
    pub total: usize,
    pub sessions: Vec<SessionDto>,
}

/// 消息响应 DTO (兼容 NestJS 格式)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDto {
    pub id: i64,
    pub uuid: String,
    pub session_id: String,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_args: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    pub created_at: String,
}

/// 消息列表响应
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageListDto {
    pub total: usize,
    pub messages: Vec<MessageDto>,
}

/// 项目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub session_count: i64,
    pub message_count: i64,
    pub last_active: Option<String>,
}

impl Project {
    /// 转换时间为本地时区
    pub fn with_local_time(mut self) -> Self {
        self.last_active = to_local_time(self.last_active.as_deref());
        self
    }

    /// 转换为 API DTO
    pub fn to_dto(self) -> ProjectDto {
        let now = now_local_iso();
        ProjectDto {
            id: self.id,
            name: self.name.clone(),
            path: self.path.clone(),
            encoded_dir_name: Some(encode_path(&self.path)),
            source: Some("claude".to_string()),
            created_at: self.last_active.clone().unwrap_or_else(|| now.clone()),
            updated_at: self.last_active.clone().unwrap_or_else(|| now.clone()),
            session_count: Some(self.session_count),
            message_count: Some(self.message_count),
        }
    }

    /// 转换为详情 DTO
    pub fn to_detail_dto(self) -> ProjectDetailDto {
        let now = now_local_iso();
        ProjectDetailDto {
            id: self.id,
            name: self.name.clone(),
            path: self.path.clone(),
            encoded_dir_name: Some(encode_path(&self.path)),
            source: Some("claude".to_string()),
            created_at: self.last_active.clone().unwrap_or_else(|| now.clone()),
            updated_at: self.last_active.clone().unwrap_or_else(|| now.clone()),
            session_count: self.session_count,
            message_count: self.message_count,
        }
    }
}

/// 编码路径为目录名 (与 Claude Code 格式兼容)
fn encode_path(path: &str) -> String {
    path.replace('/', "-")
        .trim_start_matches('-')
        .to_string()
}

/// 会话
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_id: i64,
    pub project_name: String,
    pub message_count: i64,
    pub first_message: Option<String>,
    pub last_message: Option<String>,
}

impl Session {
    /// 转换时间为本地时区
    pub fn with_local_time(mut self) -> Self {
        self.first_message = to_local_time(self.first_message.as_deref());
        self.last_message = to_local_time(self.last_message.as_deref());
        self
    }

    /// 转换为 API DTO
    pub fn to_dto(self) -> SessionDto {
        let now = now_local_iso();
        SessionDto {
            id: self.id,
            project_id: self.project_id,
            status: "completed".to_string(),
            source: Some("claude".to_string()),
            channel: Some("code".to_string()),
            cwd: None,
            model: None,
            meta: None,
            message_count: self.message_count,
            created_at: self.first_message.clone().unwrap_or_else(|| now.clone()),
            updated_at: self.last_message.clone().unwrap_or_else(|| now.clone()),
        }
    }
}

/// 消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub uuid: String,
    pub session_id: String,
    pub r#type: String,
    pub content: String,
    pub timestamp: Option<String>,
}

impl Message {
    /// 转换时间为本地时区
    pub fn with_local_time(mut self) -> Self {
        self.timestamp = to_local_time(self.timestamp.as_deref());
        self
    }

    /// 转换为 API DTO
    pub fn to_dto(self) -> MessageDto {
        let now = now_local_iso();
        MessageDto {
            id: self.id,
            uuid: self.uuid,
            session_id: self.session_id,
            r#type: self.r#type,
            source: Some("claude".to_string()),
            channel: Some("code".to_string()),
            model: None,
            tool_call_id: None,
            tool_name: None,
            tool_args: None,
            raw: None,
            meta: None,
            content: self.content,
            timestamp: self.timestamp.clone(),
            created_at: self.timestamp.unwrap_or_else(|| now),
        }
    }
}

/// 搜索结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub message_id: i64,
    pub session_id: String,
    pub project_id: i64,
    pub project_name: String,
    pub r#type: String,
    pub content: String,
    pub snippet: String,
    pub score: f64,
    pub timestamp: Option<String>,
}

impl SearchResult {
    /// 转换时间为本地时区
    pub fn with_local_time(mut self) -> Self {
        self.timestamp = to_local_time(self.timestamp.as_deref());
        self
    }
}

// ==================== 从 claude_session_db 类型转换 ====================

/// 毫秒时间戳转 ISO 8601 字符串（本地时区）
pub fn ms_to_local_iso(ms: i64) -> String {
    use chrono::TimeZone;
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string())
        .unwrap_or_else(now_local_iso)
}

impl From<DbProject> for ProjectDto {
    fn from(p: DbProject) -> Self {
        ProjectDto {
            id: p.id,
            name: p.name,
            path: p.path.clone(),
            encoded_dir_name: Some(encode_path(&p.path)),
            source: Some(p.source),
            created_at: ms_to_local_iso(p.created_at),
            updated_at: ms_to_local_iso(p.updated_at),
            session_count: None, // 需要额外查询
            message_count: None, // 需要额外查询
        }
    }
}

impl From<DbProject> for ProjectDetailDto {
    fn from(p: DbProject) -> Self {
        ProjectDetailDto {
            id: p.id,
            name: p.name,
            path: p.path.clone(),
            encoded_dir_name: Some(encode_path(&p.path)),
            source: Some(p.source),
            created_at: ms_to_local_iso(p.created_at),
            updated_at: ms_to_local_iso(p.updated_at),
            session_count: 0, // 需要额外查询
            message_count: 0, // 需要额外查询
        }
    }
}

impl From<DbSession> for SessionDto {
    fn from(s: DbSession) -> Self {
        SessionDto {
            id: s.session_id,
            project_id: s.project_id,
            status: "completed".to_string(),
            source: Some("claude".to_string()),
            channel: s.channel,
            cwd: s.cwd,
            model: s.model,
            meta: s.meta.and_then(|m| serde_json::from_str(&m).ok()),
            message_count: s.message_count,
            created_at: ms_to_local_iso(s.created_at),
            updated_at: ms_to_local_iso(s.updated_at),
        }
    }
}

impl From<DbMessage> for MessageDto {
    fn from(m: DbMessage) -> Self {
        MessageDto {
            id: m.id,
            uuid: m.uuid,
            session_id: m.session_id,
            r#type: m.r#type.to_string(),
            source: m.source,
            channel: m.channel,
            model: m.model,
            tool_call_id: m.tool_call_id,
            tool_name: m.tool_name,
            tool_args: m.tool_args,
            raw: m.raw,
            meta: None,
            content: m.content_text, // 使用 content_text 作为显示内容
            timestamp: Some(ms_to_local_iso(m.timestamp)),
            created_at: ms_to_local_iso(m.timestamp),
        }
    }
}
