//! Memex Library
//!
//! Claude Code 会话历史管理系统 - 可作为库使用
//! 支持多种 CLI 数据源 (Claude Code, Codex CLI)

// 核心模块
pub mod adapter;
pub mod collector;
pub mod compact;
pub mod config;
pub mod domain;
pub mod embedding;
pub mod indexer;
pub mod llm;
pub mod rag;
pub mod search;
pub mod vector;

// 共享数据库适配器（ETerm/Vlaude 数据统一层）
pub mod shared_adapter;

// CLI 专用模块
#[cfg(feature = "cli")]
pub mod api;
#[cfg(feature = "cli")]
pub mod archive;
#[cfg(feature = "cli")]
pub mod backup;
#[cfg(feature = "cli")]
pub mod mcp;
#[cfg(feature = "cli")]
pub mod watcher;

// Re-export 常用类型
pub use adapter::ParsedMessage;
pub use config::Config;
pub use domain::{Message, Project, SearchResult, Session};
