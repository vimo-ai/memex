//! 文件监听服务 - 实时监听 Claude/Codex 会话文件变化

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use tokio::sync::mpsc;

use crate::collector::Collector;
use crate::config::Config as AppConfig;
use crate::indexer::IndexQueue;

/// 文件监听服务
pub struct FileWatcher {
    config: AppConfig,
    collector: Collector,
    index_queue: Option<IndexQueue>,
}

impl FileWatcher {
    /// 创建文件监听服务
    pub fn new(config: AppConfig, collector: Collector, index_queue: Option<IndexQueue>) -> Self {
        Self { config, collector, index_queue }
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

        // Watch Claude projects directory
        let claude_path = &self.config.claude_projects_path;
        if claude_path.exists() {
            debouncer.watcher().watch(claude_path, RecursiveMode::Recursive)?;
            tracing::info!("👁️ Watching Claude directory: {:?}", claude_path);
        } else {
            tracing::warn!("⚠️ Claude directory not found: {:?}", claude_path);
        }

        // Watch Codex directory
        let codex_path = &self.config.codex_path;
        if codex_path.exists() {
            debouncer.watcher().watch(codex_path, RecursiveMode::Recursive)?;
            tracing::info!("👁️ Watching Codex directory: {:?}", codex_path);
        } else {
            tracing::warn!("⚠️ Codex directory not found: {:?}", codex_path);
        }

        tracing::info!("🔄 File watcher service started");

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
        // 只关心 .jsonl 文件
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("jsonl") {
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
                tracing::error!("❌ Precise collection failed {:?}: {}", path.file_name().unwrap_or_default(), e);
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
        }
    }
}
