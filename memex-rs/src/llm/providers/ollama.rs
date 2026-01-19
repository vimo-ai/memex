//! Ollama Provider
//!
//! 实现 EmbeddingProvider 和 ChatProvider

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use crate::llm::chat::ChatProvider;
use crate::llm::core::{LlmClientConfig, LlmClientCore};
use crate::llm::embedding::EmbeddingProvider;
use crate::llm::types::{ChatMessage, ChatResponse};

/// Ollama Provider
///
/// 同时实现 EmbeddingProvider 和 ChatProvider
#[derive(Clone)]
pub struct OllamaProvider {
    core: LlmClientCore,
    embedding_model: String,
    chat_model: String,
}

impl OllamaProvider {
    /// 创建 Ollama Provider
    pub fn new(base_url: &str, embedding_model: &str, chat_model: &str) -> Self {
        let config = LlmClientConfig::new(base_url);
        Self {
            core: LlmClientCore::new(config),
            embedding_model: embedding_model.to_string(),
            chat_model: chat_model.to_string(),
        }
    }

    /// 检查指定模型是否可用
    async fn check_model(&self, model: &str) -> bool {
        let result: Result<OllamaTagsResponse> = self.core.get("/api/tags").await;
        match result {
            Ok(resp) => resp
                .models
                .iter()
                .any(|m| m.name.starts_with(model)),
            Err(_) => false,
        }
    }

    /// 检查 embedding 模型是否可用
    pub async fn is_embedding_model_available(&self) -> bool {
        self.check_model(&self.embedding_model).await
    }

    /// 检查 chat 模型是否可用
    pub async fn is_chat_model_available(&self) -> bool {
        self.check_model(&self.chat_model).await
    }
}

// ==================== EmbeddingProvider 实现 ====================

#[async_trait]
impl EmbeddingProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn model(&self) -> &str {
        &self.embedding_model
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let request = OllamaEmbeddingRequest {
            model: self.embedding_model.clone(),
            prompt: text.to_string(),
        };

        let response: OllamaEmbeddingResponse = self
            .core
            .post_json("/api/embeddings", &request)
            .await
            .context("Ollama embedding 请求失败")?;

        Ok(response.embedding)
    }

    async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        const CONCURRENCY: usize = 10;

        // 使用 buffered 而非 buffer_unordered，保证结果顺序与输入一致
        let results: Vec<Result<Vec<f32>>> = stream::iter(texts.into_iter())
            .map(|text| async move { self.embed(&text).await })
            .buffered(CONCURRENCY)
            .collect()
            .await;

        results.into_iter().collect()
    }

    async fn is_available(&self) -> bool {
        self.core.health_check("/api/tags").await
    }
}

// ==================== ChatProvider 实现 ====================

#[async_trait]
impl ChatProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn model(&self) -> &str {
        &self.chat_model
    }

    async fn chat(&self, messages: &[ChatMessage]) -> Result<ChatResponse> {
        let ollama_messages: Vec<OllamaChatMessage> = messages
            .iter()
            .map(|m| OllamaChatMessage {
                role: m.role.as_str().to_string(),
                content: m.content.clone(),
            })
            .collect();

        let request = OllamaChatRequest {
            model: self.chat_model.clone(),
            messages: ollama_messages,
            stream: false,
        };

        let response: OllamaChatResponse = self
            .core
            .post_json("/api/chat", &request)
            .await
            .context("Ollama chat 请求失败")?;

        Ok(ChatResponse {
            content: response.message.map(|m| m.content).unwrap_or_default(),
            tokens_used: response.eval_count,
            finish_reason: None,
        })
    }

    async fn is_available(&self) -> bool {
        self.core.health_check("/api/tags").await
    }
}

// ==================== Ollama API 类型 ====================

/// Ollama tags 响应
#[derive(Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelInfo>,
}

#[derive(Deserialize)]
struct OllamaModelInfo {
    name: String,
}

/// Ollama embedding 请求
#[derive(Serialize)]
struct OllamaEmbeddingRequest {
    model: String,
    prompt: String,
}

/// Ollama embedding 响应
#[derive(Deserialize)]
struct OllamaEmbeddingResponse {
    embedding: Vec<f32>,
}

/// Ollama chat 请求
#[derive(Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaChatMessage>,
    stream: bool,
}

#[derive(Serialize)]
struct OllamaChatMessage {
    role: String,
    content: String,
}

/// Ollama chat 响应
#[derive(Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaChatMessageResponse>,
    eval_count: Option<u64>,
}

#[derive(Deserialize)]
struct OllamaChatMessageResponse {
    content: String,
}

impl std::fmt::Debug for OllamaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OllamaProvider")
            .field("core", &self.core)
            .field("embedding_model", &self.embedding_model)
            .field("chat_model", &self.chat_model)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 测试修复: buffered 保证顺序
    ///
    /// 验证使用 buffered 后结果顺序与输入一致
    #[tokio::test]
    async fn test_buffered_preserves_order() {
        // 模拟不同处理时间的任务
        // 第一个任务最慢，最后一个任务最快
        let delays = vec![50u64, 30, 10, 5, 1]; // 毫秒
        let inputs: Vec<usize> = (0..delays.len()).collect();

        // 使用 buffered（当前实现的方式，保序）
        let results: Vec<usize> = stream::iter(inputs.clone().into_iter())
            .map(|i| {
                let delay = delays[i];
                async move {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    i // 返回原始索引
                }
            })
            .buffered(5)
            .collect()
            .await;

        println!("输入顺序:    {:?}", inputs);
        println!("buffered:    {:?}", results);

        // buffered 应该保持顺序
        assert_eq!(
            results, inputs,
            "buffered 应该保持输入顺序"
        );
        println!("✓ 修复验证: buffered 保持输入顺序");
    }
}
