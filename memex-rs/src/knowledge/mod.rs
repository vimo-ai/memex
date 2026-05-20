//! L4 Knowledge Pipeline — 知识结晶
//!
//! 从 L0 原始消息提取结构化知识，独立于 compact 加速链路。
//! compact(0.6B): L0 → L1 → L2 → L3
//! knowledge(强模型): L0 → L4

mod config;
mod extractor;
mod matcher;
mod prompt;
mod service;
mod store;

pub use config::KnowledgeConfig;
pub use extractor::Extractor;
pub use matcher::{
    cluster_unmatched, evolve_cluster, match_nodes_to_clusters, name_clusters, EvolutionResult,
};
pub use service::{KnowledgeService, ProcessResult, SessionKnowledge};
pub use store::{KnowledgeStore, KnowledgeNode, KnowledgeCluster, KnowledgeRelation, PendingProject};
