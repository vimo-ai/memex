//! 共享数据库适配器
//!
//! 集成 claude-session-db，实现 Writer 协调和数据共享

use anyhow::Result;
use claude_session_db::{
    coordination::{Role, WriterHealth, WriterType},
    db::MessageInput,
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
    /// - `shared_db_path`: 共享数据库路径（默认 ~/.vimo/db/claude-session.db）
    pub fn new(shared_db_path: Option<PathBuf>) -> Result<Self> {
        let db_path = shared_db_path.unwrap_or_else(|| {
            // 支持 VIMO_HOME 环境变量覆盖
            let vimo_root = std::env::var("VIMO_HOME").unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{}/.vimo", home)
            });
            PathBuf::from(format!("{}/db/claude-session.db", vimo_root))
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
    pub async fn register(&self) -> Result<Role> {
        // 重置取消标志
        *self.heartbeat_cancel.write().await = false;

        let mut db = self.db.write().await;
        let role = db.register_writer(WriterType::MemexDaemon)?;
        drop(db);

        *self.role.write().await = role;

        if role == Role::Writer {
            info!("[SharedDB] 成为 Writer，启动协调任务");
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
    pub async fn try_takeover(&self) -> Result<bool> {
        // 重置取消标志
        *self.heartbeat_cancel.write().await = false;

        let mut db = self.db.write().await;
        let taken = db.try_takeover()?;

        if taken {
            drop(db);
            *self.role.write().await = Role::Writer;
            self.start_heartbeat().await;
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

    /// Upsert 会话
    pub async fn upsert_session(&self, session_id: &str, project_id: i64) -> Result<()> {
        // 写操作，使用 write 锁
        let db = self.db.write().await;
        Ok(db.upsert_session(session_id, project_id)?)
    }

    /// 批量插入消息
    pub async fn insert_messages(&self, session_id: &str, messages: &[MessageInput]) -> Result<usize> {
        // 写操作，使用 write 锁
        let db = self.db.write().await;
        Ok(db.insert_messages(session_id, messages)?)
    }

    /// 列出项目
    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        let db = self.db.read().await;
        Ok(db.list_projects()?)
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

    /// 获取统计信息
    pub async fn get_stats(&self) -> Result<claude_session_db::Stats> {
        let db = self.db.read().await;
        Ok(db.get_stats()?)
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
