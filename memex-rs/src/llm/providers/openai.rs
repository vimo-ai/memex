//! OpenAI 兼容 Provider
//!
//! 支持 OpenAI、OpenRouter、DeepSeek 等 OpenAI 兼容 API

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::llm::chat::ChatProvider;
use crate::llm::core::{LlmClientConfig, LlmClientCore};
use crate::llm::embedding::EmbeddingProvider;
use crate::llm::types::{ChatMessage, ChatResponse};

/// OpenAI 兼容 Provider
///
/// 支持所有 OpenAI 兼容 API（OpenRouter、DeepSeek 等）
#[derive(Clone)]
pub struct OpenAIProvider {
    core: LlmClientCore,
    model: String,
    /// 是否支持 embedding（部分 provider 不支持）
    supports_embedding: bool,
}

impl OpenAIProvider {
    /// 创建 OpenAI Provider
    ///
    /// # Arguments
    /// * `base_url` - API 基础 URL（如 https://api.openai.com/v1）
    /// * `api_key` - API Key
    /// * `model` - 模型名称（如 gpt-4o-mini）
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        let config = LlmClientConfig::new(base_url).with_api_key(api_key);
        Self {
            core: LlmClientCore::new(config),
            model: model.to_string(),
            supports_embedding: true,
        }
    }

    /// 创建 OpenRouter Provider
    ///
    /// OpenRouter 不支持 embedding，只支持 chat
    pub fn openrouter(api_key: &str, model: &str) -> Self {
        let config = LlmClientConfig::new("https://openrouter.ai/api/v1").with_api_key(api_key);
        Self {
            core: LlmClientCore::new(config),
            model: model.to_string(),
            supports_embedding: false,
        }
    }

    /// 创建 DeepSeek Provider
    pub fn deepseek(api_key: &str, model: &str) -> Self {
        let config = LlmClientConfig::new("https://api.deepseek.com/v1").with_api_key(api_key);
        Self {
            core: LlmClientCore::new(config),
            model: model.to_string(),
            supports_embedding: false,
        }
    }

    /// 设置是否支持 embedding
    pub fn with_embedding_support(mut self, supports: bool) -> Self {
        self.supports_embedding = supports;
        self
    }
}

// ==================== ChatProvider 实现 ====================

#[async_trait]
impl ChatProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn chat(&self, messages: &[ChatMessage]) -> Result<ChatResponse> {
        let openai_messages: Vec<OpenAIChatMessage> = messages
            .iter()
            .map(|m| OpenAIChatMessage {
                role: m.role.as_str().to_string(),
                content: m.content.clone(),
            })
            .collect();

        let request = OpenAIChatRequest {
            model: self.model.clone(),
            messages: openai_messages,
        };

        let response: OpenAIChatResponse = self
            .core
            .post_json("/chat/completions", &request)
            .await
            .context("OpenAI chat 请求失败")?;

        // choices 为空时返回明确错误，而非静默返回空字符串
        let choice =
            response.choices.into_iter().next().ok_or_else(|| {
                anyhow::anyhow!("OpenAI 返回空 choices，可能由于内容过滤或 API 错误")
            })?;

        // content 可能为 null（如 tool call），此时返回空字符串
        let content = choice.message.content.unwrap_or_default();
        let finish_reason = choice.finish_reason;

        Ok(ChatResponse {
            content,
            tokens_used: response.usage.map(|u| u.total_tokens),
            finish_reason,
        })
    }

    async fn is_available(&self) -> bool {
        // OpenAI API 没有简单的健康检查端点
        // 可以尝试发送一个简单请求或直接返回 true
        true
    }
}

// ==================== EmbeddingProvider 实现 ====================

#[async_trait]
impl EmbeddingProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        if !self.supports_embedding {
            anyhow::bail!("此 Provider 不支持 embedding");
        }

        let request = OpenAIEmbeddingRequest {
            model: self.model.clone(),
            input: text.to_string(),
        };

        let response: OpenAIEmbeddingResponse = self
            .core
            .post_json("/embeddings", &request)
            .await
            .context("OpenAI embedding 请求失败")?;

        response
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| anyhow::anyhow!("Embedding 响应为空"))
    }

    async fn is_available(&self) -> bool {
        self.supports_embedding
    }
}

// ==================== OpenAI API 类型 ====================

/// OpenAI chat 请求
#[derive(Serialize)]
struct OpenAIChatRequest {
    model: String,
    messages: Vec<OpenAIChatMessage>,
}

#[derive(Serialize)]
struct OpenAIChatMessage {
    role: String,
    content: String,
}

/// OpenAI chat 响应
#[derive(Deserialize)]
struct OpenAIChatResponse {
    choices: Vec<OpenAIChatChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Deserialize)]
struct OpenAIChatChoice {
    message: OpenAIChatMessageResponse,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAIChatMessageResponse {
    // content 可能为 null（如 tool call 场景）
    content: Option<String>,
}

#[derive(Deserialize)]
struct OpenAIUsage {
    total_tokens: u64,
}

/// OpenAI embedding 请求
#[derive(Serialize)]
struct OpenAIEmbeddingRequest {
    model: String,
    input: String,
}

/// OpenAI embedding 响应
#[derive(Deserialize)]
struct OpenAIEmbeddingResponse {
    data: Vec<OpenAIEmbeddingData>,
}

#[derive(Deserialize)]
struct OpenAIEmbeddingData {
    embedding: Vec<f32>,
}

impl std::fmt::Debug for OpenAIProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAIProvider")
            .field("core", &self.core)
            .field("model", &self.model)
            .field("supports_embedding", &self.supports_embedding)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试修复: content 为 null 时可以正常反序列化
    #[test]
    fn test_openai_response_with_null_content_succeeds() {
        // 模拟 OpenAI 返回 content 为 null 的响应（tool call 场景）
        let json = r#"{
            "choices": [{
                "message": {
                    "content": null
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"total_tokens": 100}
        }"#;

        // 修复后：content 是 Option<String>，可以处理 null
        let result: Result<OpenAIChatResponse, _> = serde_json::from_str(json);
        assert!(result.is_ok(), "content: null 应该可以正常反序列化");

        let response = result.unwrap();
        let content = response.choices[0].message.content.clone();
        assert_eq!(content, None, "null content 应该解析为 None");
        println!("✓ 修复验证: content 为 null 时正常反序列化为 None");
    }

    /// 测试修复: content 为正常字符串时也能工作
    #[test]
    fn test_openai_response_with_normal_content() {
        let json = r#"{
            "choices": [{
                "message": {
                    "content": "Hello, world!"
                },
                "finish_reason": "stop"
            }],
            "usage": {"total_tokens": 50}
        }"#;

        let response: OpenAIChatResponse = serde_json::from_str(json).unwrap();
        let content = response.choices[0].message.content.clone();
        assert_eq!(content, Some("Hello, world!".to_string()));
        println!("✓ 修复验证: 正常 content 可以正常解析");
    }

    /// 测试修复: choices 为空时反序列化成功但后续处理应返回错误
    #[test]
    fn test_empty_choices_should_be_handled_as_error() {
        let json = r#"{
            "choices": [],
            "usage": {"total_tokens": 0}
        }"#;

        // 反序列化本身应该成功
        let response: OpenAIChatResponse = serde_json::from_str(json).unwrap();
        assert!(response.choices.is_empty());

        // 但业务逻辑应该将其视为错误（在 chat 方法中处理）
        let choice = response.choices.into_iter().next();
        assert!(choice.is_none(), "空 choices 应该返回 None");
        println!("✓ 修复验证: 空 choices 可检测，业务层应返回错误");
    }
}
