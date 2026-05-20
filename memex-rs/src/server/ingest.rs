//! Ingest - 接收客户端 push 数据，写入 server 端 DB
//!
//! 复用 ai-cli-session-db 的标准 schema，server 只额外加 pushed_by 字段。
//! 去重依赖自然键：uuid (messages), session_id (sessions), path (projects)。

use axum::{
    extract::{ws, Path, Query, State, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use rusqlite::{params, Connection};
use std::sync::Arc;
use tracing::{debug, error, info};

use ai_cli_session_db::schema;
use ai_cli_session_db::sync::{
    SyncAck, SyncBatch, SyncChainNode, SyncContinuationChain, SyncFrame, SyncMessage, SyncProject,
    SyncPushRequest, SyncPushResponse, SyncSession, SyncSessionRelation, SyncTalk,
};

use crate::auth::{AuthState, AuthenticatedUser};

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

        conn.execute_batch(schema::TABLES_SQL)?;
        conn.execute_batch(schema::INDEXES_SQL)?;
        conn.execute_batch(schema::FTS_SCHEMA_SQL)?;

        Self::server_migrations(&conn)?;

        info!("server DB 已初始化: {db_path}");
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn checkpoint(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    fn server_migrations(conn: &Connection) -> anyhow::Result<()> {
        let has_pushed_by: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('sessions') WHERE name='pushed_by'")?
            .exists([])?;

        if !has_pushed_by {
            conn.execute_batch("ALTER TABLE sessions ADD COLUMN pushed_by TEXT;")?;
        }

        let has_repo_url: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('projects') WHERE name='repo_url'")?
            .exists([])?;

        if !has_repo_url {
            conn.execute_batch("ALTER TABLE projects ADD COLUMN repo_url TEXT;")?;
        }

        // talks_fts backfill: 填充已有 talks 数据到 FTS 索引
        let talks_fts_exists: bool = conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='talks_fts'")
            .and_then(|mut s| s.exists([]))
            .unwrap_or(false);
        if talks_fts_exists {
            let fts_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM talks_fts", [], |row| row.get(0))
                .unwrap_or(0);
            let talks_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM talks", [], |row| row.get(0))
                .unwrap_or(0);
            if fts_count == 0 && talks_count > 0 {
                info!(
                    "backfilling talks_fts with {} existing records",
                    talks_count
                );
                conn.execute_batch(
                    "INSERT INTO talks_fts(rowid, summary_l2) SELECT id, summary_l2 FROM talks;",
                )?;
            }
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sync_devices (
                device_id          TEXT PRIMARY KEY,
                username           TEXT NOT NULL,
                last_heartbeat_at  INTEGER NOT NULL DEFAULT 0,
                last_db_updated_at INTEGER NOT NULL DEFAULT 0,
                connected_since    INTEGER NOT NULL DEFAULT 0
            );",
        )?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS registered_users (
                username   TEXT PRIMARY KEY,
                api_key    TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000)
            );",
        )?;

        Ok(())
    }

    fn ingest_batch(&self, pushed_by: &str, batch: &SyncBatch) -> IngestResult {
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
            if let Err(e) = Self::upsert_project(&tx, project) {
                debug!("upsert project 失败: {e}");
            }
        }

        for session in &batch.sessions {
            if let Err(e) = Self::upsert_session(&tx, pushed_by, session) {
                debug!("upsert session 失败: {e}");
            }
        }

        for msg in &batch.messages {
            match Self::insert_message(&tx, msg) {
                Ok(true) => result.accepted += 1,
                Ok(false) => result.skipped += 1,
                Err(e) => {
                    debug!("insert message 失败: {e}");
                    result.skipped += 1;
                }
            }
        }

        for talk in &batch.talks {
            let _ = Self::upsert_talk(&tx, talk);
        }

        for rel in &batch.session_relations {
            let _ = Self::insert_relation(&tx, rel);
        }

        for chain in &batch.continuation_chains {
            let _ = Self::insert_chain(&tx, chain);
        }

        for node in &batch.chain_nodes {
            let _ = Self::insert_chain_node(&tx, node);
        }

        if let Err(e) = tx.commit() {
            error!("提交事务失败: {e}");
            return IngestResult::default();
        }

        result
    }

    fn upsert_project(conn: &Connection, p: &SyncProject) -> rusqlite::Result<()> {
        conn.execute(
            r#"INSERT INTO projects (path, name, source, repo_url)
               VALUES (?1, ?2, ?3, ?4)
               ON CONFLICT(path) DO UPDATE SET
                   name = excluded.name,
                   repo_url = COALESCE(excluded.repo_url, projects.repo_url),
                   updated_at = (strftime('%s','now')*1000)"#,
            params![p.path, p.name, p.source, p.repo_url],
        )?;
        Ok(())
    }

    fn upsert_session(conn: &Connection, pushed_by: &str, s: &SyncSession) -> rusqlite::Result<()> {
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
            r#"INSERT INTO sessions (session_id, project_id, cwd, model, channel,
                   message_count, last_message_at, meta, pushed_by, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
               ON CONFLICT(session_id) DO UPDATE SET
                   message_count = excluded.message_count,
                   last_message_at = COALESCE(excluded.last_message_at, sessions.last_message_at),
                   pushed_by = excluded.pushed_by,
                   updated_at = excluded.updated_at"#,
            params![
                s.session_id,
                project_id,
                s.cwd,
                s.model,
                s.channel,
                s.message_count,
                s.last_message_at,
                s.meta,
                pushed_by,
                s.created_at,
                s.updated_at
            ],
        )?;
        Ok(())
    }

    fn insert_message(conn: &Connection, m: &SyncMessage) -> rusqlite::Result<bool> {
        let result = conn.execute(
            r#"INSERT OR IGNORE INTO messages
               (session_id, uuid, type, content_text, content_full, timestamp, sequence,
                source, channel, model, tool_call_id, tool_name, tool_args, raw,
                approval_status, approval_resolved_at,
                input_tokens, output_tokens, cache_read_input_tokens, cache_creation_input_tokens)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)"#,
            params![
                m.session_id, m.uuid, m.msg_type,
                m.content_text, m.content_full, m.timestamp, m.sequence,
                m.source, m.channel, m.model,
                m.tool_call_id, m.tool_name, m.tool_args, m.raw,
                m.approval_status, m.approval_resolved_at,
                m.input_tokens, m.output_tokens, m.cache_read_input_tokens, m.cache_creation_input_tokens
            ],
        )?;
        Ok(result > 0)
    }

    fn upsert_talk(conn: &Connection, t: &SyncTalk) -> rusqlite::Result<()> {
        conn.execute(
            r#"INSERT INTO talks (session_id, talk_id, summary_l2, summary_l3)
               VALUES (?1, ?2, ?3, ?4)
               ON CONFLICT(session_id, talk_id) DO UPDATE SET
                   summary_l2 = excluded.summary_l2,
                   summary_l3 = COALESCE(excluded.summary_l3, talks.summary_l3),
                   updated_at = (strftime('%s','now')*1000)"#,
            params![t.session_id, t.talk_id, t.summary_l2, t.summary_l3],
        )?;
        Ok(())
    }

    fn insert_relation(conn: &Connection, r: &SyncSessionRelation) -> rusqlite::Result<()> {
        conn.execute(
            r#"INSERT OR IGNORE INTO session_relations
               (parent_session_id, child_session_id, relation_type, source)
               VALUES (?1, ?2, ?3, ?4)"#,
            params![
                r.parent_session_id,
                r.child_session_id,
                r.relation_type,
                r.source
            ],
        )?;
        Ok(())
    }

    fn insert_chain(conn: &Connection, c: &SyncContinuationChain) -> rusqlite::Result<()> {
        conn.execute(
            r#"INSERT OR IGNORE INTO continuation_chains (chain_id, root_session_id)
               VALUES (?1, ?2)"#,
            params![c.chain_id, c.root_session_id],
        )?;
        Ok(())
    }

    fn insert_chain_node(conn: &Connection, n: &SyncChainNode) -> rusqlite::Result<()> {
        conn.execute(
            r#"INSERT OR IGNORE INTO continuation_chain_nodes
               (session_id, chain_id, prev_session_id, depth)
               VALUES (?1, ?2, ?3, ?4)"#,
            params![n.session_id, n.chain_id, n.prev_session_id, n.depth],
        )?;
        Ok(())
    }

    pub fn update_device_heartbeat(
        &self,
        device_id: &str,
        username: &str,
        last_db_updated_at: i64,
    ) {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().timestamp_millis();
        let _ = conn.execute(
            "INSERT INTO sync_devices (device_id, username, last_heartbeat_at, last_db_updated_at, connected_since)
             VALUES (?1, ?2, ?3, ?4, ?3)
             ON CONFLICT(device_id) DO UPDATE SET
                 last_heartbeat_at = excluded.last_heartbeat_at,
                 last_db_updated_at = excluded.last_db_updated_at",
            params![device_id, username, now, last_db_updated_at],
        );
    }

    pub fn register_user(&self, name: &str) -> Option<String> {
        let conn = self.conn.lock();
        let existing: Option<String> = conn
            .query_row(
                "SELECT api_key FROM registered_users WHERE username = ?1",
                params![name],
                |row| row.get(0),
            )
            .ok();

        if let Some(key) = existing {
            return Some(key);
        }

        let api_key = format!("mk_{}", uuid::Uuid::new_v4());
        match conn.execute(
            "INSERT OR IGNORE INTO registered_users (username, api_key) VALUES (?1, ?2)",
            params![name, api_key],
        ) {
            Ok(_) => Some(api_key),
            Err(e) => {
                error!("register user failed: {e}");
                None
            }
        }
    }

    pub fn lookup_registered_user_by_key(&self, api_key: &str) -> Option<String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT username FROM registered_users WHERE api_key = ?1",
            params![api_key],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn query_peers(&self) -> Vec<serde_json::Value> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now();
        let today_start = now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        let hour_ago = now.timestamp_millis() - 3_600_000;

        let mut stmt = match conn.prepare(
            r#"
            SELECT
                s.pushed_by,
                COUNT(DISTINCT s.session_id),
                COALESCE(SUM(s.message_count), 0),
                MAX(s.updated_at),
                COUNT(DISTINCT CASE WHEN s.updated_at >= ?1 THEN s.session_id END),
                COALESCE(
                    NULLIF(
                        (SELECT json_group_array(name) FROM (
                            SELECT DISTINCT p.name
                            FROM sessions s2
                            JOIN projects p ON p.id = s2.project_id
                            WHERE s2.pushed_by = s.pushed_by
                              AND s2.updated_at >= ?2
                            ORDER BY s2.updated_at DESC
                            LIMIT 3
                        )),
                        '[]'
                    ),
                    (SELECT json_group_array(name) FROM (
                        SELECT p.name
                        FROM sessions s2
                        JOIN projects p ON p.id = s2.project_id
                        WHERE s2.pushed_by = s.pushed_by
                        ORDER BY s2.updated_at DESC
                        LIMIT 1
                    ))
                )
            FROM sessions s
            WHERE s.pushed_by IS NOT NULL
            GROUP BY s.pushed_by
            ORDER BY MAX(s.updated_at) DESC
            "#,
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let rows = stmt
            .query_map(params![today_start, hour_ago], |row| {
                let projects_json: String = row.get(5)?;
                let active_projects: Vec<String> =
                    serde_json::from_str(&projects_json).unwrap_or_default();
                Ok(serde_json::json!({
                    "name": row.get::<_, String>(0)?,
                    "sessions": row.get::<_, i64>(1)?,
                    "messages": row.get::<_, i64>(2)?,
                    "last_active_at": row.get::<_, Option<i64>>(3)?,
                    "today_sessions": row.get::<_, i64>(4)?,
                    "active_project": active_projects.first(),
                    "active_projects": active_projects
                }))
            })
            .ok();

        rows.map(|r| r.filter_map(|v| v.ok()).collect())
            .unwrap_or_default()
    }

    pub fn query_peer_sessions(
        &self,
        name: &str,
        date: Option<&str>,
        limit: usize,
    ) -> serde_json::Value {
        let conn = self.conn.lock();

        let (date_start, date_end) = if let Some(d) = date {
            let start = format!("{d}T00:00:00");
            let end = format!("{d}T23:59:59");
            let start_ms = chrono::NaiveDateTime::parse_from_str(&start, "%Y-%m-%dT%H:%M:%S")
                .map(|dt| dt.and_utc().timestamp_millis())
                .unwrap_or(0);
            let end_ms = chrono::NaiveDateTime::parse_from_str(&end, "%Y-%m-%dT%H:%M:%S")
                .map(|dt| dt.and_utc().timestamp_millis())
                .unwrap_or(i64::MAX);
            (start_ms, end_ms)
        } else {
            let now = chrono::Utc::now();
            let today_start = now
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis();
            (today_start, now.timestamp_millis())
        };

        let limit = limit.min(200);

        let mut stmt = match conn.prepare(
            r#"
            SELECT
                s.session_id,
                p.name AS project,
                s.model,
                s.message_count,
                s.created_at,
                s.updated_at
            FROM sessions s
            JOIN projects p ON p.id = s.project_id
            WHERE s.pushed_by = ?1
              AND s.updated_at >= ?2
              AND s.updated_at <= ?3
            ORDER BY s.updated_at DESC
            LIMIT ?4
            "#,
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("query_peer_sessions prepare failed: {e}");
                return serde_json::json!({"sessions": [], "total": 0});
            }
        };

        let rows: Vec<serde_json::Value> = stmt
            .query_map(
                params![name, date_start, date_end, limit as i64],
                |row| {
                    Ok(serde_json::json!({
                        "session_id": row.get::<_, String>(0)?,
                        "project": row.get::<_, String>(1)?,
                        "model": row.get::<_, Option<String>>(2)?,
                        "message_count": row.get::<_, i64>(3)?,
                        "created_at": row.get::<_, i64>(4)?,
                        "updated_at": row.get::<_, i64>(5)?,
                    }))
                },
            )
            .ok()
            .map(|r| r.filter_map(|v| v.ok()).collect())
            .unwrap_or_default();

        let total = rows.len();
        serde_json::json!({
            "peer": name,
            "date": date.unwrap_or("today"),
            "sessions": rows,
            "total": total,
        })
    }

    pub fn query_peer_timeline(
        &self,
        name: &str,
        date: Option<&str>,
    ) -> serde_json::Value {
        let conn = self.conn.lock();

        let (date_start, date_end) = if let Some(d) = date {
            let start = format!("{d}T00:00:00");
            let end = format!("{d}T23:59:59");
            let start_ms = chrono::NaiveDateTime::parse_from_str(&start, "%Y-%m-%dT%H:%M:%S")
                .map(|dt| dt.and_utc().timestamp_millis())
                .unwrap_or(0);
            let end_ms = chrono::NaiveDateTime::parse_from_str(&end, "%Y-%m-%dT%H:%M:%S")
                .map(|dt| dt.and_utc().timestamp_millis())
                .unwrap_or(i64::MAX);
            (start_ms, end_ms)
        } else {
            let now = chrono::Utc::now();
            let today_start = now
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis();
            (today_start, now.timestamp_millis())
        };

        // 按小时聚合消息数
        let mut stmt = match conn.prepare(
            r#"
            SELECT
                CAST((m.timestamp / 1000 / 3600) % 24 AS INTEGER) AS hour,
                COUNT(*) AS count,
                COUNT(DISTINCT m.session_id) AS sessions,
                MIN(m.timestamp) AS first_msg,
                MAX(m.timestamp) AS last_msg
            FROM messages m
            JOIN sessions s ON s.session_id = m.session_id
            WHERE s.pushed_by = ?1
              AND m.timestamp >= ?2
              AND m.timestamp <= ?3
            GROUP BY hour
            ORDER BY hour
            "#,
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("query_peer_timeline prepare failed: {e}");
                return serde_json::json!({"hours": [], "total_messages": 0});
            }
        };

        let hours: Vec<serde_json::Value> = stmt
            .query_map(params![name, date_start, date_end], |row| {
                Ok(serde_json::json!({
                    "hour": row.get::<_, i64>(0)?,
                    "messages": row.get::<_, i64>(1)?,
                    "sessions": row.get::<_, i64>(2)?,
                    "first_at": row.get::<_, i64>(3)?,
                    "last_at": row.get::<_, i64>(4)?,
                }))
            })
            .ok()
            .map(|r| r.filter_map(|v| v.ok()).collect())
            .unwrap_or_default();

        let total_messages: i64 = hours
            .iter()
            .map(|h| h["messages"].as_i64().unwrap_or(0))
            .sum();
        let active_hours = hours.len();

        // 计算工作时段
        let first_active = hours.first().and_then(|h| h["first_at"].as_i64());
        let last_active = hours.last().and_then(|h| h["last_at"].as_i64());

        serde_json::json!({
            "peer": name,
            "date": date.unwrap_or("today"),
            "hours": hours,
            "total_messages": total_messages,
            "active_hours": active_hours,
            "first_active_at": first_active,
            "last_active_at": last_active,
        })
    }

    pub fn query_session_messages(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
        order: &str,
    ) -> serde_json::Value {
        let conn = self.conn.lock();

        let full_session_id: Option<String> = conn
            .prepare("SELECT session_id FROM sessions WHERE session_id LIKE ?1 || '%' LIMIT 1")
            .ok()
            .and_then(|mut s| s.query_row(params![session_id], |row| row.get(0)).ok());

        let Some(sid) = full_session_id else {
            return serde_json::json!({"error": "session not found", "session_id": session_id});
        };

        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                params![sid],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let sql = if order == "desc" {
            "SELECT uuid, type, CASE WHEN content_text = '' THEN content_full ELSE content_text END, timestamp, sequence
             FROM messages WHERE session_id = ?1
             ORDER BY sequence DESC LIMIT ?2 OFFSET ?3"
        } else {
            "SELECT uuid, type, CASE WHEN content_text = '' THEN content_full ELSE content_text END, timestamp, sequence
             FROM messages WHERE session_id = ?1
             ORDER BY sequence ASC LIMIT ?2 OFFSET ?3"
        };

        let limit = limit.min(200);

        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => {
                error!("query_session_messages prepare failed: {e}");
                return serde_json::json!({"messages": [], "total": 0});
            }
        };

        let messages: Vec<serde_json::Value> = stmt
            .query_map(params![sid, limit as i64, offset as i64], |row| {
                Ok(serde_json::json!({
                    "uuid": row.get::<_, String>(0)?,
                    "role": row.get::<_, String>(1)?,
                    "text": row.get::<_, String>(2)?,
                    "timestamp": row.get::<_, i64>(3)?,
                    "sequence": row.get::<_, i64>(4)?,
                }))
            })
            .ok()
            .map(|r| r.filter_map(|v| v.ok()).collect())
            .unwrap_or_default();

        let has_more = (offset + messages.len()) < total as usize;

        // session meta
        let session_meta: Option<serde_json::Value> = conn
            .prepare(
                "SELECT s.session_id, p.name, s.model, s.message_count, s.created_at, s.updated_at, s.pushed_by
                 FROM sessions s JOIN projects p ON p.id = s.project_id
                 WHERE s.session_id = ?1",
            )
            .ok()
            .and_then(|mut s| {
                s.query_row(params![sid], |row| {
                    Ok(serde_json::json!({
                        "session_id": row.get::<_, String>(0)?,
                        "project": row.get::<_, String>(1)?,
                        "model": row.get::<_, Option<String>>(2)?,
                        "message_count": row.get::<_, i64>(3)?,
                        "created_at": row.get::<_, i64>(4)?,
                        "updated_at": row.get::<_, i64>(5)?,
                        "pushed_by": row.get::<_, Option<String>>(6)?,
                    }))
                })
                .ok()
            });

        serde_json::json!({
            "session": session_meta,
            "messages": messages,
            "total": total,
            "pagination": {
                "offset": offset,
                "limit": limit,
                "has_more": has_more,
            }
        })
    }

    pub fn mark_device_connected(&self, device_id: &str, username: &str) {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().timestamp_millis();
        let _ = conn.execute(
            "INSERT INTO sync_devices (device_id, username, last_heartbeat_at, last_db_updated_at, connected_since)
             VALUES (?1, ?2, ?3, 0, ?3)
             ON CONFLICT(device_id) DO UPDATE SET
                 connected_since = excluded.connected_since,
                 last_heartbeat_at = excluded.last_heartbeat_at,
                 username = excluded.username",
            params![device_id, username, now],
        );
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
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<SyncPushRequest>,
) -> impl IntoResponse {
    let msg_count = request.batch.messages.len();
    info!("收到 push: user={}, messages={}", user.name, msg_count);

    let result = state.ingest_batch(&user.name, &request.batch);

    info!(
        "ingest 完成: accepted={}, skipped={}",
        result.accepted, result.skipped
    );

    let response = SyncPushResponse {
        accepted: result.accepted,
        skipped: result.skipped,
        server_cursors: request.cursors,
    };

    (StatusCode::OK, Json(response))
}

async fn handle_peers(
    State(state): State<Arc<IngestState>>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> impl IntoResponse {
    let peers = state.query_peers();
    (StatusCode::OK, Json(peers))
}

async fn handle_ws_upgrade(
    State(state): State<Arc<IngestState>>,
    auth_state: Option<Extension<Arc<AuthState>>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let auth = auth_state.map(|Extension(a)| a);
    ws.on_upgrade(move |socket| handle_ws_connection(socket, state, auth))
}

async fn handle_ws_connection(
    mut socket: ws::WebSocket,
    state: Arc<IngestState>,
    auth_state: Option<Arc<AuthState>>,
) {
    let (device_id, username) = match ws_authenticate(&mut socket, &auth_state).await {
        Some(pair) => pair,
        None => return,
    };

    info!(
        "ws connected: device={}, user={}",
        &device_id[..8.min(device_id.len())],
        username
    );
    state.mark_device_connected(&device_id, &username);

    while let Some(Ok(msg)) = socket.recv().await {
        let text = match msg {
            ws::Message::Text(t) => t,
            ws::Message::Close(_) => break,
            _ => continue,
        };

        let frame: SyncFrame = match serde_json::from_str(&text) {
            Ok(f) => f,
            Err(e) => {
                let ack = SyncAck::PushFail {
                    message: format!("invalid frame: {e}"),
                };
                let _ = socket
                    .send(ws::Message::Text(
                        serde_json::to_string(&ack).unwrap().into(),
                    ))
                    .await;
                continue;
            }
        };

        let ack = match frame {
            SyncFrame::Auth { .. } => SyncAck::AuthFail {
                message: "already authenticated".to_string(),
            },
            SyncFrame::Push { batch, cursors } => {
                let msg_count = batch.messages.len();
                debug!(
                    "ws push: device={}, messages={}",
                    &device_id[..8.min(device_id.len())],
                    msg_count
                );
                let result = state.ingest_batch(&username, &batch);
                SyncAck::PushOk {
                    accepted: result.accepted,
                    skipped: result.skipped,
                    server_cursors: cursors,
                }
            }
            SyncFrame::Heartbeat { last_db_updated_at } => {
                state.update_device_heartbeat(&device_id, &username, last_db_updated_at);
                debug!(
                    "ws heartbeat: device={}, last_db_updated_at={}",
                    &device_id[..8.min(device_id.len())],
                    last_db_updated_at
                );
                SyncAck::HeartbeatOk
            }
        };

        if socket
            .send(ws::Message::Text(
                serde_json::to_string(&ack).unwrap().into(),
            ))
            .await
            .is_err()
        {
            break;
        }
    }

    info!(
        "ws disconnected: device={}",
        &device_id[..8.min(device_id.len())]
    );
}

async fn ws_authenticate(
    socket: &mut ws::WebSocket,
    auth_state: &Option<Arc<AuthState>>,
) -> Option<(String, String)> {
    let msg = tokio::time::timeout(std::time::Duration::from_secs(10), socket.recv())
        .await
        .ok()??
        .ok()?;

    let text = match msg {
        ws::Message::Text(t) => t,
        _ => {
            let ack = SyncAck::AuthFail {
                message: "expected text frame".to_string(),
            };
            let _ = socket
                .send(ws::Message::Text(
                    serde_json::to_string(&ack).unwrap().into(),
                ))
                .await;
            return None;
        }
    };

    let frame: SyncFrame = match serde_json::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            let ack = SyncAck::AuthFail {
                message: format!("invalid auth frame: {e}"),
            };
            let _ = socket
                .send(ws::Message::Text(
                    serde_json::to_string(&ack).unwrap().into(),
                ))
                .await;
            return None;
        }
    };

    let (device_id, api_key) = match frame {
        SyncFrame::Auth { device_id, api_key } => (device_id, api_key),
        _ => {
            let ack = SyncAck::AuthFail {
                message: "first frame must be Auth".to_string(),
            };
            let _ = socket
                .send(ws::Message::Text(
                    serde_json::to_string(&ack).unwrap().into(),
                ))
                .await;
            return None;
        }
    };

    let username = if let Some(auth) = auth_state {
        match auth.verify(&api_key) {
            Some(name) => name.to_string(),
            None => {
                let ack = SyncAck::AuthFail {
                    message: "invalid api key".to_string(),
                };
                let _ = socket
                    .send(ws::Message::Text(
                        serde_json::to_string(&ack).unwrap().into(),
                    ))
                    .await;
                return None;
            }
        }
    } else {
        device_id.clone()
    };

    let ack = SyncAck::AuthOk {
        server_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    if socket
        .send(ws::Message::Text(
            serde_json::to_string(&ack).unwrap().into(),
        ))
        .await
        .is_err()
    {
        return None;
    }

    Some((device_id, username))
}

#[derive(Deserialize)]
struct PeerSessionsQuery {
    #[serde(default)]
    date: Option<String>,
    #[serde(default = "default_peer_limit")]
    limit: usize,
}

fn default_peer_limit() -> usize {
    50
}

#[derive(Deserialize)]
struct SessionMessagesQuery {
    #[serde(default = "default_session_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_order")]
    order: String,
}

fn default_session_limit() -> usize {
    50
}

fn default_order() -> String {
    "asc".to_string()
}

async fn handle_session_messages(
    State(state): State<Arc<IngestState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Query(params): Query<SessionMessagesQuery>,
) -> impl IntoResponse {
    if !user.is_admin {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "admin access required"}))).into_response();
    }
    let result = state.query_session_messages(&id, params.limit, params.offset, &params.order);
    (StatusCode::OK, Json(result)).into_response()
}

async fn handle_peer_sessions(
    State(state): State<Arc<IngestState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(name): Path<String>,
    Query(params): Query<PeerSessionsQuery>,
) -> impl IntoResponse {
    if !user.is_admin {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "admin access required"}))).into_response();
    }
    let sessions = state.query_peer_sessions(&name, params.date.as_deref(), params.limit);
    (StatusCode::OK, Json(sessions)).into_response()
}

async fn handle_peer_timeline(
    State(state): State<Arc<IngestState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(name): Path<String>,
    Query(params): Query<PeerSessionsQuery>,
) -> impl IntoResponse {
    if !user.is_admin {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "admin access required"}))).into_response();
    }
    let timeline = state.query_peer_timeline(&name, params.date.as_deref());
    (StatusCode::OK, Json(timeline)).into_response()
}

pub fn create_sync_router(state: Arc<IngestState>) -> Router {
    Router::new()
        .route("/api/sync/push", post(handle_push))
        .route("/api/sync/peers", get(handle_peers))
        .route("/api/sync/peers/{name}/sessions", get(handle_peer_sessions))
        .route("/api/sync/peers/{name}/timeline", get(handle_peer_timeline))
        .route("/api/sessions/{id}/messages", get(handle_session_messages))
        .route("/api/sync/stream", get(handle_ws_upgrade))
        .layer(axum::extract::DefaultBodyLimit::max(50 * 1024 * 1024))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_server() -> (TempDir, Arc<IngestState>) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("server.db");
        let state = Arc::new(IngestState::new(db_path.to_str().unwrap()).unwrap());
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
                    source: None,
                    channel: None,
                    model: None,
                    tool_call_id: None,
                    tool_name: None,
                    tool_args: None,
                    raw: None,
                    approval_status: None,
                    approval_resolved_at: None,
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                },
                SyncMessage {
                    uuid: "uuid-2".to_string(),
                    session_id: "sess-1".to_string(),
                    msg_type: "assistant".to_string(),
                    content_text: "world".to_string(),
                    content_full: "world".to_string(),
                    timestamp: 1001,
                    sequence: 2,
                    source: None,
                    channel: None,
                    model: None,
                    tool_call_id: None,
                    tool_name: None,
                    tool_args: None,
                    raw: None,
                    approval_status: None,
                    approval_resolved_at: None,
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
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
        let result = state.ingest_batch("alice", &batch);

        assert_eq!(result.accepted, 2);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn test_ingest_dedup() {
        let (_dir, state) = setup_server();
        let batch = make_batch();

        let r1 = state.ingest_batch("alice", &batch);
        assert_eq!(r1.accepted, 2);

        let r2 = state.ingest_batch("alice", &batch);
        assert_eq!(r2.accepted, 0);
        assert_eq!(r2.skipped, 2);
    }

    #[test]
    fn test_ingest_multi_user() {
        let (_dir, state) = setup_server();
        let batch = make_batch();
        state.ingest_batch("alice", &batch);

        let mut batch2 = make_batch();
        batch2.messages[0].uuid = "uuid-3".to_string();
        batch2.messages[1].uuid = "uuid-4".to_string();
        batch2.sessions[0].session_id = "sess-2".to_string();
        batch2.messages[0].session_id = "sess-2".to_string();
        batch2.messages[1].session_id = "sess-2".to_string();

        let r2 = state.ingest_batch("bob", &batch2);
        assert_eq!(r2.accepted, 2);
    }

    #[test]
    fn test_pushed_by() {
        let (_dir, state) = setup_server();
        let batch = make_batch();
        state.ingest_batch("alice", &batch);

        let conn = state.conn.lock();
        let pushed_by: String = conn
            .query_row("SELECT pushed_by FROM sessions LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(pushed_by, "alice");
    }

    #[test]
    fn test_fts_indexed() {
        let (_dir, state) = setup_server();
        let batch = make_batch();
        state.ingest_batch("alice", &batch);

        let conn = state.conn.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages_fts WHERE content_full MATCH 'hello'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_repo_url_preserved() {
        let (_dir, state) = setup_server();
        let batch = make_batch();
        state.ingest_batch("alice", &batch);

        let conn = state.conn.lock();
        let url: String = conn
            .query_row("SELECT repo_url FROM projects LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(url, "git@github.com:vimo-ai/ETerm.git");
    }
}
