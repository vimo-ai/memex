//! 数据库层 - SQLite + FTS5

#![allow(dead_code)] // 预留 API: create_session

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::domain::{Message, Project, SearchResult, Session};

/// Schema 初始化 SQL
const SCHEMA_SQL: &str = r#"
-- 项目表
CREATE TABLE IF NOT EXISTS projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'claude',
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    UNIQUE(path, source)
);

-- 会话表
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    project_id INTEGER NOT NULL,
    cwd TEXT,
    model TEXT,
    source TEXT DEFAULT 'claude',
    channel TEXT DEFAULT 'code',
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    FOREIGN KEY (project_id) REFERENCES projects(id)
);

-- 消息表
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uuid TEXT NOT NULL,
    session_id TEXT NOT NULL,
    type TEXT NOT NULL,
    content TEXT NOT NULL,
    source TEXT DEFAULT 'claude',
    channel TEXT DEFAULT 'code',
    timestamp TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
CREATE INDEX IF NOT EXISTS idx_messages_type ON messages(type);
CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_uuid_session ON messages(uuid, session_id);

-- 向量索引状态字段（迁移）
-- 注意：SQLite 不支持 IF NOT EXISTS 于 ALTER TABLE，需要单独处理

-- FTS5 全文搜索
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    content,
    content=messages,
    content_rowid=id,
    tokenize='unicode61'
);

-- FTS 触发器
CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.id, old.content);
    INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
END;
"#;

/// 数据库连接包装
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// 打开数据库
    pub fn open(path: &Path) -> Result<Self> {
        // 确保目录存在
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("无法打开数据库: {:?}", path))?;

        // 初始化 schema
        conn.execute_batch(SCHEMA_SQL)
            .context("初始化数据库 schema 失败")?;

        // 迁移：添加 vector_indexed 字段（如果不存在）
        Self::migrate_vector_indexed(&conn)?;

        tracing::info!("数据库已打开: {:?}", path);

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    // ==================== 项目操作 ====================

    /// 获取或创建项目
    pub fn get_or_create_project(&self, name: &str, path: &str, source: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();

        // 先尝试查找（按 path + source 唯一）
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM projects WHERE path = ?1 AND source = ?2",
                params![path, source],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(id) = existing {
            return Ok(id);
        }

        // 创建新项目
        conn.execute(
            "INSERT INTO projects (name, path, source) VALUES (?1, ?2, ?3)",
            params![name, path, source],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// 获取所有项目
    pub fn get_projects(&self) -> Result<Vec<Project>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                p.id,
                p.name,
                p.path,
                COUNT(DISTINCT s.id) as session_count,
                COUNT(m.id) as message_count,
                MAX(m.timestamp) as last_active
            FROM projects p
            LEFT JOIN sessions s ON s.project_id = p.id
            LEFT JOIN messages m ON m.session_id = s.id
            GROUP BY p.id
            ORDER BY last_active DESC NULLS LAST
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                session_count: row.get(3)?,
                message_count: row.get(4)?,
                last_active: row.get(5)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 获取项目详情
    pub fn get_project(&self, id: i64) -> Result<Option<Project>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"
            SELECT
                p.id,
                p.name,
                p.path,
                COUNT(DISTINCT s.id) as session_count,
                COUNT(m.id) as message_count,
                MAX(m.timestamp) as last_active
            FROM projects p
            LEFT JOIN sessions s ON s.project_id = p.id
            LEFT JOIN messages m ON m.session_id = s.id
            WHERE p.id = ?1
            GROUP BY p.id
            "#,
            params![id],
            |row| {
                Ok(Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    session_count: row.get(3)?,
                    message_count: row.get(4)?,
                    last_active: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    // ==================== 会话操作 ====================

    /// 检查会话是否存在
    pub fn session_exists(&self, session_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// 通过前缀解析完整会话 ID
    pub fn resolve_session_id(&self, prefix: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("{}%", prefix);
        let result: rusqlite::Result<String> = conn.query_row(
            "SELECT id FROM sessions WHERE id LIKE ?1 LIMIT 1",
            params![pattern],
            |row| row.get(0),
        );
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 按 ID 前缀搜索会话列表
    pub fn search_sessions_by_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("{}%", prefix);

        let mut stmt = conn.prepare(
            r#"
            SELECT
                s.id,
                s.project_id,
                p.name as project_name,
                COUNT(m.id) as message_count,
                MIN(m.timestamp) as first_message,
                MAX(m.timestamp) as last_message
            FROM sessions s
            JOIN projects p ON p.id = s.project_id
            LEFT JOIN messages m ON m.session_id = s.id
            WHERE s.id LIKE ?1
            GROUP BY s.id
            ORDER BY last_message DESC NULLS LAST
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![pattern, limit as i64], |row| {
            Ok(Session {
                id: row.get(0)?,
                project_id: row.get(1)?,
                project_name: row.get(2)?,
                message_count: row.get(3)?,
                first_message: row.get(4)?,
                last_message: row.get(5)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 获取单个会话详情
    pub fn get_session(&self, session_id: &str) -> Result<Option<Session>> {
        let conn = self.conn.lock().unwrap();

        conn.query_row(
            r#"
            SELECT
                s.id,
                s.project_id,
                p.name as project_name,
                COUNT(m.id) as message_count,
                MIN(m.timestamp) as first_message,
                MAX(m.timestamp) as last_message
            FROM sessions s
            JOIN projects p ON p.id = s.project_id
            LEFT JOIN messages m ON m.session_id = s.id
            WHERE s.id = ?1
            GROUP BY s.id
            "#,
            params![session_id],
            |row| {
                Ok(Session {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    project_name: row.get(2)?,
                    message_count: row.get(3)?,
                    first_message: row.get(4)?,
                    last_message: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    /// 获取项目下的会话列表
    pub fn get_sessions_by_project(&self, project_id: i64) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            r#"
            SELECT
                s.id,
                s.project_id,
                p.name as project_name,
                COUNT(m.id) as message_count,
                MIN(m.timestamp) as first_message,
                MAX(m.timestamp) as last_message
            FROM sessions s
            JOIN projects p ON p.id = s.project_id
            LEFT JOIN messages m ON m.session_id = s.id
            WHERE s.project_id = ?1
            GROUP BY s.id
            ORDER BY last_message DESC NULLS LAST
            "#,
        )?;

        let rows = stmt.query_map(params![project_id], |row| {
            Ok(Session {
                id: row.get(0)?,
                project_id: row.get(1)?,
                project_name: row.get(2)?,
                message_count: row.get(3)?,
                first_message: row.get(4)?,
                last_message: row.get(5)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 创建会话
    pub fn create_session(
        &self,
        session_id: &str,
        project_id: i64,
        cwd: Option<&str>,
        model: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO sessions (id, project_id, cwd, model) VALUES (?1, ?2, ?3, ?4)",
            params![session_id, project_id, cwd, model],
        )?;
        Ok(())
    }

    /// 获取会话列表
    pub fn get_sessions(&self, project_id: Option<i64>, limit: usize) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();

        let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = if let Some(pid) = project_id {
            (
                r#"
                SELECT
                    s.id,
                    s.project_id,
                    p.name as project_name,
                    COUNT(m.id) as message_count,
                    MIN(m.timestamp) as first_message,
                    MAX(m.timestamp) as last_message
                FROM sessions s
                JOIN projects p ON p.id = s.project_id
                LEFT JOIN messages m ON m.session_id = s.id
                WHERE s.project_id = ?1
                GROUP BY s.id
                ORDER BY last_message DESC NULLS LAST
                LIMIT ?2
                "#,
                vec![Box::new(pid) as Box<dyn rusqlite::ToSql>, Box::new(limit as i64)],
            )
        } else {
            (
                r#"
                SELECT
                    s.id,
                    s.project_id,
                    p.name as project_name,
                    COUNT(m.id) as message_count,
                    MIN(m.timestamp) as first_message,
                    MAX(m.timestamp) as last_message
                FROM sessions s
                JOIN projects p ON p.id = s.project_id
                LEFT JOIN messages m ON m.session_id = s.id
                GROUP BY s.id
                ORDER BY last_message DESC NULLS LAST
                LIMIT ?1
                "#,
                vec![Box::new(limit as i64)],
            )
        };

        let mut stmt = conn.prepare(sql)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(Session {
                id: row.get(0)?,
                project_id: row.get(1)?,
                project_name: row.get(2)?,
                message_count: row.get(3)?,
                first_message: row.get(4)?,
                last_message: row.get(5)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 获取会话的消息数量
    pub fn get_session_message_count(&self, session_id: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    /// 创建会话 (v2 - 支持 source 和 channel)
    pub fn create_session_v2(
        &self,
        session_id: &str,
        project_id: i64,
        cwd: Option<&str>,
        model: Option<&str>,
        source: &str,
        channel: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO sessions (id, project_id, cwd, model, source, channel) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_id, project_id, cwd, model, source, channel],
        )?;
        Ok(())
    }

    // ==================== 消息操作 ====================

    /// 批量插入消息
    /// 返回 (插入数量, 新插入的消息 ID 列表)
    pub fn insert_messages_v2(&self, session_id: &str, messages: &[crate::adapter::ParsedMessage]) -> Result<(usize, Vec<i64>)> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        let mut inserted = 0;
        let mut new_ids = Vec::new();

        for msg in messages {
            let result = tx.execute(
                r#"
                INSERT OR IGNORE INTO messages (uuid, session_id, type, content, source, channel, timestamp)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
                params![
                    msg.uuid,
                    session_id,
                    msg.message_type.to_string(),
                    msg.content,
                    msg.source.to_string(),
                    msg.channel,
                    msg.timestamp,
                ],
            );

            if let Ok(n) = result {
                if n > 0 {
                    inserted += n;
                    new_ids.push(tx.last_insert_rowid());
                }
            }
        }

        tx.commit()?;
        Ok((inserted, new_ids))
    }

    /// 获取会话的消息
    pub fn get_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        self.get_messages_with_options(session_id, None, false)
    }

    /// 获取会话的消息（带分页和排序选项）
    /// - limit: 返回数量限制
    /// - desc: true 表示倒序（最新的在前）
    pub fn get_messages_with_options(
        &self,
        session_id: &str,
        limit: Option<usize>,
        desc: bool,
    ) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let order = if desc { "DESC" } else { "ASC" };

        let sql = format!(
            "SELECT id, uuid, session_id, type, content, timestamp
             FROM messages WHERE session_id = ?1 ORDER BY id {} LIMIT ?2",
            order
        );

        let limit_val = limit.unwrap_or(i64::MAX as usize) as i64;
        let mut stmt = conn.prepare(&sql)?;

        let rows = stmt.query_map(params![session_id, limit_val], |row| {
            Ok(Message {
                id: row.get(0)?,
                uuid: row.get(1)?,
                session_id: row.get(2)?,
                r#type: row.get(3)?,
                content: row.get(4)?,
                timestamp: row.get(5)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 按 ID 列表获取消息
    pub fn get_messages_by_ids(&self, ids: &[i64]) -> Result<Vec<Message>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            r#"
            SELECT id, uuid, session_id, type, content, timestamp
            FROM messages
            WHERE id IN ({})
            ORDER BY id ASC
            "#,
            placeholders
        );

        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(Message {
                id: row.get(0)?,
                uuid: row.get(1)?,
                session_id: row.get(2)?,
                r#type: row.get(3)?,
                content: row.get(4)?,
                timestamp: row.get(5)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // ==================== 搜索操作 ====================

    /// FTS5 全文搜索
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        project_id: Option<i64>,
    ) -> Result<Vec<SearchResult>> {
        let conn = self.conn.lock().unwrap();

        let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = if let Some(pid) = project_id {
            (
                r#"
                SELECT
                    m.id,
                    m.session_id,
                    s.project_id,
                    p.name as project_name,
                    m.type,
                    m.content,
                    snippet(messages_fts, 0, '<mark>', '</mark>', '...', 64) as snippet,
                    bm25(messages_fts) as score,
                    m.timestamp
                FROM messages_fts
                JOIN messages m ON messages_fts.rowid = m.id
                JOIN sessions s ON m.session_id = s.id
                JOIN projects p ON s.project_id = p.id
                WHERE messages_fts MATCH ?1
                  AND s.project_id = ?2
                ORDER BY score
                LIMIT ?3
                "#,
                vec![
                    Box::new(query.to_string()) as Box<dyn rusqlite::ToSql>,
                    Box::new(pid),
                    Box::new(limit as i64),
                ],
            )
        } else {
            (
                r#"
                SELECT
                    m.id,
                    m.session_id,
                    s.project_id,
                    p.name as project_name,
                    m.type,
                    m.content,
                    snippet(messages_fts, 0, '<mark>', '</mark>', '...', 64) as snippet,
                    bm25(messages_fts) as score,
                    m.timestamp
                FROM messages_fts
                JOIN messages m ON messages_fts.rowid = m.id
                JOIN sessions s ON m.session_id = s.id
                JOIN projects p ON s.project_id = p.id
                WHERE messages_fts MATCH ?1
                ORDER BY score
                LIMIT ?2
                "#,
                vec![
                    Box::new(query.to_string()) as Box<dyn rusqlite::ToSql>,
                    Box::new(limit as i64),
                ],
            )
        };

        let mut stmt = conn.prepare(sql)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(SearchResult {
                message_id: row.get(0)?,
                session_id: row.get(1)?,
                project_id: row.get(2)?,
                project_name: row.get(3)?,
                r#type: row.get(4)?,
                content: row.get(5)?,
                snippet: row.get(6)?,
                score: row.get(7)?,
                timestamp: row.get(8)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // ==================== 统计操作 ====================

    /// 获取统计信息
    pub fn get_stats(&self) -> Result<Stats> {
        let conn = self.conn.lock().unwrap();

        let project_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))?;

        let session_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;

        let message_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;

        Ok(Stats {
            project_count,
            session_count,
            message_count,
        })
    }

    // ==================== 管理操作 ====================

    /// 获取缺少 cwd 的会话数量
    pub fn count_sessions_without_cwd(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE cwd IS NULL OR cwd = ''",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    /// 获取所有项目（带 source 字段）
    pub fn get_all_projects_with_source(&self) -> Result<Vec<ProjectWithSource>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, name, path, source
            FROM projects
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(ProjectWithSource {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                source: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 更新会话的项目 ID
    pub fn update_sessions_project_id(&self, from_project_id: i64, to_project_id: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "UPDATE sessions SET project_id = ?1 WHERE project_id = ?2",
            params![to_project_id, from_project_id],
        )?;
        Ok(count)
    }

    /// 删除项目
    pub fn delete_project(&self, project_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM projects WHERE id = ?1", params![project_id])?;
        Ok(())
    }

    // ==================== 向量索引状态 ====================

    /// 迁移：添加 vector_indexed 字段
    fn migrate_vector_indexed(conn: &Connection) -> Result<()> {
        // 检查字段是否存在
        let has_column: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = 'vector_indexed'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .unwrap_or(false);

        if !has_column {
            tracing::info!("迁移：添加 vector_indexed 字段");
            conn.execute_batch(
                r#"
                ALTER TABLE messages ADD COLUMN vector_indexed INTEGER DEFAULT 0;
                CREATE INDEX IF NOT EXISTS idx_messages_vector_indexed ON messages(vector_indexed);
                "#,
            )?;
            tracing::info!("迁移完成：vector_indexed 字段已添加");
        }

        Ok(())
    }

    /// 获取未向量索引的消息（用于增量索引）
    /// 只返回 assistant 类型的消息
    pub fn get_unindexed_messages(&self, limit: usize) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, uuid, session_id, type, content, timestamp
            FROM messages
            WHERE vector_indexed = 0 AND type = 'assistant'
            ORDER BY id ASC
            LIMIT ?1
            "#,
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(Message {
                id: row.get(0)?,
                uuid: row.get(1)?,
                session_id: row.get(2)?,
                r#type: row.get(3)?,
                content: row.get(4)?,
                timestamp: row.get(5)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 标记消息已向量索引
    pub fn mark_messages_indexed(&self, message_ids: &[i64]) -> Result<usize> {
        if message_ids.is_empty() {
            return Ok(0);
        }

        let conn = self.conn.lock().unwrap();
        let placeholders: String = message_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE messages SET vector_indexed = 1 WHERE id IN ({})",
            placeholders
        );

        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = message_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();

        let count = stmt.execute(params.as_slice())?;
        Ok(count)
    }

    /// 获取未索引消息的数量
    pub fn count_unindexed_messages(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE vector_indexed = 0 AND type = 'assistant'",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    /// 去重项目 - 按 path 合并，保留 session 最多的记录
    /// 返回 (合并数量, 删除的项目 ID 列表)
    pub fn deduplicate_projects(&self) -> Result<(usize, Vec<i64>)> {
        let conn = self.conn.lock().unwrap();

        // 找出所有重复的 path（有多条记录）
        let mut stmt = conn.prepare(
            r#"
            SELECT path, GROUP_CONCAT(id) as ids
            FROM projects
            GROUP BY path
            HAVING COUNT(*) > 1
            "#,
        )?;

        let duplicates: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        drop(stmt);

        let mut merged_count = 0;
        let mut deleted_ids = Vec::new();

        for (path, ids_str) in duplicates {
            let ids: Vec<i64> = ids_str
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();

            if ids.len() < 2 {
                continue;
            }

            // 找出每个 project 的 session 数量
            let mut project_sessions: Vec<(i64, i64)> = Vec::new();
            for &id in &ids {
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sessions WHERE project_id = ?1",
                        params![id],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);
                project_sessions.push((id, count));
            }

            // 按 session 数量降序排序，保留第一个（最多的）
            project_sessions.sort_by(|a, b| b.1.cmp(&a.1));
            let keep_id = project_sessions[0].0;

            // 合并其他 project 的 sessions 到保留的那个
            for &(id, _) in &project_sessions[1..] {
                conn.execute(
                    "UPDATE sessions SET project_id = ?1 WHERE project_id = ?2",
                    params![keep_id, id],
                )?;

                // 删除重复的 project
                conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;

                deleted_ids.push(id);
                merged_count += 1;
            }

            tracing::info!(
                "去重项目 path={}: 保留 ID {}, 删除 {:?}",
                path,
                keep_id,
                &project_sessions[1..].iter().map(|(id, _)| id).collect::<Vec<_>>()
            );
        }

        Ok((merged_count, deleted_ids))
    }
}

/// 带 source 的项目信息
#[derive(Debug, Clone)]
pub struct ProjectWithSource {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub source: String,
}

/// 统计信息
#[derive(Debug, Clone, serde::Serialize)]
pub struct Stats {
    pub project_count: i64,
    pub session_count: i64,
    pub message_count: i64,
}
