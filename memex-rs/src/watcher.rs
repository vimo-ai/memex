//! 文件监听服务 - 实时监听 AI CLI 会话文件变化
//!
//! 使用 ai-cli-session-collector 的自注册机制：
//! - `all_watch_configs()` 获取所有适配器的监听配置
//! - 每个适配器定义自己的路径、扩展名、递归模式

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use tokio::sync::mpsc;

use claude_session_db::all_watch_configs;

use crate::collector::Collector;
use crate::config::Config as AppConfig;
use crate::indexer::IndexQueue;

/// 文件监听服务
pub struct FileWatcher {
    config: AppConfig,
    collector: Collector,
    index_queue: Option<IndexQueue>,
    /// 支持的文件扩展名（从适配器收集）
    supported_extensions: HashSet<String>,
}

impl FileWatcher {
    /// 创建文件监听服务
    pub fn new(config: AppConfig, collector: Collector, index_queue: Option<IndexQueue>) -> Self {
        // 从适配器收集所有支持的扩展名
        let supported_extensions: HashSet<String> = all_watch_configs()
            .iter()
            .flat_map(|c| c.extensions.iter().map(|e| e.to_string()))
            .collect();

        Self {
            config,
            collector,
            index_queue,
            supported_extensions,
        }
    }

    /// 启动监听（异步）
    pub async fn start(self: Arc<Self>) -> anyhow::Result<()> {
        let (tx, mut rx) = mpsc::channel(100);

        // 创建 debouncer（2秒防抖）
        let mut debouncer = new_debouncer(Duration::from_secs(2), move |res| {
            if let Ok(events) = res {
                for event in events {
                    let _ = tx.blocking_send(event);
                }
            }
        })?;

        // 使用适配器自注册的监听配置
        let watch_configs = all_watch_configs();

        for config in &watch_configs {
            let recursive_mode = if config.recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };

            match debouncer.watcher().watch(&config.path, recursive_mode) {
                Ok(_) => {
                    tracing::info!(
                        "👁️ Watching {} directory: {:?} (extensions: {:?})",
                        config.name,
                        config.path,
                        config.extensions
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "⚠️ Failed to watch {} directory {:?}: {}",
                        config.name,
                        config.path,
                        e
                    );
                }
            }
        }

        if watch_configs.is_empty() {
            tracing::warn!("⚠️ No valid watch directories found");
        }

        tracing::info!(
            "🔄 File watcher service started ({} directories)",
            watch_configs.len()
        );

        // 处理文件变化事件
        let watcher = self.clone();
        tokio::spawn(async move {
            // 保持 debouncer 存活
            let _debouncer = debouncer;

            while let Some(event) = rx.recv().await {
                watcher.handle_event(&event.path, &event.kind).await;
            }
        });

        Ok(())
    }

    /// 处理文件变化事件
    async fn handle_event(&self, path: &PathBuf, kind: &DebouncedEventKind) {
        // 检查扩展名是否被任意适配器支持
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e,
            None => return,
        };

        if !self.supported_extensions.contains(ext) {
            return;
        }

        match kind {
            DebouncedEventKind::Any => {
                tracing::debug!("📝 File change detected: {:?}", path);
                self.trigger_collect(path).await;
            }
            _ => {}
        }
    }

    /// Trigger collection (precise indexing of single file, not full scan)
    async fn trigger_collect(&self, path: &PathBuf) {
        // Convert path to string
        let path_str = match path.to_str() {
            Some(s) => s,
            None => {
                tracing::warn!("⚠️ Cannot convert path: {:?}", path);
                return;
            }
        };

        // Precise single file collection (efficient! no longer scanning 9000+ files)
        match self.collector.collect_by_path(path_str) {
            Ok(result) => {
                // Async trigger vector indexing
                if result.messages_inserted > 0 {
                    tracing::debug!(
                        "📝 File change: {:?} → {} new messages",
                        path.file_name().unwrap_or_default(),
                        result.messages_inserted
                    );
                    if let Some(queue) = &self.index_queue {
                        queue.enqueue(result.new_message_ids).await;
                    }
                }
            }
            Err(e) => {
                tracing::error!(
                    "❌ Precise collection failed {:?}: {}",
                    path.file_name().unwrap_or_default(),
                    e
                );
            }
        }
    }
}

impl Clone for FileWatcher {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            collector: self.collector.clone(),
            index_queue: self.index_queue.clone(),
            supported_extensions: self.supported_extensions.clone(),
        }
    }
}
