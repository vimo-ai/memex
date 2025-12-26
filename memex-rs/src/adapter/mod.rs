//! Adapter 架构 - 支持多种 CLI 数据源
//!
//! 设计参考 NestJS 版本的 adapter 模式，支持:
//! - Claude Code (JSONL)
//! - Codex CLI (history.jsonl + rollout)
//! - 未来更多 CLI 工具

mod claude;
mod codex;
mod registry;

pub use claude::ClaudeAdapter;
pub use codex::CodexAdapter;
pub use registry::AdapterRegistry;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// 数据来源标识
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Claude,
    Codex,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Claude => write!(f, "claude"),
            Source::Codex => write!(f, "codex"),
        }
    }
}

/// 消息类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageType {
    User,
    Assistant,
    Tool,
}

impl std::fmt::Display for MessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageType::User => write!(f, "user"),
            MessageType::Assistant => write!(f, "assistant"),
            MessageType::Tool => write!(f, "tool"),
        }
    }
}

/// 标准化的会话元数据
#[derive(Debug, Clone, Serialize)]
pub struct SessionMeta {
    /// 会话 ID
    pub id: String,
    /// 数据来源
    pub source: Source,
    /// 渠道 (cli/code/gui)
    pub channel: Option<String>,
    /// 关联项目真实路径
    pub project_path: String,
    /// 项目名称
    pub project_name: Option<String>,
    /// Claude 编码目录名
    pub encoded_dir_name: Option<String>,
    /// 会话文件完整路径
    pub session_path: Option<String>,
    /// 文件修改时间戳 (毫秒)
    pub file_mtime: Option<u64>,
    /// 文件大小
    pub file_size: Option<u64>,
    /// 工作目录
    pub cwd: Option<String>,
    /// 默认模型
    pub model: Option<String>,
    /// 额外元信息
    pub meta: Option<serde_json::Value>,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

/// 标准化的消息
#[derive(Debug, Clone, Serialize)]
pub struct ParsedMessage {
    /// 消息 UUID
    pub uuid: String,
    /// 会话 ID
    pub session_id: String,
    /// 消息类型
    pub message_type: MessageType,
    /// 消息内容
    pub content: String,
    /// 时间戳
    pub timestamp: Option<String>,
    /// 数据来源
    pub source: Source,
    /// 渠道
    pub channel: Option<String>,
    /// 模型
    pub model: Option<String>,
    /// Tool call ID
    pub tool_call_id: Option<String>,
    /// Tool 名称
    pub tool_name: Option<String>,
    /// Tool 参数
    pub tool_args: Option<String>,
    /// 原始数据
    pub raw: Option<String>,
}

/// 适配器解析结果
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub messages: Vec<ParsedMessage>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub meta: Option<serde_json::Value>,
}

/// 会话适配器 trait
pub trait ConversationAdapter: Send + Sync {
    /// 数据来源标识
    fn source(&self) -> Source;

    /// 列出当前来源下的所有会话元数据
    fn list_sessions(&self) -> Result<Vec<SessionMeta>>;

    /// 解析单个会话
    fn parse_session(&self, meta: &SessionMeta) -> Result<Option<ParseResult>>;
}
