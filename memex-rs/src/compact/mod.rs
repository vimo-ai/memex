//! Compact 层 - LLM 驱动的会话压缩/摘要
//!
//! 多层可选压缩架构：
//! - L0: 原文（messages 表）- 始终保留
//! - L1: Observations - 每个工具调用一个
//! - L2: Talk Summary - 每轮对话一个
//! - L3: Session Summary - 整个会话一个（依赖 L2）
//!
//! 核心差异化：原文始终保留，压缩层可选、可重做

mod config;
mod db;
mod indexer;
mod prompt;
mod queue;
mod service;
mod source;
mod vector;

pub use config::CompactConfig;
pub use db::{CompactDB, Observation, ObservationType, SessionSummary, TalkSummary};
pub use indexer::CompactIndexer;
pub use prompt::{L1Output, L1Prompt, L2Output, L2Prompt, L3Output, L3Prompt};
pub use queue::{run_compact_worker, CompactQueue, CompactTask, CompactWorker, SessionTracker};
pub use service::{CompactResult, CompactService};
pub use source::{
    build_parsed_session, Message as SourceMessage, MessageRole, ParsedSession, Talk, ToolCall,
    ToolCategory,
};
pub use vector::{CompactLevel, CompactVectorRecord, CompactVectorSearchResult, CompactVectorStore};
