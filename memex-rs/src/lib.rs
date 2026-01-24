//! Memex Library
//!
//! Claude Code 会话历史管理系统 - 可作为库使用
//! 支持多种 CLI 数据源 (Claude Code, Codex CLI)
//!
//! # Agent Client 架构
//!
//! memex-rs 使用 Agent Client 模式：
//! - 文件监听和写入由 vimo-agent 统一负责
//! - memex-rs 通过 Agent Client 订阅事件
//! - 收到 NewMessage 事件后触发 compact 和向量索引
//!
//! ```text
//! vimo-agent ──(NewMessage)──> memex-rs ──> compact/indexer
//! ```

// 核心模块
pub mod compact;
pub mod config;
pub mod domain;
pub mod embedding;
pub mod indexer;
pub mod inject;
pub mod llm;
pub mod rag;
pub mod search;
pub mod vector;

// 只读数据库（替代 SharedDbAdapter）
pub mod db_reader;

// Agent Client（事件驱动）
pub mod agent_client;

// CLI 专用模块
#[cfg(feature = "cli")]
pub mod api;
#[cfg(feature = "cli")]
pub mod archive;
#[cfg(feature = "cli")]
pub mod backup;
#[cfg(feature = "cli")]
pub mod mcp;

// Re-export 常用类型
pub use ai_cli_session_db::ParsedMessage;
pub use config::Config;
pub use db_reader::DbReader;
pub use domain::{Message, Project, SearchResult, Session};
