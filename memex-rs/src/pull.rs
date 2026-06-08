//! Pull / Download —— 从 server 拉取某人提交的 session 到本地 peers.db
//!
//! 设计动机：远程搜索每查一次就在 server（NAS）上跑一遍混合检索，是查询期持续算力。
//! pull 把它换成一次性 bulk SELECT —— 把某个 peer 的 L0 原文拉到本地 peers.db，
//! 之后所有搜索都在本地 FTS5 上跑，server 查询期零负载。
//!
//! - 只拉 L0 原文（messages），不带向量、不带 talks/L2/L3。
//! - server 端纯 SELECT 组装，按 (updated_at, session_id) keyset 分页。
//! - 客户端写入独立的 peers.db（不混进自己的 ai-cli-session.db，避免被 sync 回传给 server）。

use serde::{Deserialize, Serialize};

// ==================== 传输类型（server 与 client 共享） ====================

/// 拉取游标：按 session 的 (updated_at, session_id) 做 keyset 翻页
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullCursor {
    pub updated_at: i64,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullProject {
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullSession {
    pub session_id: String,
    pub project_path: String,
    /// 真实归属人（server 端 sessions.pushed_by），按 session_id 拉取时也能落对 peer
    pub pushed_by: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub message_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// L0 消息精简字段（去掉 token / approval / raw 等本地搜索用不到的列）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullMessage {
    pub session_id: String,
    pub uuid: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub content_text: String,
    pub content_full: String,
    pub timestamp: i64,
    pub sequence: i64,
    pub source: Option<String>,
    pub model: Option<String>,
    pub tool_name: Option<String>,
}

/// 一页拉取结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullPage {
    pub projects: Vec<PullProject>,
    pub sessions: Vec<PullSession>,
    pub messages: Vec<PullMessage>,
    /// 下一页游标；None 表示已到末尾
    pub next_cursor: Option<PullCursor>,
    pub has_more: bool,
}

impl PullPage {
    pub fn empty() -> Self {
        Self {
            projects: Vec::new(),
            sessions: Vec::new(),
            messages: Vec::new(),
            next_cursor: None,
            has_more: false,
        }
    }
}

// ==================== 客户端：peers.db 写入器（仅 cli-core） ====================

#[cfg(feature = "cli-core")]
pub use peers_db::PeersDb;

#[cfg(feature = "cli-core")]
mod peers_db {
    use super::{PullPage, PullProject, PullSession};
    use anyhow::{Context, Result};
    use rusqlite::{params, Connection};
    use std::path::Path;

    /// 本地 peers.db —— 存放从 server 拉来的他人 session（L0 + FTS5）
    pub struct PeersDb {
        conn: Connection,
    }

    impl PeersDb {
        /// 打开 peers.db。若检测到旧的孤儿 schema（无 messages_fts），重建覆盖。
        pub fn open(path: &Path) -> Result<Self> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            let conn = Connection::open(path)
                .with_context(|| format!("打开 peers.db 失败: {}", path.display()))?;
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 PRAGMA busy_timeout=10000;",
            )?;

            let has_fts: bool = conn
                .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='messages_fts'")
                .and_then(|mut s| s.exists([]))
                .unwrap_or(false);

            if !has_fts {
                // 旧孤儿 schema（无 FTS）：丢弃可再生数据，重建
                conn.execute_batch(
                    "DROP TABLE IF EXISTS messages;
                     DROP TABLE IF EXISTS sessions;
                     DROP TABLE IF EXISTS projects;",
                )?;
            }

            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS projects (
                     id   INTEGER PRIMARY KEY AUTOINCREMENT,
                     path TEXT NOT NULL UNIQUE,
                     name TEXT NOT NULL,
                     peer TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS sessions (
                     id            INTEGER PRIMARY KEY AUTOINCREMENT,
                     session_id    TEXT NOT NULL UNIQUE,
                     project_id    INTEGER NOT NULL REFERENCES projects(id),
                     peer          TEXT NOT NULL,
                     message_count INTEGER NOT NULL DEFAULT 0,
                     model         TEXT,
                     source        TEXT DEFAULT 'claude',
                     cwd           TEXT,
                     created_at    INTEGER,
                     updated_at    INTEGER
                 );
                 CREATE INDEX IF NOT EXISTS idx_peer_sessions_peer ON sessions(peer);
                 CREATE INDEX IF NOT EXISTS idx_peer_sessions_updated ON sessions(updated_at DESC);
                 CREATE TABLE IF NOT EXISTS messages (
                     id           INTEGER PRIMARY KEY AUTOINCREMENT,
                     session_id   TEXT NOT NULL,
                     uuid         TEXT NOT NULL UNIQUE,
                     type         TEXT NOT NULL,
                     content_text TEXT NOT NULL,
                     content_full TEXT NOT NULL,
                     timestamp    INTEGER NOT NULL,
                     sequence     INTEGER NOT NULL,
                     source       TEXT DEFAULT 'claude',
                     model        TEXT,
                     tool_name    TEXT
                 );
                 CREATE INDEX IF NOT EXISTS idx_peer_messages_session ON messages(session_id);
                 CREATE INDEX IF NOT EXISTS idx_peer_messages_timestamp ON messages(timestamp);
                 CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                     content_full,
                     tokenize='trigram'
                 );",
            )?;

            Ok(Self { conn })
        }

        /// 写入一页拉取结果，返回新写入的消息数（去重后）。
        ///
        /// `fallback_peer` 仅在 session 自身缺 `pushed_by` 时兜底（正常都用真实 owner）。
        pub fn ingest_page(&mut self, fallback_peer: &str, page: &PullPage) -> Result<usize> {
            let tx = self.conn.transaction()?;
            for s in &page.sessions {
                let peer = s.pushed_by.as_deref().unwrap_or(fallback_peer);
                // project 跟随 session 的 owner 落 peer 列
                if let Some(proj) = page.projects.iter().find(|p| p.path == s.project_path) {
                    Self::upsert_project(&tx, peer, proj)?;
                }
                Self::upsert_session(&tx, peer, s)?;
            }
            let mut inserted = 0usize;
            for m in &page.messages {
                let changed = tx.execute(
                    "INSERT OR IGNORE INTO messages
                     (session_id, uuid, type, content_text, content_full,
                      timestamp, sequence, source, model, tool_name)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        m.session_id, m.uuid, m.msg_type, m.content_text, m.content_full,
                        m.timestamp, m.sequence, m.source, m.model, m.tool_name
                    ],
                )?;
                if changed > 0 {
                    // FTS 与 messages 共用 rowid，便于回查
                    let rowid = tx.last_insert_rowid();
                    tx.execute(
                        "INSERT INTO messages_fts(rowid, content_full) VALUES (?1, ?2)",
                        params![rowid, m.content_full],
                    )?;
                    inserted += 1;
                }
            }
            tx.commit()?;
            Ok(inserted)
        }

        /// 只读 FTS5 搜索 peers.db（渐进式开口给 MCP source=peers 用）。
        ///
        /// peers.db 不存在或尚无 FTS 表时返回空，不报错。结果形状对齐远程搜索：
        /// {session, snippet, project, peer, time, source:"peers"}
        pub fn search_readonly(
            path: &Path,
            query: &str,
            limit: usize,
        ) -> Result<Vec<serde_json::Value>> {
            if !path.exists() {
                return Ok(Vec::new());
            }
            let conn = Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            let has_fts: bool = conn
                .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='messages_fts'")
                .and_then(|mut s| s.exists([]))
                .unwrap_or(false);
            if !has_fts {
                return Ok(Vec::new());
            }

            // 把查询当作字符串字面量，避免 FTS5 语法字符触发解析错误
            let match_expr = format!("\"{}\"", query.replace('"', "\"\""));
            let limit = limit.clamp(1, 100) as i64;

            let mut stmt = conn.prepare(
                "SELECT m.session_id, m.content_text, m.timestamp, s.peer, p.name
                 FROM messages_fts f
                 JOIN messages m ON m.id = f.rowid
                 JOIN sessions s ON s.session_id = m.session_id
                 JOIN projects p ON p.id = s.project_id
                 WHERE f.content_full MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(params![match_expr, limit], |row| {
                    let session: String = row.get(0)?;
                    let snippet: String = row.get(1)?;
                    let ts: i64 = row.get(2)?;
                    let peer: String = row.get(3)?;
                    let project: String = row.get(4)?;
                    Ok(serde_json::json!({
                        "session": session,
                        "snippet": snippet,
                        "project": project,
                        "peer": peer,
                        "time": ts,
                        "source": "peers",
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        }

        fn upsert_project(conn: &Connection, peer: &str, p: &PullProject) -> Result<()> {
            conn.execute(
                "INSERT INTO projects (path, name, peer) VALUES (?1, ?2, ?3)
                 ON CONFLICT(path) DO UPDATE SET name = excluded.name",
                params![p.path, p.name, peer],
            )?;
            Ok(())
        }

        fn upsert_session(conn: &Connection, peer: &str, s: &PullSession) -> Result<()> {
            let project_id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM projects WHERE path = ?1",
                    params![s.project_path],
                    |row| row.get(0),
                )
                .ok();
            let Some(project_id) = project_id else {
                return Ok(());
            };
            conn.execute(
                "INSERT INTO sessions
                   (session_id, project_id, peer, message_count, model, cwd, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(session_id) DO UPDATE SET
                   message_count = excluded.message_count,
                   updated_at    = excluded.updated_at",
                params![
                    s.session_id, project_id, peer, s.message_count,
                    s.model, s.cwd, s.created_at, s.updated_at
                ],
            )?;
            Ok(())
        }
    }
}
