//! Compact 任务队列
//!
//! 异步处理 compact 任务，避免阻塞文件监听
//! 支持 session idle 检测触发 L3 生成

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, RwLock};

use super::config::CompactConfig;
use super::db::CompactDB;
use super::indexer::CompactIndexer;
use super::service::CompactService;
use crate::db_reader::DbReader;
use crate::indexer::VectorIndexer;
use crate::llm::ChatProvider;

/// 默认 idle 超时时间（5 分钟）
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// 默认 idle 检查间隔（1 分钟）
const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Compact 任务
#[derive(Debug, Clone)]
pub enum CompactTask {
    /// 处理会话（L1 + L2）
    ProcessSession { session_id: String },
    /// 生成 L3（session 结束后）
    GenerateL3 { session_id: String },
}

/// Session 活跃状态追踪
#[derive(Clone)]
pub struct SessionTracker {
    activity: Arc<RwLock<HashMap<String, Instant>>>,
    idle_timeout: Duration,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self {
            activity: Arc::new(RwLock::new(HashMap::new())),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }

    /// 更新 session 活跃时间
    pub async fn touch(&self, session_id: &str) {
        let mut activity = self.activity.write().await;
        activity.insert(session_id.to_string(), Instant::now());
    }

    /// 获取并清除 idle sessions
    pub async fn drain_idle(&self) -> Vec<String> {
        let now = Instant::now();
        let mut activity = self.activity.write().await;

        let idle_sessions: Vec<String> = activity
            .iter()
            .filter(|(_, last_active)| now.duration_since(**last_active) > self.idle_timeout)
            .map(|(id, _)| id.clone())
            .collect();

        for id in &idle_sessions {
            activity.remove(id);
        }

        idle_sessions
    }
}

impl Default for SessionTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Compact 任务队列
///
/// 提供异步任务入队，实际处理由外部 worker 完成
#[derive(Clone)]
pub struct CompactQueue {
    sender: mpsc::Sender<CompactTask>,
    tracker: SessionTracker,
}

impl CompactQueue {
    /// 创建 Compact 队列（只创建 channel，不启动 worker）
    pub fn new() -> (Self, mpsc::Receiver<CompactTask>) {
        let (sender, receiver) = mpsc::channel(100);
        let tracker = SessionTracker::new();

        (Self { sender, tracker }, receiver)
    }

    /// 入队：处理会话
    pub async fn enqueue_session(&self, session_id: String) {
        // 更新活跃时间
        self.tracker.touch(&session_id).await;

        if let Err(e) = self
            .sender
            .send(CompactTask::ProcessSession {
                session_id: session_id.clone(),
            })
            .await
        {
            tracing::error!("Compact queue send failed: {}", e);
        }
    }

    /// 入队：生成 L3（手动触发）
    pub async fn enqueue_l3(&self, session_id: String) {
        if let Err(e) = self
            .sender
            .send(CompactTask::GenerateL3 { session_id })
            .await
        {
            tracing::error!("Compact queue send failed: {}", e);
        }
    }

    /// 获取 tracker（用于 idle 检测）
    pub fn tracker(&self) -> &SessionTracker {
        &self.tracker
    }
}

impl Default for CompactQueue {
    fn default() -> Self {
        Self::new().0
    }
}

/// Compact Worker - 处理任务的 worker
pub struct CompactWorker {
    service: CompactService,
    compact_db: Arc<CompactDB>,
    tracker: SessionTracker,
    needs_l3: bool,
    /// 向量索引器（compact 完成后自动触发 L0 向量索引）
    vector_indexer: Option<VectorIndexer>,
}

impl CompactWorker {
    /// 创建 worker
    pub fn new(
        db: Arc<DbReader>,
        chat_provider: Arc<dyn ChatProvider>,
        compact_db: Arc<CompactDB>,
        config: CompactConfig,
        tracker: SessionTracker,
        indexer: Option<CompactIndexer>,
        vector_indexer: Option<VectorIndexer>,
    ) -> Self {
        let needs_l3 = config.l3_session_summary;
        let mut service = CompactService::new(db, chat_provider, compact_db.clone(), config);

        // 设置向量索引器（如果提供）
        if let Some(idx) = indexer {
            service = service.with_indexer(idx);
        }

        Self {
            service,
            compact_db,
            tracker,
            needs_l3,
            vector_indexer,
        }
    }

    /// 处理单个任务
    pub async fn handle_task(&self, task: CompactTask) {
        match task {
            CompactTask::ProcessSession { session_id } => {
                self.handle_process_session(&session_id).await;
            }
            CompactTask::GenerateL3 { session_id } => {
                self.handle_generate_l3(&session_id).await;
            }
        }
    }

    /// 检查并处理 idle sessions
    ///
    /// idle 超时触发：
    /// 1. 先执行 L1 + L2（当前 Talk 的 compact）
    /// 2. 再生成 L3（如果启用）
    pub async fn check_idle_sessions(&self) {
        let idle_sessions = self.tracker.drain_idle().await;
        for session_id in idle_sessions {
            tracing::debug!("Session idle detected: {}", session_id);

            // 1. 先触发 L1 + L2
            self.handle_process_session(&session_id).await;

            // 2. 再生成 L3（如果启用）
            if self.needs_l3 {
                self.handle_generate_l3(&session_id).await;
            }
        }
    }

    /// 处理会话（L1 + L2）- 统一触发入口
    ///
    /// 使用锁机制避免重复执行：
    /// 1. try_lock → 失败则跳过
    /// 2. 执行 compact（内部检查是否有新内容）
    /// 3. unlock（调用者负责）
    async fn handle_process_session(&self, session_id: &str) {
        // 1. 尝试获取锁
        match self.compact_db.try_lock(session_id).await {
            Ok(true) => {
                // 获取锁成功，继续处理
            }
            Ok(false) => {
                // 已有其他任务在处理，跳过
                tracing::debug!("Compact skipped (task already in progress): {}", session_id);
                return;
            }
            Err(e) => {
                tracing::error!(
                    "Compact failed to acquire lock (session={}): {}",
                    session_id,
                    e
                );
                return;
            }
        }

        // 2. 执行 compact（内部会检查是否有新内容）
        let result = self.service.process_session(session_id).await;

        // 3. 释放锁（无论成功失败）
        if let Err(unlock_err) = self.compact_db.unlock(session_id).await {
            tracing::error!(
                "Compact 释放锁失败 (session={}): {}",
                session_id,
                unlock_err
            );
        }

        // 4. 处理结果
        let compact_had_content = match result {
            Ok(result) => {
                let had_content = result.observations_count > 0 || result.talk_summaries_count > 0;
                if had_content {
                    tracing::debug!(
                        "📝 Compact: session={} L1={} L2={} (pruned={}, merged={})",
                        session_id,
                        result.observations_count,
                        result.talk_summaries_count,
                        result.tool_calls_pruned,
                        result.tool_calls_merged,
                    );
                }
                had_content
            }
            Err(e) => {
                tracing::error!("Compact processing failed (session={}): {}", session_id, e);
                false
            }
        };

        // 5. Compact 有新内容时，顺带触发向量索引（处理新入库的未索引消息）
        if compact_had_content {
            if let Some(ref indexer) = self.vector_indexer {
                match indexer.index_batch(200).await {
                    Ok(result) if result.indexed_messages > 0 => {
                        tracing::debug!(
                            "🔍 Post-compact indexing: {} messages, {} chunks",
                            result.indexed_messages,
                            result.indexed_chunks,
                        );
                    }
                    Err(e) => {
                        tracing::warn!("Post-compact indexing failed: {}", e);
                    }
                    _ => {}
                }
            }
        }
    }

    /// 生成 L3 Session Summary
    async fn handle_generate_l3(&self, session_id: &str) {
        match self.service.generate_l3_session_summary(session_id).await {
            Ok(true) => {
                tracing::info!("📋 L3 Session Summary generated: {}", session_id);
            }
            Ok(false) => {
                tracing::debug!("L3 skipped (no L2 data or disabled): {}", session_id);
            }
            Err(e) => {
                tracing::error!("L3 generation failed (session={}): {}", session_id, e);
            }
        }
    }
}

/// 启动 compact worker 循环
pub async fn run_compact_worker(worker: CompactWorker, mut receiver: mpsc::Receiver<CompactTask>) {
    tracing::info!("🔄 Compact worker started");

    let mut idle_check_interval = tokio::time::interval(DEFAULT_CHECK_INTERVAL);

    loop {
        tokio::select! {
            // 处理任务队列
            task = receiver.recv() => {
                match task {
                    Some(task) => {
                        worker.handle_task(task).await;
                    }
                    None => {
                        tracing::info!("Compact worker stopping (channel closed)");
                        break;
                    }
                }
            }
            // 定时检查 idle sessions
            _ = idle_check_interval.tick() => {
                worker.check_idle_sessions().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idle_timeout_default() {
        assert_eq!(DEFAULT_IDLE_TIMEOUT, Duration::from_secs(300));
    }

    #[tokio::test]
    async fn test_session_tracker() {
        let tracker = SessionTracker::new();

        // Touch a session
        tracker.touch("session-1").await;

        // Should not be idle immediately
        let idle = tracker.drain_idle().await;
        assert!(idle.is_empty());
    }
}
