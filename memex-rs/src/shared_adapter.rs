//! 共享数据库适配器
//!
//! 集成 claude-session-db，实现 Writer 协调和数据共享

use anyhow::Result;
use claude_session_db::{
    coordination::{Role, WriterHealth, WriterType},
    db::{MessageInput, SessionInput},
    DbConfig, Message, Project, SearchResult, Session, SessionDB,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// 共享数据库适配器
///
/// 封装 claude-session-db，提供：
/// - Writer 协调（多组件共存）
/// - 心跳维护
/// - 角色切换回调
pub struct SharedDbAdapter {
    /// 共享数据库连接
    db: Arc<RwLock<SessionDB>>,
    /// 当前角色
    role: Arc<RwLock<Role>>,
    /// 心跳任务取消标志
    heartbeat_cancel: Arc<RwLock<bool>>,
    /// 心跳任务句柄（用于清理）
    heartbeat_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
}

impl SharedDbAdapter {
    /// 创建适配器（连接到共享数据库）
    ///
    /// # Arguments
    /// - `shared_db_path`: 共享数据库路径（默认 ~/.vimo/db/ai-cli-session.db）
    pub fn new(shared_db_path: Option<PathBuf>) -> Result<Self> {
        let db_path = shared_db_path.unwrap_or_else(|| {
            // 支持 VIMO_HOME 环境变量覆盖
            let vimo_root = std::env::var("VIMO_HOME").unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{}/.vimo", home)
            });
            PathBuf::from(format!("{}/db/ai-cli-session.db", vimo_root))
        });

        // 确保目录存在
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        info!("[SharedDB] 连接共享数据库: {:?}", db_path);
        let config = DbConfig::local(db_path.to_string_lossy().into_owned());
        let db = SessionDB::connect(config)?;

        Ok(Self {
            db: Arc::new(RwLock::new(db)),
            role: Arc::new(RwLock::new(Role::Reader)),
            heartbeat_cancel: Arc::new(RwLock::new(false)),
            heartbeat_handle: Arc::new(RwLock::new(None)),
        })
    }

    /// 注册为 Writer（启动时调用）
    ///
    /// 成为 Writer 时会自动触发全量采集，统一所有组件的采集逻辑。
    pub async fn register(&self) -> Result<Role> {
        // 重置取消标志
        *self.heartbeat_cancel.write().await = false;

        let mut db = self.db.write().await;
        let (role, collect_result) = db.register_writer_and_collect(WriterType::MemexDaemon)?;
        drop(db);

        *self.role.write().await = role;

        if role == Role::Writer {
            if let Some(result) = collect_result {
                info!(
                    "[SharedDB] 成为 Writer，采集完成: {} sessions, {} 条新消息",
                    result.sessions_scanned, result.messages_inserted
                );
            } else {
                info!("[SharedDB] 成为 Writer，启动协调任务");
            }
        } else {
            info!("[SharedDB] 成为 Reader（已有其他 Writer），启动协调任务");
        }

        // Writer 或 Reader 都启动协调任务：
        // - Writer: 发送心跳
        // - Reader: 检查 Writer 是否超时，尝试接管
        self.start_heartbeat().await;

        Ok(role)
    }

    /// 启动协调任务（Writer 心跳 / Reader 健康检查）
    async fn start_heartbeat(&self) {
        // 先停止已有的任务
        self.stop_heartbeat().await;

        let db = self.db.clone();
        let role = self.role.clone();
        let cancel = self.heartbeat_cancel.clone();

        let handle = tokio::spawn(async move {
            let interval = tokio::time::Duration::from_secs(10); // 10s

            loop {
                tokio::time::sleep(interval).await;

                if *cancel.read().await {
                    debug!("[SharedDB] 协调任务收到取消信号");
                    break;
                }

                let current_role = *role.read().await;

                if current_role == Role::Writer {
                    // Writer: 发送心跳
                    let db_guard = db.write().await;
                    match db_guard.heartbeat() {
                        Ok(()) => {
                            debug!("[SharedDB] 心跳成功");
                        }
                        Err(e) => {
                            warn!("[SharedDB] 心跳失败（可能被抢占）: {}", e);
                            drop(db_guard);
                            *role.write().await = Role::Reader;
                            // 不退出循环，继续作为 Reader 检查
                        }
                    }
                } else {
                    // Reader: 检查 Writer 是否超时，尝试接管
                    let db_guard = db.read().await;
                    match db_guard.check_writer_health() {
                        Ok(health) => {
                            drop(db_guard);
                            if matches!(health, WriterHealth::Timeout | WriterHealth::Released) {
                                info!("[SharedDB] Writer {:?}，尝试接管...", health);
                                let mut db_guard = db.write().await;
                                match db_guard.try_takeover() {
                                    Ok(true) => {
                                        info!("[SharedDB] 接管成功，现在是 Writer");
                                        *role.write().await = Role::Writer;
                                    }
                                    Ok(false) => {
                                        debug!("[SharedDB] 接管失败，其他组件已抢先");
                                    }
                                    Err(e) => {
                                        warn!("[SharedDB] 接管出错: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("[SharedDB] 检查 Writer 健康状态失败: {}", e);
                        }
                    }
                }
            }
        });

        *self.heartbeat_handle.write().await = Some(handle);
    }

    /// 停止心跳任务
    async fn stop_heartbeat(&self) {
        *self.heartbeat_cancel.write().await = true;

        if let Some(handle) = self.heartbeat_handle.write().await.take() {
            handle.abort();
        }
    }

    /// 释放 Writer（退出时调用）
    pub async fn release(&self) -> Result<()> {
        self.stop_heartbeat().await;

        // 使用 write 锁，因为 release_writer 会修改状态
        let db = self.db.write().await;
        db.release_writer()?;
        drop(db);

        *self.role.write().await = Role::Reader;
        info!("[SharedDB] 已释放 Writer");
        Ok(())
    }

    /// 获取当前角色
    pub async fn role(&self) -> Role {
        *self.role.read().await
    }

    /// 是否为 Writer
    pub async fn is_writer(&self) -> bool {
        *self.role.read().await == Role::Writer
    }

    /// 检查 Writer 健康状态（Reader 调用，用于检测是否需要接管）
    pub async fn check_writer_health(&self) -> Result<WriterHealth> {
        let db = self.db.read().await;
        Ok(db.check_writer_health()?)
    }

    /// 尝试接管（Reader 在检测到超时后调用）
    ///
    /// 接管成功时会自动触发全量采集，统一所有组件的采集逻辑。
    pub async fn try_takeover(&self) -> Result<bool> {
        // 重置取消标志
        *self.heartbeat_cancel.write().await = false;

        let mut db = self.db.write().await;
        let (taken, collect_result) = db.try_takeover_and_collect()?;

        if taken {
            drop(db);
            *self.role.write().await = Role::Writer;
            self.start_heartbeat().await;

            if let Some(result) = collect_result {
                info!(
                    "[SharedDB] 接管成功，采集完成: {} sessions, {} 条新消息",
                    result.sessions_scanned, result.messages_inserted
                );
            }
        }

        Ok(taken)
    }

    // ==================== 数据操作 API ====================

    /// 获取或创建项目
    pub async fn get_or_create_project(&self, name: &str, path: &str, source: &str) -> Result<i64> {
        // 写操作，使用 write 锁
        let db = self.db.write().await;
        Ok(db.get_or_create_project(name, path, source)?)
    }

    /// 获取或创建项目（支持 encoded_dir_name）
    pub async fn get_or_create_project_with_encoded(
        &self,
        name: &str,
        path: &str,
        source: &str,
        encoded_dir_name: Option<&str>,
    ) -> Result<i64> {
        let db = self.db.write().await;
        Ok(db.get_or_create_project_with_encoded(name, path, source, encoded_dir_name)?)
    }

    /// Upsert 会话（完整版，支持所有元数据字段）
    pub async fn upsert_session(&self, input: &SessionInput) -> Result<()> {
        let db = self.db.write().await;
        Ok(db.upsert_session_full(input)?)
    }

    /// 批量插入消息
    pub async fn insert_messages(
        &self,
        session_id: &str,
        messages: &[MessageInput],
    ) -> Result<usize> {
        // 写操作，使用 write 锁
        let db = self.db.write().await;
        Ok(db.insert_messages(session_id, messages)?)
    }

    /// 列出项目
    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        let db = self.db.read().await;
        Ok(db.list_projects()?)
    }

    /// 列出项目（带统计信息）
    pub async fn list_projects_with_stats(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<claude_session_db::ProjectWithStats>> {
        let db = self.db.read().await;
        Ok(db.list_projects_with_stats(limit, offset)?)
    }

    /// 列出会话
    pub async fn list_sessions(&self, project_id: i64) -> Result<Vec<Session>> {
        let db = self.db.read().await;
        Ok(db.list_sessions(project_id)?)
    }

    /// 列出消息
    pub async fn list_messages(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Message>> {
        let db = self.db.read().await;
        Ok(db.list_messages(session_id, limit, offset)?)
    }

    /// FTS 搜索
    pub async fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let db = self.db.read().await;
        Ok(db.search_fts(query, limit)?)
    }

    /// 按项目 FTS 搜索
    pub async fn search_fts_with_project(
        &self,
        query: &str,
        limit: usize,
        project_id: Option<i64>,
    ) -> Result<Vec<SearchResult>> {
        let db = self.db.read().await;
        Ok(db.search_fts_with_project(query, limit, project_id)?)
    }

    /// FTS 搜索（完整版，支持日期范围过滤）
    pub async fn search_fts_full(
        &self,
        query: &str,
        limit: usize,
        project_id: Option<i64>,
        order_by: claude_session_db::SearchOrderBy,
        start_timestamp: Option<i64>,
        end_timestamp: Option<i64>,
    ) -> Result<Vec<SearchResult>> {
        let db = self.db.read().await;
        Ok(db.search_fts_full(
            query,
            limit,
            project_id,
            order_by,
            start_timestamp,
            end_timestamp,
        )?)
    }

    /// 获取统计信息
    pub async fn get_stats(&self) -> Result<claude_session_db::Stats> {
        let db = self.db.read().await;
        Ok(db.get_stats()?)
    }

    // ==================== 向量索引 ====================

    /// 获取未向量索引的消息（用于增量索引）
    pub async fn get_unindexed_messages(
        &self,
        limit: usize,
    ) -> Result<Vec<claude_session_db::Message>> {
        let db = self.db.read().await;
        Ok(db.get_unindexed_messages(limit)?)
    }

    /// 标记消息已向量索引
    pub async fn mark_messages_indexed(&self, message_ids: &[i64]) -> Result<usize> {
        let db = self.db.write().await;
        Ok(db.mark_messages_indexed(message_ids)?)
    }

    /// 获取未索引消息的数量
    pub async fn count_unindexed_messages(&self) -> Result<i64> {
        let db = self.db.read().await;
        Ok(db.count_unindexed_messages()?)
    }

    /// 标记消息向量索引失败
    pub async fn mark_message_index_failed(&self, message_id: i64) -> Result<()> {
        let db = self.db.write().await;
        Ok(db.mark_message_index_failed(message_id)?)
    }

    /// 批量标记消息向量索引失败
    pub async fn mark_messages_index_failed(&self, message_ids: &[i64]) -> Result<usize> {
        let db = self.db.write().await;
        Ok(db.mark_messages_index_failed(message_ids)?)
    }

    /// 获取索引失败的消息
    pub async fn get_failed_indexed_messages(
        &self,
        limit: usize,
    ) -> Result<Vec<claude_session_db::Message>> {
        let db = self.db.read().await;
        Ok(db.get_failed_indexed_messages(limit)?)
    }

    /// 统计索引失败的消息数量
    pub async fn count_failed_indexed_messages(&self) -> Result<i64> {
        let db = self.db.read().await;
        Ok(db.count_failed_indexed_messages()?)
    }

    /// 重置失败的索引状态（将 -1 改为 0，可重新索引）
    pub async fn reset_failed_indexed_messages(&self) -> Result<usize> {
        let db = self.db.write().await;
        Ok(db.reset_failed_indexed_messages()?)
    }

    /// 按 ID 列表获取消息
    pub async fn get_messages_by_ids(
        &self,
        ids: &[i64],
    ) -> Result<Vec<claude_session_db::Message>> {
        let db = self.db.read().await;
        Ok(db.get_messages_by_ids(ids)?)
    }

    // ==================== 新增 API ====================

    /// 获取单个 Session
    pub async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Option<claude_session_db::Session>> {
        let db = self.db.read().await;
        Ok(db.get_session(session_id)?)
    }

    /// 检查 Session 是否存在
    pub async fn session_exists(&self, session_id: &str) -> Result<bool> {
        let db = self.db.read().await;
        Ok(db.session_exists(session_id)?)
    }

    /// 获取 Session 的消息数量
    pub async fn get_session_message_count(&self, session_id: &str) -> Result<i64> {
        let db = self.db.read().await;
        Ok(db.get_session_message_count(session_id)?)
    }

    /// 获取 Session 的最新消息时间戳（毫秒）
    pub async fn get_session_latest_timestamp(&self, session_id: &str) -> Result<Option<i64>> {
        let db = self.db.read().await;
        Ok(db.get_session_latest_timestamp(session_id)?)
    }

    /// 获取 Sessions (支持可选的 project_id 过滤)
    pub async fn get_sessions(
        &self,
        project_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<Session>> {
        let db = self.db.read().await;
        Ok(db.get_sessions(project_id, limit)?)
    }

    /// 获取 Session 的所有 Messages (无分页)
    pub async fn get_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let db = self.db.read().await;
        Ok(db.get_messages(session_id)?)
    }

    /// 获取 Session 的 Messages (带分页和排序选项)
    pub async fn get_messages_with_options(
        &self,
        session_id: &str,
        limit: Option<usize>,
        desc: bool,
    ) -> Result<Vec<Message>> {
        let db = self.db.read().await;
        Ok(db.get_messages_with_options(session_id, limit, desc)?)
    }

    /// 通过前缀解析完整会话 ID
    pub async fn resolve_session_id(&self, prefix: &str) -> Result<Option<String>> {
        let db = self.db.read().await;
        Ok(db.resolve_session_id(prefix)?)
    }

    /// 按 session_id 前缀搜索会话列表
    pub async fn search_sessions_by_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<Session>> {
        let db = self.db.read().await;
        Ok(db.search_sessions_by_prefix(prefix, limit)?)
    }

    /// 获取单个 Project
    pub async fn get_project(&self, id: i64) -> Result<Option<Project>> {
        let db = self.db.read().await;
        Ok(db.get_project(id)?)
    }

    /// 统计缺少 cwd 的会话数量
    pub async fn count_sessions_without_cwd(&self) -> Result<i64> {
        let db = self.db.read().await;
        Ok(db.count_sessions_without_cwd()?)
    }

    /// 获取所有项目（带 source 字段）
    pub async fn get_all_projects_with_source(
        &self,
    ) -> Result<Vec<claude_session_db::ProjectWithSource>> {
        let db = self.db.read().await;
        Ok(db.get_all_projects_with_source()?)
    }

    /// 更新会话的项目 ID
    pub async fn update_sessions_project_id(
        &self,
        from_project_id: i64,
        to_project_id: i64,
    ) -> Result<usize> {
        let db = self.db.write().await;
        Ok(db.update_sessions_project_id(from_project_id, to_project_id)?)
    }

    /// 删除项目
    pub async fn delete_project(&self, project_id: i64) -> Result<()> {
        let db = self.db.write().await;
        Ok(db.delete_project(project_id)?)
    }

    /// 去重项目
    pub async fn deduplicate_projects(&self) -> Result<(usize, Vec<i64>)> {
        let db = self.db.write().await;
        Ok(db.deduplicate_projects()?)
    }

    /// 获取最近的消息（用于 SessionStart fallback）
    ///
    /// 当没有 L3/L2/L1 数据时，直接返回最近的原始消息
    ///
    /// # Arguments
    /// * `project_id` - 可选的项目 ID 过滤
    /// * `limit` - 最大返回数量
    pub async fn get_recent_messages(
        &self,
        project_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        // 获取最近的 sessions（最多 limit 个）
        let sessions = self.get_sessions(project_id, limit).await?;

        if sessions.is_empty() {
            return Ok(vec![]);
        }

        let mut all_messages = Vec::new();

        // 从每个 session 获取最新的消息
        for session in sessions {
            // 获取该 session 最新的几条消息（降序）
            let msgs = self
                .get_messages_with_options(&session.session_id, Some(3), true)
                .await?;
            all_messages.extend(msgs);

            // 如果已经够了，就停止
            if all_messages.len() >= limit {
                break;
            }
        }

        // 按时间戳排序（降序）并截断
        all_messages.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all_messages.truncate(limit);

        Ok(all_messages)
    }

    // ==================== 数据库维护 ====================

    /// 执行 WAL checkpoint，将 WAL 数据合并回主数据库
    ///
    /// 在优雅关闭时调用，确保数据持久化
    pub async fn checkpoint(&self) -> Result<()> {
        let db = self.db.write().await;
        db.checkpoint()?;
        Ok(())
    }

    /// 检查数据库完整性（快速检查）
    pub async fn quick_check(&self) -> Result<claude_session_db::IntegrityCheckResult> {
        let db = self.db.read().await;
        Ok(db.quick_check()?)
    }

    /// 检查数据库完整性（完整检查，较慢）
    pub async fn integrity_check(&self) -> Result<claude_session_db::IntegrityCheckResult> {
        let db = self.db.read().await;
        Ok(db.integrity_check()?)
    }

    // ==================== 采集操作 ====================

    /// 执行全量采集（同步，使用 claude-session-db::Collector）
    ///
    /// 此方法是同步的，会阻塞当前线程直到完成。
    /// 应该在 spawn_blocking 或非 async 上下文中调用。
    pub fn collect_all_sync(&self) -> Result<claude_session_db::CollectResult> {
        // 使用 try_write 获取锁，避免 async 死锁
        let db = self.db.try_write().map_err(|_| {
            anyhow::anyhow!("无法获取数据库写锁，可能有其他操作正在进行")
        })?;

        let collector = claude_session_db::Collector::new(&db);
        Ok(collector.collect_all()?)
    }

    /// 按路径采集单个会话（同步，使用 claude-session-db::Collector）
    pub fn collect_by_path_sync(&self, path: &str) -> Result<claude_session_db::CollectResult> {
        let db = self.db.try_write().map_err(|_| {
            anyhow::anyhow!("无法获取数据库写锁，可能有其他操作正在进行")
        })?;

        let collector = claude_session_db::Collector::new(&db);
        Ok(collector.collect_by_path(path)?)
    }
}

impl Drop for SharedDbAdapter {
    fn drop(&mut self) {
        // 尝试同步停止心跳（best effort）
        if let Ok(mut cancel) = self.heartbeat_cancel.try_write() {
            *cancel = true;
        }
        if let Ok(mut handle) = self.heartbeat_handle.try_write() {
            if let Some(h) = handle.take() {
                h.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_adapter_creation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let adapter = SharedDbAdapter::new(Some(db_path)).unwrap();
        let role = adapter.register().await.unwrap();

        // 首次注册应该成为 Writer
        assert_eq!(role, Role::Writer);
        assert!(adapter.is_writer().await);
    }

    #[tokio::test]
    async fn test_two_adapters_coordination() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let adapter1 = SharedDbAdapter::new(Some(db_path.clone())).unwrap();
        let adapter2 = SharedDbAdapter::new(Some(db_path)).unwrap();

        let role1 = adapter1.register().await.unwrap();
        let role2 = adapter2.register().await.unwrap();

        // 第一个成为 Writer，第二个成为 Reader
        assert_eq!(role1, Role::Writer);
        assert_eq!(role2, Role::Reader);
    }

    #[tokio::test]
    async fn test_release_and_reregister() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let adapter = SharedDbAdapter::new(Some(db_path)).unwrap();

        // 首次注册
        let role1 = adapter.register().await.unwrap();
        assert_eq!(role1, Role::Writer);

        // 释放
        adapter.release().await.unwrap();
        assert_eq!(adapter.role().await, Role::Reader);

        // 再次注册应该能成为 Writer
        let role2 = adapter.register().await.unwrap();
        assert_eq!(role2, Role::Writer);
    }
}
