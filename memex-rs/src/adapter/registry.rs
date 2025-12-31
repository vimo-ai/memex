//! 适配器注册表 - 管理所有数据源适配器

use std::sync::Arc;

use claude_session_db::{ClaudeAdapter, CodexAdapter, ConversationAdapter};
use crate::config::Config;

/// 适配器注册表
#[derive(Clone)]
pub struct AdapterRegistry {
    adapters: Vec<Arc<dyn ConversationAdapter>>,
}

impl AdapterRegistry {
    /// 从配置创建注册表
    pub fn from_config(config: &Config) -> Self {
        let mut adapters: Vec<Arc<dyn ConversationAdapter>> = Vec::new();

        // Claude Code 适配器
        let claude = ClaudeAdapter::new(config.claude_projects_path.clone());
        adapters.push(Arc::new(claude));

        // Codex CLI 适配器
        let codex = CodexAdapter::new(config.codex_path.clone());
        adapters.push(Arc::new(codex));

        Self { adapters }
    }

    /// 获取所有适配器
    pub fn adapters(&self) -> &[Arc<dyn ConversationAdapter>] {
        &self.adapters
    }
}
