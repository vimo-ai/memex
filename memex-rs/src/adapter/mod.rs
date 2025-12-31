//! Adapter 架构 - 支持多种 CLI 数据源
//!
//! 核心解析逻辑已提取到 ai-cli-session-collector crate，
//! 这里只保留 AdapterRegistry 和必要的 re-export。

mod registry;

pub use registry::AdapterRegistry;

// 只 re-export 实际使用的类型
pub use claude_session_db::ParsedMessage;
