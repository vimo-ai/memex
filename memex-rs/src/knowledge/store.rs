use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

// ==================== Domain Types ====================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeNode {
    pub id: String,
    pub cluster_id: String,
    pub project_id: i64,
    pub session_id: String,
    pub day: String,
    pub topic: String,
    pub conclusion: String,
    pub node_type: String,
    pub confidence: f64,
    pub pipeline_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeCluster {
    pub id: String,
    pub project_id: i64,
    pub canonical_topic: String,
    pub l5_domain: Option<String>,
    pub current_understanding: Option<String>,
    pub evolution_narrative: Option<String>,
    pub final_confidence: Option<f64>,
    pub node_count: i64,
    pub day_count: i64,
    pub first_day: Option<String>,
    pub last_day: Option<String>,
    pub pipeline_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeRelation {
    pub from_node_id: String,
    pub to_node_id: String,
    pub relation_type: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct KnowledgeProgress {
    pub session_id: String,
    pub status: String,
    pub node_count: i64,
    pub error: Option<String>,
    pub pipeline_version: Option<String>,
}

// ==================== Store ====================

pub struct KnowledgeStore {
    conn: Arc<Mutex<Connection>>,
}

impl KnowledgeStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    pub async fn ensure_schema(&self) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch(SCHEMA_SQL)?;
        // Migrate: allow NULL cluster_id + drop FK constraints
        // (FK on cluster_id breaks atomic per-session insert)
        let col_info: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='knowledge_nodes'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_default();
        if col_info.contains("NOT NULL") && col_info.contains("cluster_id") {
            conn.execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DROP TABLE IF EXISTS knowledge_nodes_new;
                 CREATE TABLE knowledge_nodes_new (
                    id TEXT PRIMARY KEY, cluster_id TEXT, project_id INTEGER NOT NULL,
                    session_id TEXT NOT NULL, day TEXT NOT NULL, topic TEXT NOT NULL,
                    conclusion TEXT NOT NULL, node_type TEXT NOT NULL, confidence REAL NOT NULL,
                    pipeline_version TEXT, created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000)
                 );
                 INSERT INTO knowledge_nodes_new SELECT * FROM knowledge_nodes;
                 DROP TABLE knowledge_nodes;
                 ALTER TABLE knowledge_nodes_new RENAME TO knowledge_nodes;
                 CREATE INDEX IF NOT EXISTS idx_kn_cluster ON knowledge_nodes(cluster_id);
                 CREATE INDEX IF NOT EXISTS idx_kn_session ON knowledge_nodes(session_id);
                 CREATE INDEX IF NOT EXISTS idx_kn_project_day ON knowledge_nodes(project_id, day);"
            )?;
            tracing::info!("migrated knowledge_nodes: cluster_id nullable, FK constraints removed");
        }
        Ok(())
    }

    // ---- Progress ----

    pub async fn get_progress(&self, session_id: &str) -> Result<Option<KnowledgeProgress>> {
        let conn = self.conn.lock().await;
        let row = conn.query_row(
            "SELECT session_id, status, node_count, error, pipeline_version FROM knowledge_progress WHERE session_id = ?",
            params![session_id],
            |row| Ok(KnowledgeProgress {
                session_id: row.get(0)?,
                status: row.get(1)?,
                node_count: row.get(2)?,
                error: row.get(3)?,
                pipeline_version: row.get(4)?,
            }),
        ).optional()?;
        Ok(row)
    }

    pub async fn update_progress(
        &self,
        session_id: &str,
        status: &str,
        node_count: i64,
        error: Option<&str>,
        pipeline_version: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as i64;
        conn.execute(
            "INSERT OR REPLACE INTO knowledge_progress (session_id, status, node_count, error, processed_at, pipeline_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_id, status, node_count, error, now, pipeline_version],
        )?;
        Ok(())
    }

    pub async fn get_unprocessed_sessions(
        &self,
        project_ids: &[i64],
        limit: usize,
    ) -> Result<Vec<(String, i64, String)>> {
        let conn = self.conn.lock().await;
        let placeholders = project_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            r#"
            SELECT s.session_id, s.project_id,
                   date(s.created_at/1000, 'unixepoch', 'localtime') as day
            FROM sessions s
            WHERE s.project_id IN ({placeholders})
              AND s.session_id NOT LIKE 'agent-%'
              AND s.session_id NOT IN (SELECT session_id FROM knowledge_progress WHERE status != 'failed')
            ORDER BY s.created_at
            LIMIT ?
            "#,
        );
        let mut stmt = conn.prepare(&sql)?;

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = project_ids
            .iter()
            .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        param_values.push(Box::new(limit as i64));

        let rows = stmt.query_map(
            rusqlite::params_from_iter(param_values.iter().map(|b| b.as_ref())),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ---- Nodes ----

    pub async fn insert_nodes(&self, nodes: &[KnowledgeNode]) -> Result<usize> {
        let conn = self.conn.lock().await;
        let mut count = 0;
        for node in nodes {
            let cluster_id: Option<&str> = if node.cluster_id.is_empty() {
                None
            } else {
                Some(&node.cluster_id)
            };
            let inserted = conn.execute(
                r#"INSERT OR IGNORE INTO knowledge_nodes
                   (id, cluster_id, project_id, session_id, day, topic, conclusion, node_type, confidence, pipeline_version)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
                params![
                    node.id, cluster_id, node.project_id, node.session_id,
                    node.day, node.topic, node.conclusion, node.node_type,
                    node.confidence, node.pipeline_version,
                ],
            )?;
            count += inserted;
        }
        Ok(count)
    }

    pub async fn search_nodes(&self, query: &str, limit: usize) -> Result<Vec<KnowledgeNode>> {
        let conn = self.conn.lock().await;
        let pattern = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT id, cluster_id, project_id, session_id, day, topic, conclusion, node_type, confidence, pipeline_version
             FROM knowledge_nodes
             WHERE topic LIKE ?1 OR conclusion LIKE ?1
             ORDER BY confidence DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit as i64], |row| {
            Ok(KnowledgeNode {
                id: row.get(0)?,
                cluster_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                project_id: row.get(2)?,
                session_id: row.get(3)?,
                day: row.get(4)?,
                topic: row.get(5)?,
                conclusion: row.get(6)?,
                node_type: row.get(7)?,
                confidence: row.get(8)?,
                pipeline_version: row.get(9)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub async fn get_nodes_by_cluster(&self, cluster_id: &str) -> Result<Vec<KnowledgeNode>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, cluster_id, project_id, session_id, day, topic, conclusion, node_type, confidence, pipeline_version FROM knowledge_nodes WHERE cluster_id = ? ORDER BY day, session_id",
        )?;
        let rows = stmt.query_map(params![cluster_id], |row| {
            Ok(KnowledgeNode {
                id: row.get(0)?,
                cluster_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                project_id: row.get(2)?,
                session_id: row.get(3)?,
                day: row.get(4)?,
                topic: row.get(5)?,
                conclusion: row.get(6)?,
                node_type: row.get(7)?,
                confidence: row.get(8)?,
                pipeline_version: row.get(9)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub async fn get_nodes_by_sessions(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<KnowledgeNode>> {
        if session_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().await;
        let placeholders = session_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, cluster_id, project_id, session_id, day, topic, conclusion, node_type, confidence, pipeline_version FROM knowledge_nodes WHERE session_id IN ({placeholders}) ORDER BY day",
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(session_ids.iter()), |row| {
            Ok(KnowledgeNode {
                id: row.get(0)?,
                cluster_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                project_id: row.get(2)?,
                session_id: row.get(3)?,
                day: row.get(4)?,
                topic: row.get(5)?,
                conclusion: row.get(6)?,
                node_type: row.get(7)?,
                confidence: row.get(8)?,
                pipeline_version: row.get(9)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // ---- Clusters ----

    pub async fn insert_cluster(&self, cluster: &KnowledgeCluster) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            r#"INSERT OR IGNORE INTO knowledge_clusters
               (id, project_id, canonical_topic, l5_domain, current_understanding, evolution_narrative, final_confidence, node_count, day_count, first_day, last_day, pipeline_version)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
            params![
                cluster.id, cluster.project_id, cluster.canonical_topic, cluster.l5_domain,
                cluster.current_understanding, cluster.evolution_narrative, cluster.final_confidence,
                cluster.node_count, cluster.day_count, cluster.first_day, cluster.last_day,
                cluster.pipeline_version,
            ],
        )?;
        Ok(())
    }

    pub async fn load_clusters(&self, project_ids: &[i64]) -> Result<Vec<KnowledgeCluster>> {
        let conn = self.conn.lock().await;
        let placeholders = project_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, project_id, canonical_topic, l5_domain, current_understanding, evolution_narrative, final_confidence, node_count, day_count, first_day, last_day, pipeline_version FROM knowledge_clusters WHERE project_id IN ({placeholders})",
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(project_ids.iter()), |row| {
            Ok(KnowledgeCluster {
                id: row.get(0)?,
                project_id: row.get(1)?,
                canonical_topic: row.get(2)?,
                l5_domain: row.get(3)?,
                current_understanding: row.get(4)?,
                evolution_narrative: row.get(5)?,
                final_confidence: row.get(6)?,
                node_count: row.get(7)?,
                day_count: row.get(8)?,
                first_day: row.get(9)?,
                last_day: row.get(10)?,
                pipeline_version: row.get(11)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub async fn update_cluster_stats(&self, cluster_id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as i64;
        conn.execute(
            r#"UPDATE knowledge_clusters SET
               node_count = (SELECT COUNT(*) FROM knowledge_nodes WHERE cluster_id = ?1),
               day_count = (SELECT COUNT(DISTINCT day) FROM knowledge_nodes WHERE cluster_id = ?1),
               first_day = (SELECT MIN(day) FROM knowledge_nodes WHERE cluster_id = ?1),
               last_day = (SELECT MAX(day) FROM knowledge_nodes WHERE cluster_id = ?1),
               updated_at = ?2
               WHERE id = ?1"#,
            params![cluster_id, now],
        )?;
        Ok(())
    }

    pub async fn update_cluster_evolution(
        &self,
        cluster_id: &str,
        current_understanding: &str,
        evolution_narrative: &str,
        final_confidence: f64,
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as i64;
        conn.execute(
            r#"UPDATE knowledge_clusters SET
               current_understanding = ?2,
               evolution_narrative = ?3,
               final_confidence = ?4,
               updated_at = ?5
               WHERE id = ?1"#,
            params![
                cluster_id,
                current_understanding,
                evolution_narrative,
                final_confidence,
                now
            ],
        )?;
        Ok(())
    }

    pub async fn update_node_cluster(&self, node_id: &str, cluster_id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE knowledge_nodes SET cluster_id = ?1 WHERE id = ?2",
            params![cluster_id, node_id],
        )?;
        Ok(())
    }

    // ---- Relations ----

    pub async fn upsert_relations(&self, relations: &[KnowledgeRelation]) -> Result<usize> {
        let conn = self.conn.lock().await;
        let mut count = 0;
        for rel in relations {
            let inserted = conn.execute(
                r#"INSERT OR REPLACE INTO knowledge_relations
                   (from_node_id, to_node_id, relation_type, detail)
                   VALUES (?1, ?2, ?3, ?4)"#,
                params![
                    rel.from_node_id,
                    rel.to_node_id,
                    rel.relation_type,
                    rel.detail
                ],
            )?;
            count += inserted;
        }
        Ok(count)
    }

    // ---- Stats ----

    pub async fn stats(&self, project_ids: &[i64]) -> Result<(i64, i64)> {
        let conn = self.conn.lock().await;
        let placeholders = project_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");

        let nodes: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM knowledge_nodes WHERE project_id IN ({placeholders})"),
            rusqlite::params_from_iter(project_ids.iter()),
            |row| row.get(0),
        )?;

        let clusters: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM knowledge_clusters WHERE project_id IN ({placeholders})"
            ),
            rusqlite::params_from_iter(project_ids.iter()),
            |row| row.get(0),
        )?;

        Ok((nodes, clusters))
    }

    pub async fn global_stats(&self) -> Result<(i64, i64)> {
        let conn = self.conn.lock().await;
        let nodes: i64 =
            conn.query_row("SELECT COUNT(*) FROM knowledge_nodes", [], |r| r.get(0))?;
        let clusters: i64 =
            conn.query_row("SELECT COUNT(*) FROM knowledge_clusters", [], |r| r.get(0))?;
        Ok((nodes, clusters))
    }

    /// Projects with unprocessed sessions, sorted by most recently active.
    pub async fn get_pending_projects(
        &self,
        search: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PendingProject>> {
        let conn = self.conn.lock().await;

        let search_clause = if search.is_some() {
            "AND (p.name LIKE '%' || ? || '%' OR p.path LIKE '%' || ? || '%')"
        } else {
            ""
        };

        let sql = format!(
            r#"
            SELECT p.id, p.name, p.path,
                   COUNT(s.session_id) as pending_count,
                   MAX(s.created_at) as last_activity
            FROM projects p
            JOIN sessions s ON s.project_id = p.id
            WHERE s.session_id NOT LIKE 'agent-%'
              AND s.session_id NOT IN (SELECT session_id FROM knowledge_progress WHERE status != 'failed')
              {search_clause}
            GROUP BY p.id
            HAVING pending_count > 0
            ORDER BY last_activity DESC
            LIMIT ?
            "#
        );

        let mut stmt = conn.prepare(&sql)?;
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(q) = search {
            param_values.push(Box::new(q.to_string()));
            param_values.push(Box::new(q.to_string()));
        }
        param_values.push(Box::new(limit as i64));

        let mut rows = stmt.query(rusqlite::params_from_iter(
            param_values.iter().map(|b| b.as_ref()),
        ))?;

        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            result.push(PendingProject {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                pending_sessions: row.get(3)?,
                last_activity: row.get(4)?,
            });
        }
        Ok(result)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingProject {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub pending_sessions: i64,
    pub last_activity: Option<i64>,
}

// ==================== Schema ====================

impl KnowledgeStore {
    fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS projects (id INTEGER PRIMARY KEY, path TEXT);
             CREATE TABLE IF NOT EXISTS sessions (session_id TEXT PRIMARY KEY, project_id INTEGER, created_at INTEGER);
             INSERT INTO projects (id, path) VALUES (1, '/test/project');
             INSERT INTO sessions (session_id, project_id, created_at) VALUES ('sess-001', 1, 1714300000000);
             INSERT INTO sessions (session_id, project_id, created_at) VALUES ('sess-002', 1, 1714400000000);"
        ).unwrap();
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, cluster_id: &str, session_id: &str) -> KnowledgeNode {
        KnowledgeNode {
            id: id.to_string(),
            cluster_id: cluster_id.to_string(),
            project_id: 1,
            session_id: session_id.to_string(),
            day: "2026-04-29".to_string(),
            topic: "test topic".to_string(),
            conclusion: "test conclusion".to_string(),
            node_type: "discovery".to_string(),
            confidence: 0.85,
            pipeline_version: Some("v1".to_string()),
        }
    }

    fn make_cluster(id: &str) -> KnowledgeCluster {
        KnowledgeCluster {
            id: id.to_string(),
            project_id: 1,
            canonical_topic: "test cluster".to_string(),
            l5_domain: None,
            current_understanding: None,
            evolution_narrative: None,
            final_confidence: None,
            node_count: 0,
            day_count: 0,
            first_day: None,
            last_day: None,
            pipeline_version: Some("v1".to_string()),
        }
    }

    #[tokio::test]
    async fn schema_and_roundtrip() {
        let store = KnowledgeStore::new_in_memory();
        store.ensure_schema().await.unwrap();

        // insert cluster → node → read back
        let cluster = make_cluster("c1");
        store.insert_cluster(&cluster).await.unwrap();

        let node = make_node("n1", "c1", "sess-001");
        let inserted = store.insert_nodes(&[node]).await.unwrap();
        assert_eq!(inserted, 1);

        let nodes = store.get_nodes_by_cluster("c1").await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].topic, "test topic");

        // update stats
        store.update_cluster_stats("c1").await.unwrap();
        let clusters = store.load_clusters(&[1]).await.unwrap();
        assert_eq!(clusters[0].node_count, 1);
        assert_eq!(clusters[0].first_day.as_deref(), Some("2026-04-29"));

        // progress
        store
            .update_progress("sess-001", "extracted", 1, None, "v1")
            .await
            .unwrap();
        let p = store.get_progress("sess-001").await.unwrap().unwrap();
        assert_eq!(p.status, "extracted");
        assert_eq!(p.node_count, 1);

        // relations
        let node2 = make_node("n2", "c1", "sess-002");
        store.insert_nodes(&[node2]).await.unwrap();
        let rel = KnowledgeRelation {
            from_node_id: "n2".to_string(),
            to_node_id: "n1".to_string(),
            relation_type: "confirms".to_string(),
            detail: Some("validated".to_string()),
        };
        let count = store.upsert_relations(&[rel]).await.unwrap();
        assert_eq!(count, 1);

        // stats
        let (nodes, clusters) = store.stats(&[1]).await.unwrap();
        assert_eq!(nodes, 2);
        assert_eq!(clusters, 1);
    }

    #[tokio::test]
    async fn insert_duplicate_node_is_ignored() {
        let store = KnowledgeStore::new_in_memory();
        store.ensure_schema().await.unwrap();
        store.insert_cluster(&make_cluster("c1")).await.unwrap();

        let node = make_node("n1", "c1", "sess-001");
        store.insert_nodes(&[node.clone()]).await.unwrap();
        let second = store.insert_nodes(&[node]).await.unwrap();
        assert_eq!(second, 0);
    }

    #[tokio::test]
    async fn get_nodes_by_sessions_empty() {
        let store = KnowledgeStore::new_in_memory();
        store.ensure_schema().await.unwrap();
        let nodes = store.get_nodes_by_sessions(&[]).await.unwrap();
        assert!(nodes.is_empty());
    }
}

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_clusters (
    id              TEXT PRIMARY KEY,
    project_id      INTEGER NOT NULL,
    canonical_topic TEXT NOT NULL,
    l5_domain       TEXT,

    current_understanding TEXT,
    evolution_narrative   TEXT,
    final_confidence      REAL,

    node_count      INTEGER NOT NULL DEFAULT 0,
    day_count       INTEGER NOT NULL DEFAULT 0,
    first_day       TEXT,
    last_day        TEXT,

    pipeline_version TEXT,
    created_at      INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),
    updated_at      INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),

    FOREIGN KEY (project_id) REFERENCES projects(id)
);

CREATE INDEX IF NOT EXISTS idx_kc_project ON knowledge_clusters(project_id);
CREATE INDEX IF NOT EXISTS idx_kc_l5 ON knowledge_clusters(l5_domain) WHERE l5_domain IS NOT NULL;

CREATE TABLE IF NOT EXISTS knowledge_nodes (
    id              TEXT PRIMARY KEY,
    cluster_id      TEXT,
    project_id      INTEGER NOT NULL,
    session_id      TEXT NOT NULL,
    day             TEXT NOT NULL,

    topic           TEXT NOT NULL,
    conclusion      TEXT NOT NULL,
    node_type       TEXT NOT NULL,
    confidence      REAL NOT NULL,

    pipeline_version TEXT,
    created_at      INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000)
);

CREATE INDEX IF NOT EXISTS idx_kn_cluster ON knowledge_nodes(cluster_id);
CREATE INDEX IF NOT EXISTS idx_kn_session ON knowledge_nodes(session_id);
CREATE INDEX IF NOT EXISTS idx_kn_project ON knowledge_nodes(project_id);
CREATE INDEX IF NOT EXISTS idx_kn_day ON knowledge_nodes(day);
CREATE INDEX IF NOT EXISTS idx_kn_type ON knowledge_nodes(node_type);

CREATE TABLE IF NOT EXISTS knowledge_relations (
    from_node_id    TEXT NOT NULL,
    to_node_id      TEXT NOT NULL,
    relation_type   TEXT NOT NULL,
    detail          TEXT,

    detected_at     INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),

    PRIMARY KEY (from_node_id, to_node_id),
    FOREIGN KEY (from_node_id) REFERENCES knowledge_nodes(id),
    FOREIGN KEY (to_node_id) REFERENCES knowledge_nodes(id)
);

CREATE INDEX IF NOT EXISTS idx_kr_to ON knowledge_relations(to_node_id);
CREATE INDEX IF NOT EXISTS idx_kr_type ON knowledge_relations(relation_type);

CREATE TABLE IF NOT EXISTS knowledge_progress (
    session_id       TEXT PRIMARY KEY,
    status           TEXT NOT NULL DEFAULT 'pending',
    node_count       INTEGER DEFAULT 0,
    error            TEXT,
    processed_at     INTEGER,
    pipeline_version TEXT
);
"#;
