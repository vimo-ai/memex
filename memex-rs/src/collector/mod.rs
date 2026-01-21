//! 采集服务 - 使用 claude-session-db::Collector 统一业务逻辑
//!
//! 此模块是对 claude-session-db::Collector 的薄包装，
//! 确保 memex daemon 和 VlaudeKit 使用相同的采集逻辑。

use anyhow::Result;
use std::sync::Arc;

use crate::config::Config;
use crate::shared_adapter::SharedDbAdapter;

// Re-export CollectResult from claude-session-db
pub use claude_session_db::CollectResult;

/// 采集服务（薄包装）
///
/// 内部调用 claude-session-db::Collector，确保业务逻辑统一。
#[derive(Clone)]
pub struct Collector {
    db: Arc<SharedDbAdapter>,
}

impl Collector {
    /// 创建采集服务
    pub fn new(_config: Config, db: Arc<SharedDbAdapter>) -> Self {
        Self { db }
    }

    /// 执行全量采集
    ///
    /// 使用 claude-session-db::Collector 统一的采集逻辑。
    pub fn collect_all(&self) -> Result<CollectResult> {
        self.db.collect_all_sync()
    }

    /// 增量采集（等同于全量，因为内部有时间戳过滤）
    pub fn collect_incremental(&self) -> Result<CollectResult> {
        self.collect_all()
    }

    /// 按路径采集单个会话
    ///
    /// 使用 claude-session-db::Collector 统一的采集逻辑。
    pub fn collect_by_path(&self, path: &str) -> Result<CollectResult> {
        self.db.collect_by_path_sync(path)
    }
}
