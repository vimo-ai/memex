//! LLM 领域模型类型
//!
//! 定义与具体 Provider API 无关的领域模型

use serde::{Deserialize, Serialize};

/// 聊天消息角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    /// 系统消息（设定角色/行为）
    System,
    /// 用户消息
    User,
    /// 助手回复
    Assistant,
}

impl ChatRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
        }
    }
}

/// 聊天消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// 消息角色
    pub role: ChatRole,
    /// 消息内容
    pub content: String,
}

impl ChatMessage {
    /// 创建系统消息
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::System,
            content: content.into(),
        }
    }

    /// 创建用户消息
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
        }
    }

    /// 创建助手消息
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: content.into(),
        }
    }
}

/// 聊天响应
#[derive(Debug, Clone)]
pub struct ChatResponse {
    /// 生成的内容
    pub content: String,
    /// 使用的 token 数（如果 provider 返回）
    pub tokens_used: Option<u64>,
    /// 完成原因（如果 provider 返回）
    pub finish_reason: Option<String>,
}
