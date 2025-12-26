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

        // 监听 Claude 项目目录
        let claude_path = &self.config.claude_projects_path;
        if claude_path.exists() {
            debouncer.watcher().watch(claude_path, RecursiveMode::Recursive)?;
            tracing::info!("👁️ 监听 Claude 目录: {:?}", claude_path);
        } else {
            tracing::warn!("⚠️ Claude 目录不存在: {:?}", claude_path);
        }

        // 监听 Codex 目录
        let codex_path = &self.config.codex_path;
        if codex_path.exists() {
            debouncer.watcher().watch(codex_path, RecursiveMode::Recursive)?;
            tracing::info!("👁️ 监听 Codex 目录: {:?}", codex_path);
        } else {
            tracing::warn!("⚠️ Codex 目录不存在: {:?}", codex_path);
        }

        tracing::info!("🔄 文件监听服务已启动");

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
                tracing::debug!("📝 检测到文件变化: {:?}", path);
                self.trigger_collect(path).await;
            }
            _ => {}
        }
    }

    /// 触发采集
    async fn trigger_collect(&self, path: &PathBuf) {
        // 提取 session ID（文件名去掉扩展名）
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        if session_id.is_empty() {
            return;
        }

        tracing::info!("🔄 触发采集: session={}", session_id);

        // 执行全量采集（简单起见，后续可以优化为单会话采集）
        match self.collector.collect_all() {
            Ok(result) => {
                if result.messages_inserted > 0 {
                    tracing::info!(
                        "✅ 实时采集完成: {} 会话, {} 新消息",
                        result.sessions_scanned,
                        result.messages_inserted
                    );

                    // 异步触发向量索引
                    if let Some(queue) = &self.index_queue {
                        queue.enqueue(result.new_message_ids).await;
                    }
                }
            }
            Err(e) => {
                tracing::error!("❌ 实时采集失败: {}", e);
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
