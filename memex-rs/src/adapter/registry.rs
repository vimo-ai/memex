//! 适配器注册表 - 管理所有数据源适配器
//!
//! 使用 ai-cli-session-collector 的自注册机制：
//! - 适配器自描述配置（路径、扩展名、环境变量）
//! - 通过 `all_adapters()` 获取所有适配器

use std::sync::Arc;

use claude_session_db::{all_adapters, ConversationAdapter};

/// 适配器注册表
#[derive(Clone)]
pub struct AdapterRegistry {
    adapters: Vec<Arc<dyn ConversationAdapter>>,
}

impl AdapterRegistry {
    /// 创建注册表（使用适配器自注册机制）
    ///
    /// 每个适配器自行处理：
    /// - 默认路径
    /// - 环境变量覆盖
    /// - 文件扩展名
    pub fn new() -> Self {
        Self {
            adapters: all_adapters(),
        }
    }

    /// 获取所有适配器
    pub fn adapters(&self) -> &[Arc<dyn ConversationAdapter>] {
        &self.adapters
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}
