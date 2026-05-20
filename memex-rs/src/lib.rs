//! Memex Library
//!
//! Claude Code 会话历史管理系统 - 可作为库使用
//! 支持多种 CLI 数据源 (Claude Code, Codex CLI)
//!
//! # V2 读写分离架构
//!
//! - 文件监听和写入由 vimo-agent 统一负责
//! - memex-rs 通过 HTTP API 被动触发 compact 和向量索引
//! - 不再主动订阅 agent 事件

// 核心模块
pub mod compact;
pub mod config;
pub mod domain;
pub mod embedding;
pub mod indexer;
pub mod inject;
pub mod knowledge;
pub mod llm;
pub mod rag;
pub mod search;
pub mod vector;

// 只读数据库（替代 SharedDbAdapter）
pub mod db_reader;

// CLI 专用模块
#[cfg(feature = "cli")]
pub mod api;
#[cfg(feature = "cli")]
pub mod archive;
#[cfg(feature = "cli")]
pub mod backup;
#[cfg(all(feature = "cli-core", feature = "rust-embed"))]
pub mod embedded;
#[cfg(feature = "cli")]
pub mod mcp;

// Server 模式
#[cfg(any(feature = "server-mode", feature = "cli"))]
pub mod auth;
#[cfg(any(feature = "server-mode", feature = "cli"))]
pub mod server;

// Re-export 常用类型
pub use ai_cli_session_db::ParsedMessage;
pub use config::Config;
pub use db_reader::DbReader;
pub use domain::{Message, Project, SearchResult, Session};
