//! LLM 抽象层
//!
//! 提供统一的 LLM 能力抽象，支持多 Provider：
//! - Ollama（本地）
//! - OpenAI 兼容（OpenRouter、DeepSeek 等）
//!
//! 两个独立的能力 trait：
//! - `EmbeddingProvider`: 文本向量化
//! - `ChatProvider`: 对话生成

mod chat;
mod core;
mod embedding;
pub mod providers;
mod types;

pub use chat::{ChatProvider, ChatProviderExt};
pub use core::LlmClientCore;
pub use embedding::EmbeddingProvider;
pub use types::{ChatMessage, ChatResponse, ChatRole};

// Re-export providers
pub use providers::{OllamaProvider, OpenAIProvider};
