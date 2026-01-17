//! Chat Provider trait
//!
//! 定义对话生成能力

use anyhow::Result;
use async_trait::async_trait;

use super::types::{ChatMessage, ChatResponse};

/// Chat Provider trait
///
/// 提供对话生成能力，用于 RAG 问答、Compact 摘要等场景
#[async_trait]
pub trait ChatProvider: Send + Sync {
    /// Provider 名称
    fn name(&self) -> &str;

    /// 获取当前使用的模型名称
    fn model(&self) -> &str;

    /// 发送对话请求
    ///
    /// # Arguments
    /// * `messages` - 对话消息列表（包含历史消息和当前问题）
    async fn chat(&self, messages: &[ChatMessage]) -> Result<ChatResponse>;

    /// 检查 chat 服务是否可用
    async fn is_available(&self) -> bool;

    // TODO: 后续支持 streaming
    // async fn chat_stream(&self, messages: &[ChatMessage]) -> Result<impl Stream<Item = Result<String>>>;
}

/// ChatProvider 扩展方法
///
/// 提供便捷的单轮对话方法
#[async_trait]
pub trait ChatProviderExt: ChatProvider {
    /// 单轮对话（只发送一条用户消息）
    async fn chat_simple(&self, prompt: &str) -> Result<ChatResponse> {
        let messages = vec![ChatMessage::user(prompt)];
        self.chat(&messages).await
    }

    /// 带系统提示的单轮对话
    async fn chat_with_system(&self, system: &str, prompt: &str) -> Result<ChatResponse> {
        let messages = vec![ChatMessage::system(system), ChatMessage::user(prompt)];
        self.chat(&messages).await
    }
}

// 为所有 ChatProvider 实现扩展方法（包括 trait object）
impl<T: ChatProvider + ?Sized> ChatProviderExt for T {}
