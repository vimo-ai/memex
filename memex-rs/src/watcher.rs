//! 文件监听服务 - 实时监听 AI CLI 会话文件变化
//!
//! 使用 ai-cli-session-collector 的自注册机制：
//! - `all_watch_configs()` 获取所有适配器的监听配置
//! - 每个适配器定义自己的路径、扩展名、递归模式
//!
//! Compact 触发时机：
//! - 检测到新 user prompt → 触发上一个 Talk 的 compact
//! - idle 超时 → 由 CompactQueue 处理

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use tokio::sync::mpsc;

use ai_cli_session_db::all_watch_configs;

use crate::collector::Collector;
use crate::compact::CompactQueue;
use crate::config::Config as AppConfig;
use crate::indexer::IndexQueue;
use crate::shared_adapter::SharedDbAdapter;

/// 文件监听服务
pub struct FileWatcher {
    config: AppConfig,
    collector: Collector,
    index_queue: Option<IndexQueue>,
    /// Compact 任务队列（可选）
    compact_queue: Option<CompactQueue>,
    /// 共享数据库（用于检测 Talk 边界）
    shared_db: Option<Arc<SharedDbAdapter>>,
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
            compact_queue: None,
            shared_db: None,
            supported_extensions,
        }
    }

    /// 设置 Compact 队列和共享数据库
    pub fn with_compact(mut self, queue: CompactQueue, shared_db: Arc<SharedDbAdapter>) -> Self {
        self.compact_queue = Some(queue);
        self.shared_db = Some(shared_db);
        self
    }

    /// 设置 Compact 队列（兼容旧接口）
    #[deprecated(note = "Use with_compact instead")]
    pub fn with_compact_queue(mut self, queue: CompactQueue) -> Self {
        self.compact_queue = Some(queue);
        self
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

        if kind == &DebouncedEventKind::Any {
            tracing::debug!("📝 File change detected: {:?}", path);
            self.trigger_collect(path).await;
        }
    }

    /// Trigger collection (precise indexing of single file, not full scan)
    ///
    /// Talk 边界检测：
    /// - 执行 collection 后，更新 SessionTracker 活跃时间
    /// - 检测最新消息是否为 user → 触发 compact
    async fn trigger_collect(&self, path: &PathBuf) {
        // Convert path to string
        let path_str = match path.to_str() {
            Some(s) => s,
            None => {
                tracing::warn!("⚠️ Cannot convert path: {:?}", path);
                return;
            }
        };

        // 提取 session_id（文件名，不含扩展名）
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());

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

                    // Talk 边界检测 + Compact 触发
                    if let Some(session_id) = session_id {
                        self.handle_compact_trigger(&session_id).await;
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

    /// 处理 Compact 触发逻辑
    ///
    /// 1. 更新 SessionTracker 活跃时间（用于 idle 检测）
    /// 2. 检测是否有新 user prompt → 触发 compact
    async fn handle_compact_trigger(&self, session_id: &str) {
        let queue = match &self.compact_queue {
            Some(q) => q,
            None => return,
        };

        // 1. 更新活跃时间（idle 检测用）
        queue.tracker().touch(session_id).await;

        // 2. 检测 Talk 边界：最新消息是否为 user
        let should_compact = match &self.shared_db {
            Some(db) => self.detect_new_user_prompt(db, session_id).await,
            None => {
                // 没有 shared_db，降级为每次都触发（旧行为）
                true
            }
        };

        // 3. 触发 compact
        if should_compact {
            tracing::debug!("🎯 New user prompt detected, triggering compact: {}", session_id);
            queue.enqueue_session(session_id.to_string()).await;
        }
    }

    /// 检测是否有新 user prompt（Talk 边界）
    ///
    /// 规则：如果最新消息的 type 是 User，说明新的 Talk 开始了，
    /// 应该触发上一个 Talk 的 compact。
    async fn detect_new_user_prompt(&self, db: &SharedDbAdapter, session_id: &str) -> bool {
        use ai_cli_session_db::MessageType;

        // 获取最新 1 条消息
        match db.get_messages_with_options(session_id, Some(1), true).await {
            Ok(messages) => {
                if let Some(msg) = messages.first() {
                    // type 是 User 表示新 Talk 开始
                    msg.r#type == MessageType::User
                } else {
                    false
                }
            }
            Err(e) => {
                tracing::warn!("检测 Talk 边界失败 (session={}): {}", session_id, e);
                // 出错时降级为触发
                true
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
            compact_queue: self.compact_queue.clone(),
            shared_db: self.shared_db.clone(),
            supported_extensions: self.supported_extensions.clone(),
        }
    }
}
