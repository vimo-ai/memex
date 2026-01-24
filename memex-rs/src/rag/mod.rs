//! RAG (Retrieval Augmented Generation) 服务
//!
//! 基于历史对话提供问答能力：
//! 1. 使用混合检索获取相关消息
//! 2. 构建上下文 prompt
//! 3. 调用 LLM chat API 生成答案

use std::sync::Arc;
use tokio::sync::RwLock;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::db_reader::DbReader;
use crate::llm::EmbeddingProvider;
use crate::llm::{ChatProvider, ChatProviderExt};
use crate::search::{
    HybridSearchOptions, HybridSearchService, SearchLevel, SearchMode, SearchOrderBy,
};
use crate::vector::VectorStore;

/// RAG 响应结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagResponse {
    /// 生成的答案
    pub answer: String,
    /// 引用的来源
    pub sources: Vec<RagSource>,
    /// 使用的模型
    pub model: String,
    /// 消耗的 token 数（如果可用）
    pub tokens_used: Option<u64>,
}

/// RAG 来源信息
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagSource {
    /// 会话 ID
    pub session_id: String,
    /// 项目名称
    pub project_name: String,
    /// 消息索引
    pub message_index: usize,
    /// 匹配片段
    pub snippet: String,
    /// 得分
    pub score: f64,
}

/// RAG 查询选项
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagOptions {
    /// 用户问题
    pub question: String,
    /// 前后消息数，用于提供上下文，默认 3
    #[serde(default = "default_context_window")]
    pub context_window: usize,
    /// 最大引用数，默认 5
    #[serde(default = "default_max_sources")]
    pub max_sources: usize,
    /// 项目 ID 过滤
    pub project_id: Option<i64>,
}

fn default_context_window() -> usize {
    3
}

fn default_max_sources() -> usize {
    5
}

/// 带上下文的来源信息（内部使用）
struct SourceWithContext {
    session_id: String,
    project_name: String,
    message_index: usize,
    snippet: String,
    score: f64,
    context_messages: Vec<String>,
}

/// RAG 服务
pub struct RagService {
    db: Arc<DbReader>,
    chat: Option<Arc<dyn ChatProvider>>,
    hybrid_search: HybridSearchService,
}

impl RagService {
    /// 创建 RAG 服务
    pub fn new(
        db: Arc<DbReader>,
        chat: Option<Arc<dyn ChatProvider>>,
        embedding: Option<Arc<dyn EmbeddingProvider>>,
        vector: Option<Arc<RwLock<VectorStore>>>,
    ) -> Self {
        let hybrid_search = HybridSearchService::new(db.clone(), embedding, vector);

        Self {
            db,
            chat,
            hybrid_search,
        }
    }

    /// 基于历史对话回答问题
    pub async fn ask(&self, options: RagOptions) -> Result<RagResponse> {
        let RagOptions {
            question,
            context_window,
            max_sources,
            project_id,
        } = options;

        tracing::info!("[RAG] Received question: \"{}\"", question);

        // 1. 使用混合检索获取相关消息
        let search_options = HybridSearchOptions {
            query: question.clone(),
            limit: max_sources,
            project_id,
            mode: SearchMode::Hybrid,
            level: SearchLevel::Raw, // RAG 默认搜索原文
            order_by: SearchOrderBy::Score,
            start_date: None,
            end_date: None,
        };

        let search_results = self.hybrid_search.search(search_options).await?;

        let chat = match &self.chat {
            Some(c) => c,
            None => {
                return Ok(RagResponse {
                    answer: "Chat service unavailable, cannot generate answer.".to_string(),
                    sources: vec![],
                    model: "none".to_string(),
                    tokens_used: None,
                });
            }
        };

        if search_results.is_empty() {
            return Ok(RagResponse {
                answer: "Sorry, I couldn't find relevant information in the conversation history."
                    .to_string(),
                sources: vec![],
                model: chat.model().to_string(),
                tokens_used: None,
            });
        }

        tracing::info!("[RAG] Retrieved {} relevant messages", search_results.len());

        // 2. 为每条消息构建上下文（拉取前后消息）
        let mut sources_with_context = Vec::new();

        for result in &search_results {
            // 跳过没有 message_id 的结果（如 Compact 层结果）
            let Some(message_id) = result.message_id else {
                continue;
            };

            let context = self
                .build_message_context(&result.session_id, message_id, context_window)
                .await?;

            sources_with_context.push(SourceWithContext {
                session_id: result.session_id.clone(),
                project_name: result.project_name.clone(),
                message_index: context.message_index,
                snippet: result
                    .snippet
                    .clone()
                    .unwrap_or_else(|| result.content.chars().take(200).collect()),
                score: result.score,
                context_messages: context.messages,
            });
        }

        // 3. 构建 prompt
        let prompt = self.build_prompt(&question, &sources_with_context);

        // 4. 调用 chat API
        let (answer, tokens_used) = match chat.chat_simple(&prompt).await {
            Ok(result) => (result.content, result.tokens_used),
            Err(e) => {
                tracing::error!("[RAG] Chat call failed: {}", e);
                let error_msg = format!("Sorry, error generating answer: {}", e);
                return Ok(RagResponse {
                    answer: error_msg,
                    sources: sources_with_context
                        .into_iter()
                        .map(|s| RagSource {
                            session_id: s.session_id,
                            project_name: s.project_name,
                            message_index: s.message_index,
                            snippet: s.snippet,
                            score: s.score,
                        })
                        .collect(),
                    model: chat.model().to_string(),
                    tokens_used: None,
                });
            }
        };

        // 5. 返回结果
        Ok(RagResponse {
            answer,
            sources: sources_with_context
                .into_iter()
                .map(|s| RagSource {
                    session_id: s.session_id,
                    project_name: s.project_name,
                    message_index: s.message_index,
                    snippet: s.snippet,
                    score: s.score,
                })
                .collect(),
            model: chat.model().to_string(),
            tokens_used,
        })
    }

    /// 构建消息上下文
    async fn build_message_context(
        &self,
        session_id: &str,
        message_id: i64,
        context_window: usize,
    ) -> Result<MessageContext> {
        let all_messages = self.db.get_messages(session_id).await?;

        // 查找当前消息的索引
        let message_index = all_messages
            .iter()
            .position(|m| m.id == message_id)
            .unwrap_or(0);

        // 计算上下文窗口范围
        let start_idx = message_index.saturating_sub(context_window);
        let end_idx = (message_index + context_window + 1).min(all_messages.len());

        // 提取上下文消息
        let context_messages: Vec<String> = all_messages[start_idx..end_idx]
            .iter()
            .map(|msg| {
                let content_preview: String = msg.content_text.chars().take(500).collect();
                format!("[{}] {}", msg.r#type, content_preview)
            })
            .collect();

        Ok(MessageContext {
            message_index,
            messages: context_messages,
        })
    }

    /// 构建 prompt
    fn build_prompt(&self, question: &str, sources: &[SourceWithContext]) -> String {
        let sources_text: String = sources
            .iter()
            .enumerate()
            .map(|(idx, source)| {
                let context = source.context_messages.join("\n");
                format!(
                    "---\n[Source {}: Project {}, Session {}...]\n{}\n---",
                    idx + 1,
                    source.project_name,
                    &source.session_id[..8.min(source.session_id.len())],
                    context
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        format!(
            r#"You are a knowledge assistant that answers questions based on the user's Claude Code conversation history.

Here are relevant conversation snippets:

{}

Please answer the user's question based on the history above. If there is no relevant information in the history, please state so clearly.

User question: {}"#,
            sources_text, question
        )
    }

    /// 检查 RAG 是否可用
    pub fn is_available(&self) -> bool {
        self.chat.is_some()
    }

    /// 获取 chat 模型名称
    pub fn chat_model(&self) -> &str {
        self.chat.as_ref().map(|c| c.model()).unwrap_or("none")
    }
}

/// 消息上下文
struct MessageContext {
    message_index: usize,
    messages: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_options() {
        assert_eq!(default_context_window(), 3);
        assert_eq!(default_max_sources(), 5);
    }
}
