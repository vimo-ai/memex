//! 只读数据库封装
//!
//! 替代 SharedDbAdapter，仅提供读取功能。
//! 写入由 Agent 统一负责，memex-rs 只需要读取数据用于搜索、RAG、compact 等。

use ai_cli_session_db::{
    DbConfig, Message, Project, SearchResult, Session, SessionDB,
};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// 只读数据库
///
/// 封装 ai-cli-session-db::SessionDB，只提供读取操作。
/// 写入、协调、心跳等功能由 Agent 统一处理。
pub struct DbReader {
    /// 数据库连接
    db: Arc<RwLock<SessionDB>>,
}

impl DbReader {
    /// 创建只读数据库连接
    ///
    /// # Arguments
    /// - `db_path`: 数据库路径（默认 ~/.vimo/db/ai-cli-session.db）
    pub fn new(db_path: Option<PathBuf>) -> Result<Self> {
        let db_path = db_path.unwrap_or_else(|| {
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

        info!("[DbReader] Connecting to database (read-only mode): {:?}", db_path);
        let config = DbConfig::local(db_path.to_string_lossy().into_owned());
        let db = SessionDB::connect(config)?;

        Ok(Self {
            db: Arc::new(RwLock::new(db)),
        })
    }

    // ==================== 项目操作 ====================

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
    ) -> Result<Vec<ai_cli_session_db::ProjectWithStats>> {
        let db = self.db.read().await;
        Ok(db.list_projects_with_stats(limit, offset)?)
    }

    /// 获取单个项目
    pub async fn get_project(&self, id: i64) -> Result<Option<Project>> {
        let db = self.db.read().await;
        Ok(db.get_project(id)?)
    }

    /// 获取所有项目（带 source 字段）
    pub async fn get_all_projects_with_source(
        &self,
    ) -> Result<Vec<ai_cli_session_db::ProjectWithSource>> {
        let db = self.db.read().await;
        Ok(db.get_all_projects_with_source()?)
    }

    // ==================== 会话操作 ====================

    /// 列出会话
    pub async fn list_sessions(&self, project_id: i64) -> Result<Vec<Session>> {
        let db = self.db.read().await;
        Ok(db.list_sessions(project_id)?)
    }

    /// 获取 Sessions（支持可选的 project_id 过滤）
    pub async fn get_sessions(
        &self,
        project_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<Session>> {
        let db = self.db.read().await;
        Ok(db.get_sessions(project_id, limit)?)
    }

    /// 获取单个 Session
    pub async fn get_session(&self, session_id: &str) -> Result<Option<Session>> {
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

    /// 获取 Session 的最新消息时间戳
    pub async fn get_session_latest_timestamp(&self, session_id: &str) -> Result<Option<i64>> {
        let db = self.db.read().await;
        Ok(db.get_session_latest_timestamp(session_id)?)
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

    /// 统计缺少 cwd 的会话数量
    pub async fn count_sessions_without_cwd(&self) -> Result<i64> {
        let db = self.db.read().await;
        Ok(db.count_sessions_without_cwd()?)
    }

    // ==================== 消息操作 ====================

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

    /// 获取 Session 的所有 Messages（无分页）
    pub async fn get_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let db = self.db.read().await;
        Ok(db.get_messages(session_id)?)
    }

    /// 获取 Session 的 Messages（带分页和排序选项）
    pub async fn get_messages_with_options(
        &self,
        session_id: &str,
        limit: Option<usize>,
        desc: bool,
    ) -> Result<Vec<Message>> {
        let db = self.db.read().await;
        Ok(db.get_messages_with_options(session_id, limit, desc)?)
    }

    /// 按 ID 列表获取消息
    pub async fn get_messages_by_ids(&self, ids: &[i64]) -> Result<Vec<Message>> {
        let db = self.db.read().await;
        Ok(db.get_messages_by_ids(ids)?)
    }

    /// 获取最近的消息
    pub async fn get_recent_messages(
        &self,
        project_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let sessions = self.get_sessions(project_id, limit).await?;

        if sessions.is_empty() {
            return Ok(vec![]);
        }

        let mut all_messages = Vec::new();

        for session in sessions {
            let msgs = self
                .get_messages_with_options(&session.session_id, Some(3), true)
                .await?;
            all_messages.extend(msgs);

            if all_messages.len() >= limit {
                break;
            }
        }

        all_messages.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all_messages.truncate(limit);

        Ok(all_messages)
    }

    // ==================== 搜索操作 ====================

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
        order_by: ai_cli_session_db::SearchOrderBy,
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

    // ==================== 统计和维护 ====================

    /// 获取统计信息
    pub async fn get_stats(&self) -> Result<ai_cli_session_db::Stats> {
        let db = self.db.read().await;
        Ok(db.get_stats()?)
    }

    /// 执行 WAL checkpoint
    pub async fn checkpoint(&self) -> Result<()> {
        let db = self.db.write().await;
        db.checkpoint()?;
        Ok(())
    }

    /// 检查数据库完整性（快速检查）
    pub async fn quick_check(&self) -> Result<ai_cli_session_db::IntegrityCheckResult> {
        let db = self.db.read().await;
        Ok(db.quick_check()?)
    }

    /// 检查数据库完整性（完整检查）
    pub async fn integrity_check(&self) -> Result<ai_cli_session_db::IntegrityCheckResult> {
        let db = self.db.read().await;
        Ok(db.integrity_check()?)
    }

    // ==================== 向量索引相关（只读状态查询） ====================

    /// 获取未向量索引的消息
    pub async fn get_unindexed_messages(&self, limit: usize) -> Result<Vec<Message>> {
        let db = self.db.read().await;
        Ok(db.get_unindexed_messages(limit)?)
    }

    /// 获取未索引消息的数量
    pub async fn count_unindexed_messages(&self) -> Result<i64> {
        let db = self.db.read().await;
        Ok(db.count_unindexed_messages()?)
    }

    /// 获取索引失败的消息
    pub async fn get_failed_indexed_messages(&self, limit: usize) -> Result<Vec<Message>> {
        let db = self.db.read().await;
        Ok(db.get_failed_indexed_messages(limit)?)
    }

    /// 统计索引失败的消息数量
    pub async fn count_failed_indexed_messages(&self) -> Result<i64> {
        let db = self.db.read().await;
        Ok(db.count_failed_indexed_messages()?)
    }

    // ==================== 向量索引写入（需要写锁） ====================
    // 注意：这些是例外情况，向量索引状态写入仍然在 memex-rs 本地完成

    /// 标记消息已向量索引
    pub async fn mark_messages_indexed(&self, message_ids: &[i64]) -> Result<usize> {
        let db = self.db.write().await;
        Ok(db.mark_messages_indexed(message_ids)?)
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

    /// 重置失败的索引状态
    pub async fn reset_failed_indexed_messages(&self) -> Result<usize> {
        let db = self.db.write().await;
        Ok(db.reset_failed_indexed_messages()?)
    }

    // ==================== 管理操作（需要写锁） ====================
    // 注意：这些操作原本在 SharedDbAdapter 中，保留供 admin API 使用

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use tempfile::tempdir;

    fn set_test_env() {
        static ROOT: OnceLock<PathBuf> = OnceLock::new();
        let root = ROOT.get_or_init(|| {
            let dir = tempdir().unwrap();
            let path = dir.path().to_path_buf();
            std::mem::forget(dir);
            path
        });

        std::env::set_var("CLAUDE_PROJECTS_PATH", root);
        std::env::set_var("CODEX_PATH", root);
        std::env::set_var("OPENCODE_PATH", root);
        std::env::set_var("GEMINI_TMP_PATH", root);
    }

    #[tokio::test]
    async fn test_db_reader_creation() {
        set_test_env();
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let reader = DbReader::new(Some(db_path)).unwrap();
        let stats = reader.get_stats().await.unwrap();
        assert_eq!(stats.project_count, 0);
    }
}
