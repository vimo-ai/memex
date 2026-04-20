//! Ingest - 接收客户端 push 数据，写入 server 端 DB
//!
//! Server 端维护独立的 ai-cli-session.db，接收所有客户端推送的数据。
//! 使用自然键去重（uuid for messages, session_id for sessions, path for projects）。

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use rusqlite::{params, Connection};
use std::sync::Arc;
use tracing::{debug, error, info};

use ai_cli_session_db::sync::{
    SyncBatch, SyncChainNode, SyncContinuationChain, SyncMessage, SyncProject,
    SyncPushRequest, SyncPushResponse, SyncSession, SyncSessionRelation, SyncTalk,
};

use parking_lot::Mutex;

pub struct IngestState {
    conn: Arc<Mutex<Connection>>,
}

impl IngestState {
    pub fn new(db_path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(db_path)?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=10000;",
        )?;

        Self::ensure_schema(&conn)?;

        info!("server ingest DB 已初始化: {db_path}");
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn checkpoint(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    fn ensure_schema(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS projects (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT NOT NULL,
                name TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'claude',
                repo_url TEXT,
                device_id TEXT,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000),
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000),
                UNIQUE(path, device_id)
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                project_id INTEGER NOT NULL REFERENCES projects(id),
                device_id TEXT,
                cwd TEXT,
                model TEXT,
                channel TEXT,
                message_count INTEGER NOT NULL DEFAULT 0,
                last_message_at INTEGER,
                session_type TEXT,
                source TEXT,
                meta TEXT,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000),
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000),
                UNIQUE(session_id, device_id)
            );

            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                uuid TEXT NOT NULL UNIQUE,
                type TEXT NOT NULL,
                content_text TEXT NOT NULL,
                content_full TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                sequence INTEGER NOT NULL,
                source TEXT,
                channel TEXT,
                model TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                tool_args TEXT,
                raw TEXT,
                approval_status TEXT,
                approval_resolved_at INTEGER,
                device_id TEXT
            );

            CREATE TABLE IF NOT EXISTS talks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                talk_id TEXT NOT NULL,
                summary_l2 TEXT NOT NULL,
                summary_l3 TEXT,
                device_id TEXT,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000),
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000),
                UNIQUE(session_id, talk_id, device_id)
            );

            CREATE TABLE IF NOT EXISTS session_relations (
                parent_session_id TEXT NOT NULL,
                child_session_id TEXT NOT NULL,
                relation_type TEXT NOT NULL,
                source TEXT NOT NULL,
                device_id TEXT,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000),
                PRIMARY KEY (parent_session_id, child_session_id, relation_type)
            );

            CREATE TABLE IF NOT EXISTS continuation_chains (
                chain_id TEXT PRIMARY KEY,
                root_session_id TEXT NOT NULL,
                device_id TEXT,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000)
            );

            CREATE TABLE IF NOT EXISTS continuation_chain_nodes (
                session_id TEXT PRIMARY KEY,
                chain_id TEXT NOT NULL,
                prev_session_id TEXT,
                depth INTEGER NOT NULL DEFAULT 0,
                device_id TEXT,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000)
            );

            CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT NOT NULL UNIQUE,
                api_key_hash TEXT NOT NULL,
                display_name TEXT,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000),
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000)
            );

            CREATE TABLE IF NOT EXISTS project_access (
                user_id INTEGER NOT NULL REFERENCES users(id),
                project_id INTEGER NOT NULL REFERENCES projects(id),
                role TEXT NOT NULL DEFAULT 'member',
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000),
                PRIMARY KEY (user_id, project_id)
            );

            -- indexes
            CREATE INDEX IF NOT EXISTS idx_srv_messages_uuid ON messages(uuid);
            CREATE INDEX IF NOT EXISTS idx_srv_messages_session ON messages(session_id);
            CREATE INDEX IF NOT EXISTS idx_srv_sessions_session_id ON sessions(session_id);
            CREATE INDEX IF NOT EXISTS idx_srv_projects_repo_url ON projects(repo_url);
            "#,
        )?;

        // FTS for server-side search
        conn.execute_batch(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                content_full,
                content='messages',
                content_rowid='id',
                tokenize='unicode61'
            );

            CREATE TRIGGER IF NOT EXISTS srv_messages_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, content_full) VALUES (new.id, new.content_full);
            END;
            "#,
        )?;

        Ok(())
    }

    fn ingest_batch(&self, device_id: &str, batch: &SyncBatch) -> IngestResult {
        let conn = self.conn.lock();
        let mut result = IngestResult::default();

        let tx = match conn.unchecked_transaction() {
            Ok(tx) => tx,
            Err(e) => {
                error!("开启事务失败: {e}");
                return result;
            }
        };

        for project in &batch.projects {
            if let Err(e) = Self::upsert_project(&tx, device_id, project) {
                debug!("upsert project 失败: {e}");
            }
        }

        for session in &batch.sessions {
            if let Err(e) = Self::upsert_session(&tx, device_id, session) {
                debug!("upsert session 失败: {e}");
            }
        }

        for msg in &batch.messages {
            match Self::insert_message(&tx, device_id, msg) {
                Ok(true) => result.accepted += 1,
                Ok(false) => result.skipped += 1,
                Err(e) => {
                    debug!("insert message 失败: {e}");
                    result.skipped += 1;
                }
            }
        }

        for talk in &batch.talks {
            let _ = Self::upsert_talk(&tx, device_id, talk);
        }

        for rel in &batch.session_relations {
            let _ = Self::insert_relation(&tx, device_id, rel);
        }

        for chain in &batch.continuation_chains {
            let _ = Self::insert_chain(&tx, device_id, chain);
        }

        for node in &batch.chain_nodes {
            let _ = Self::insert_chain_node(&tx, device_id, node);
        }

        if let Err(e) = tx.commit() {
            error!("提交事务失败: {e}");
            return IngestResult::default();
        }

        result
    }

    fn upsert_project(conn: &Connection, device_id: &str, p: &SyncProject) -> rusqlite::Result<()> {
        conn.execute(
            r#"INSERT INTO projects (path, name, source, repo_url, device_id)
               VALUES (?1, ?2, ?3, ?4, ?5)
               ON CONFLICT(path, device_id) DO UPDATE SET
                   name = excluded.name,
                   repo_url = COALESCE(excluded.repo_url, projects.repo_url),
                   updated_at = (strftime('%s','now')*1000)"#,
            params![p.path, p.name, p.source, p.repo_url, device_id],
        )?;
        Ok(())
    }

    fn upsert_session(conn: &Connection, device_id: &str, s: &SyncSession) -> rusqlite::Result<()> {
        let project_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM projects WHERE path = ?1 AND device_id = ?2",
                params![s.project_path, device_id],
                |row| row.get(0),
            )
            .ok();

        let Some(project_id) = project_id else {
            return Ok(());
        };

        conn.execute(
            r#"INSERT INTO sessions (session_id, project_id, device_id, cwd, model, channel,
                   message_count, last_message_at, session_type, source, meta, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
               ON CONFLICT(session_id, device_id) DO UPDATE SET
                   message_count = excluded.message_count,
                   last_message_at = COALESCE(excluded.last_message_at, sessions.last_message_at),
                   updated_at = excluded.updated_at"#,
            params![
                s.session_id, project_id, device_id,
                s.cwd, s.model, s.channel,
                s.message_count, s.last_message_at,
                s.session_type, s.source, s.meta,
                s.created_at, s.updated_at
            ],
        )?;
        Ok(())
    }

    fn insert_message(conn: &Connection, device_id: &str, m: &SyncMessage) -> rusqlite::Result<bool> {
        let result = conn.execute(
            r#"INSERT OR IGNORE INTO messages
               (session_id, uuid, type, content_text, content_full, timestamp, sequence,
                source, channel, model, tool_call_id, tool_name, tool_args, raw,
                approval_status, approval_resolved_at, device_id)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"#,
            params![
                m.session_id, m.uuid, m.msg_type,
                m.content_text, m.content_full, m.timestamp, m.sequence,
                m.source, m.channel, m.model,
                m.tool_call_id, m.tool_name, m.tool_args, m.raw,
                m.approval_status, m.approval_resolved_at, device_id
            ],
        )?;
        Ok(result > 0)
    }

    fn upsert_talk(conn: &Connection, device_id: &str, t: &SyncTalk) -> rusqlite::Result<()> {
        conn.execute(
            r#"INSERT INTO talks (session_id, talk_id, summary_l2, summary_l3, device_id)
               VALUES (?1, ?2, ?3, ?4, ?5)
               ON CONFLICT(session_id, talk_id, device_id) DO UPDATE SET
                   summary_l2 = excluded.summary_l2,
                   summary_l3 = COALESCE(excluded.summary_l3, talks.summary_l3),
                   updated_at = (strftime('%s','now')*1000)"#,
            params![t.session_id, t.talk_id, t.summary_l2, t.summary_l3, device_id],
        )?;
        Ok(())
    }

    fn insert_relation(conn: &Connection, device_id: &str, r: &SyncSessionRelation) -> rusqlite::Result<()> {
        conn.execute(
            r#"INSERT OR IGNORE INTO session_relations
               (parent_session_id, child_session_id, relation_type, source, device_id)
               VALUES (?1, ?2, ?3, ?4, ?5)"#,
            params![r.parent_session_id, r.child_session_id, r.relation_type, r.source, device_id],
        )?;
        Ok(())
    }

    fn insert_chain(conn: &Connection, device_id: &str, c: &SyncContinuationChain) -> rusqlite::Result<()> {
        conn.execute(
            r#"INSERT OR IGNORE INTO continuation_chains (chain_id, root_session_id, device_id)
               VALUES (?1, ?2, ?3)"#,
            params![c.chain_id, c.root_session_id, device_id],
        )?;
        Ok(())
    }

    fn insert_chain_node(conn: &Connection, device_id: &str, n: &SyncChainNode) -> rusqlite::Result<()> {
        conn.execute(
            r#"INSERT OR IGNORE INTO continuation_chain_nodes
               (session_id, chain_id, prev_session_id, depth, device_id)
               VALUES (?1, ?2, ?3, ?4, ?5)"#,
            params![n.session_id, n.chain_id, n.prev_session_id, n.depth, device_id],
        )?;
        Ok(())
    }
}

#[derive(Debug, Default)]
struct IngestResult {
    accepted: u64,
    skipped: u64,
}

// ==================== HTTP handlers ====================

async fn handle_push(
    State(state): State<Arc<IngestState>>,
    Json(request): Json<SyncPushRequest>,
) -> impl IntoResponse {
    let msg_count = request.batch.messages.len();
    info!(
        "收到 push: device={}, messages={}",
        &request.device_id[..8.min(request.device_id.len())],
        msg_count
    );

    let result = state.ingest_batch(&request.device_id, &request.batch);

    info!("ingest 完成: accepted={}, skipped={}", result.accepted, result.skipped);

    let response = SyncPushResponse {
        accepted: result.accepted,
        skipped: result.skipped,
        server_cursors: request.cursors,
    };

    (StatusCode::OK, Json(response))
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status": "ok", "mode": "server"})))
}

pub fn create_sync_router(state: Arc<IngestState>) -> Router {
    Router::new()
        .route("/api/sync/push", post(handle_push))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_cli_session_db::sync::SyncCursor;
    use tempfile::TempDir;

    fn setup_server() -> (TempDir, Arc<IngestState>) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("server.db");
        let state = Arc::new(
            IngestState::new(db_path.to_str().unwrap()).unwrap(),
        );
        (dir, state)
    }

    fn make_batch() -> SyncBatch {
        SyncBatch {
            projects: vec![SyncProject {
                path: "/Users/alice/code/ETerm".to_string(),
                name: "ETerm".to_string(),
                source: "claude".to_string(),
                repo_url: Some("git@github.com:vimo-ai/ETerm.git".to_string()),
            }],
            sessions: vec![SyncSession {
                session_id: "sess-1".to_string(),
                project_path: "/Users/alice/code/ETerm".to_string(),
                cwd: Some("/Users/alice/code/ETerm".to_string()),
                model: Some("claude-opus-4".to_string()),
                channel: None,
                message_count: 2,
                last_message_at: Some(1000),
                session_type: None,
                source: Some("claude".to_string()),
                meta: None,
                created_at: 1000,
                updated_at: 1000,
            }],
            messages: vec![
                SyncMessage {
                    uuid: "uuid-1".to_string(),
                    session_id: "sess-1".to_string(),
                    msg_type: "user".to_string(),
                    content_text: "hello".to_string(),
                    content_full: "hello".to_string(),
                    timestamp: 1000,
                    sequence: 1,
                    source: None, channel: None, model: None,
                    tool_call_id: None, tool_name: None, tool_args: None,
                    raw: None, approval_status: None, approval_resolved_at: None,
                },
                SyncMessage {
                    uuid: "uuid-2".to_string(),
                    session_id: "sess-1".to_string(),
                    msg_type: "assistant".to_string(),
                    content_text: "world".to_string(),
                    content_full: "world".to_string(),
                    timestamp: 1001,
                    sequence: 2,
                    source: None, channel: None, model: None,
                    tool_call_id: None, tool_name: None, tool_args: None,
                    raw: None, approval_status: None, approval_resolved_at: None,
                },
            ],
            session_relations: vec![],
            continuation_chains: vec![],
            chain_nodes: vec![],
            talks: vec![SyncTalk {
                session_id: "sess-1".to_string(),
                talk_id: "talk-1".to_string(),
                summary_l2: "user said hello".to_string(),
                summary_l3: None,
            }],
        }
    }

    #[test]
    fn test_ingest_batch() {
        let (_dir, state) = setup_server();
        let batch = make_batch();
        let result = state.ingest_batch("device-alice", &batch);

        assert_eq!(result.accepted, 2);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn test_ingest_dedup() {
        let (_dir, state) = setup_server();
        let batch = make_batch();

        let r1 = state.ingest_batch("device-alice", &batch);
        assert_eq!(r1.accepted, 2);

        let r2 = state.ingest_batch("device-alice", &batch);
        assert_eq!(r2.accepted, 0, "重复推送应全部跳过");
        assert_eq!(r2.skipped, 2);
    }

    #[test]
    fn test_ingest_multi_device() {
        let (_dir, state) = setup_server();
        let batch = make_batch();

        state.ingest_batch("device-alice", &batch);

        let mut batch2 = make_batch();
        batch2.messages[0].uuid = "uuid-3".to_string();
        batch2.messages[1].uuid = "uuid-4".to_string();

        let r2 = state.ingest_batch("device-bob", &batch2);
        assert_eq!(r2.accepted, 2, "不同设备的不同 uuid 应该都写入");
    }

    #[test]
    fn test_ingest_talks() {
        let (_dir, state) = setup_server();
        let batch = make_batch();
        state.ingest_batch("device-alice", &batch);

        let conn = state.conn.lock();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM talks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_fts_indexed() {
        let (_dir, state) = setup_server();
        let batch = make_batch();
        state.ingest_batch("device-alice", &batch);

        let conn = state.conn.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages_fts WHERE content_full MATCH 'hello'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "FTS 应自动索引插入的消息");
    }

    #[test]
    fn test_repo_url_preserved() {
        let (_dir, state) = setup_server();
        let batch = make_batch();
        state.ingest_batch("device-alice", &batch);

        let conn = state.conn.lock();
        let url: String = conn
            .query_row("SELECT repo_url FROM projects LIMIT 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(url, "git@github.com:vimo-ai/ETerm.git");
    }
}
