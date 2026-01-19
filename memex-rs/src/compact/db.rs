//! Compact 数据库层
//!
//! 管理 observations / talk_summaries / session_summaries 三张表

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Observation 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObservationType {
    /// Bug 修复
    Bugfix,
    /// 新功能
    Feature,
    /// 重构
    Refactor,
    /// 一般修改
    Change,
    /// 发现/理解
    Discovery,
    /// 决策
    Decision,
}

impl ObservationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bugfix => "bugfix",
            Self::Feature => "feature",
            Self::Refactor => "refactor",
            Self::Change => "change",
            Self::Discovery => "discovery",
            Self::Decision => "decision",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "bugfix" => Some(Self::Bugfix),
            "feature" => Some(Self::Feature),
            "refactor" => Some(Self::Refactor),
            "change" => Some(Self::Change),
            "discovery" => Some(Self::Discovery),
            "decision" => Some(Self::Decision),
            _ => None,
        }
    }
}

impl std::fmt::Display for ObservationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// L1: Observation（每个工具调用/操作一个）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub session_id: String,
    pub prompt_number: i32,
    pub source_offset: Option<i64>,

    pub observation_type: ObservationType,
    pub title: String,
    pub subtitle: Option<String>,
    pub facts: Option<Vec<String>>,
    pub narrative: Option<String>,
    pub files_read: Option<Vec<String>>,
    pub files_modified: Option<Vec<String>>,

    pub provider: Option<String>,
    pub model: Option<String>,
    pub created_at: String,
}

/// L2: Talk Summary（每轮对话一个）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TalkSummary {
    pub id: String,
    pub session_id: String,
    pub prompt_number: i32,

    pub user_request: Option<String>,
    pub summary: String,
    pub completed: Option<String>,
    pub files_involved: Option<Vec<String>>,

    pub provider: Option<String>,
    pub model: Option<String>,
    pub tokens_input: Option<i32>,
    pub tokens_output: Option<i32>,
    pub created_at: String,
}

/// L3: Session Summary（每个会话一个）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub session_id: String,

    pub summary: String,
    pub key_points: Option<Vec<String>>,
    pub files_involved: Option<Vec<String>>,
    pub technologies: Option<Vec<String>>,

    pub provider: Option<String>,
    pub model: Option<String>,
    pub tokens_input: Option<i32>,
    pub tokens_output: Option<i32>,
    pub created_at: String,
    pub updated_at: String,
}

/// 处理进度
#[derive(Debug, Clone)]
pub struct ProcessingProgress {
    pub last_processed_offset: i64,
    pub last_processed_prompt: i32,
}

impl Default for ProcessingProgress {
    fn default() -> Self {
        Self {
            last_processed_offset: -1, // -1 表示未处理，确保 sequence=0 的消息能被包含
            last_processed_prompt: 0,
        }
    }
}

/// Compact 数据库
///
/// 独立管理 compact 相关表，复用共享数据库文件
pub struct CompactDB {
    conn: Arc<Mutex<Connection>>,
}

impl CompactDB {
    /// 连接数据库并执行 migration
    ///
    /// # Arguments
    /// * `db_path` - 数据库文件路径
    /// * `fts_tokenizer` - FTS tokenizer 类型:
    ///   - "trigram": 支持中英文（子串匹配，默认推荐）
    ///   - "unicode61": 仅英文（精确词匹配，索引更小）
    pub fn connect<P: AsRef<Path>>(db_path: P, fts_tokenizer: Option<&str>) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let tokenizer = fts_tokenizer.unwrap_or("trigram");

        // 同步执行 migration
        Self::migrate_sync(&conn, tokenizer)?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        Ok(db)
    }

    /// 同步执行 migration（在创建连接时调用）
    fn migrate_sync(conn: &Connection, tokenizer: &str) -> Result<()> {

        // L1: Observations
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS observations (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                prompt_number INTEGER NOT NULL,
                source_offset INTEGER,

                type TEXT NOT NULL,
                title TEXT NOT NULL,
                subtitle TEXT,
                facts TEXT,
                narrative TEXT,
                files_read TEXT,
                files_modified TEXT,

                provider TEXT,
                model TEXT,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_obs_session ON observations(session_id);
            CREATE INDEX IF NOT EXISTS idx_obs_prompt ON observations(session_id, prompt_number);
            CREATE INDEX IF NOT EXISTS idx_obs_type ON observations(type);

            -- 唯一约束：防止异常重跑产生重复 observation
            CREATE UNIQUE INDEX IF NOT EXISTS idx_obs_unique
                ON observations(session_id, source_offset)
                WHERE source_offset IS NOT NULL;
            "#,
        )?;

        // L2: Talk Summaries
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS talk_summaries (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                prompt_number INTEGER NOT NULL,

                user_request TEXT,
                summary TEXT NOT NULL,
                completed TEXT,
                files_involved TEXT,

                provider TEXT,
                model TEXT,
                tokens_input INTEGER,
                tokens_output INTEGER,
                created_at TEXT NOT NULL,
                UNIQUE(session_id, prompt_number)
            );

            CREATE INDEX IF NOT EXISTS idx_talk_session ON talk_summaries(session_id);
            CREATE INDEX IF NOT EXISTS idx_talk_prompt ON talk_summaries(session_id, prompt_number);
            "#,
        )?;

        // L3: Session Summaries
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS session_summaries (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL UNIQUE,

                summary TEXT NOT NULL,
                key_points TEXT,
                files_involved TEXT,
                technologies TEXT,

                provider TEXT,
                model TEXT,
                tokens_input INTEGER,
                tokens_output INTEGER,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_session_summary ON session_summaries(session_id);
            "#,
        )?;

        // 处理进度表
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS compact_progress (
                session_id TEXT PRIMARY KEY,
                last_processed_offset INTEGER DEFAULT -1,
                last_processed_prompt INTEGER DEFAULT 0,
                updated_at TEXT NOT NULL,
                processing INTEGER DEFAULT 0,
                processing_started_at INTEGER
            );
            "#,
        )?;

        // 迁移：为旧表添加新字段（如果不存在）
        // SQLite 不支持 ADD COLUMN IF NOT EXISTS，用 PRAGMA 检查
        let has_processing: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('compact_progress') WHERE name = 'processing'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !has_processing {
            conn.execute_batch(
                r#"
                ALTER TABLE compact_progress ADD COLUMN processing INTEGER DEFAULT 0;
                ALTER TABLE compact_progress ADD COLUMN processing_started_at INTEGER;
                "#,
            )?;
            tracing::info!("compact_progress 表已添加 processing 字段");
        }

        // FTS5 全文搜索索引
        // 使用 external content 模式减少数据冗余
        // tokenizer 可配置：trigram（中英文）或 unicode61（纯英文）
        Self::create_fts_tables_if_needed(conn, tokenizer)?;

        tracing::debug!("Compact migration 完成 (tokenizer={})", tokenizer);
        Ok(())
    }

    /// 创建 FTS 表（如果不存在）
    ///
    /// 使用 external content 模式，通过 content= 和 content_rowid= 关联主表
    fn create_fts_tables_if_needed(conn: &Connection, tokenizer: &str) -> Result<()> {
        // 检查 FTS 表是否已存在
        let obs_fts_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='observations_fts'",
            [],
            |row| row.get(0),
        )?;

        if obs_fts_exists {
            // FTS 表已存在，跳过创建
            return Ok(());
        }

        tracing::info!("创建 FTS 索引表 (tokenizer={})", tokenizer);

        // L1 Observations FTS (external content)
        conn.execute(
            &format!(
                r#"
                CREATE VIRTUAL TABLE IF NOT EXISTS observations_fts USING fts5(
                    title,
                    subtitle,
                    narrative,
                    facts,
                    content='observations',
                    content_rowid='rowid',
                    tokenize='{}'
                )
                "#,
                tokenizer
            ),
            [],
        )?;

        // L2 Talk Summaries FTS (external content)
        conn.execute(
            &format!(
                r#"
                CREATE VIRTUAL TABLE IF NOT EXISTS talk_summaries_fts USING fts5(
                    user_request,
                    summary,
                    completed,
                    content='talk_summaries',
                    content_rowid='rowid',
                    tokenize='{}'
                )
                "#,
                tokenizer
            ),
            [],
        )?;

        // L3 Session Summaries FTS (external content)
        conn.execute(
            &format!(
                r#"
                CREATE VIRTUAL TABLE IF NOT EXISTS session_summaries_fts USING fts5(
                    summary,
                    key_points,
                    content='session_summaries',
                    content_rowid='rowid',
                    tokenize='{}'
                )
                "#,
                tokenizer
            ),
            [],
        )?;

        // 创建 triggers 自动同步 FTS 索引
        Self::create_fts_triggers(conn)?;

        Ok(())
    }

    /// 创建 FTS 同步 triggers
    ///
    /// 对于 external content FTS，需要手动维护索引同步
    fn create_fts_triggers(conn: &Connection) -> Result<()> {
        // observations triggers
        conn.execute_batch(
            r#"
            CREATE TRIGGER IF NOT EXISTS observations_ai AFTER INSERT ON observations BEGIN
                INSERT INTO observations_fts(rowid, title, subtitle, narrative, facts)
                VALUES (NEW.rowid, NEW.title, NEW.subtitle, NEW.narrative, NEW.facts);
            END;

            CREATE TRIGGER IF NOT EXISTS observations_ad AFTER DELETE ON observations BEGIN
                INSERT INTO observations_fts(observations_fts, rowid, title, subtitle, narrative, facts)
                VALUES ('delete', OLD.rowid, OLD.title, OLD.subtitle, OLD.narrative, OLD.facts);
            END;

            CREATE TRIGGER IF NOT EXISTS observations_au AFTER UPDATE ON observations BEGIN
                INSERT INTO observations_fts(observations_fts, rowid, title, subtitle, narrative, facts)
                VALUES ('delete', OLD.rowid, OLD.title, OLD.subtitle, OLD.narrative, OLD.facts);
                INSERT INTO observations_fts(rowid, title, subtitle, narrative, facts)
                VALUES (NEW.rowid, NEW.title, NEW.subtitle, NEW.narrative, NEW.facts);
            END;
            "#,
        )?;

        // talk_summaries triggers
        conn.execute_batch(
            r#"
            CREATE TRIGGER IF NOT EXISTS talk_summaries_ai AFTER INSERT ON talk_summaries BEGIN
                INSERT INTO talk_summaries_fts(rowid, user_request, summary, completed)
                VALUES (NEW.rowid, NEW.user_request, NEW.summary, NEW.completed);
            END;

            CREATE TRIGGER IF NOT EXISTS talk_summaries_ad AFTER DELETE ON talk_summaries BEGIN
                INSERT INTO talk_summaries_fts(talk_summaries_fts, rowid, user_request, summary, completed)
                VALUES ('delete', OLD.rowid, OLD.user_request, OLD.summary, OLD.completed);
            END;

            CREATE TRIGGER IF NOT EXISTS talk_summaries_au AFTER UPDATE ON talk_summaries BEGIN
                INSERT INTO talk_summaries_fts(talk_summaries_fts, rowid, user_request, summary, completed)
                VALUES ('delete', OLD.rowid, OLD.user_request, OLD.summary, OLD.completed);
                INSERT INTO talk_summaries_fts(rowid, user_request, summary, completed)
                VALUES (NEW.rowid, NEW.user_request, NEW.summary, NEW.completed);
            END;
            "#,
        )?;

        // session_summaries triggers
        conn.execute_batch(
            r#"
            CREATE TRIGGER IF NOT EXISTS session_summaries_ai AFTER INSERT ON session_summaries BEGIN
                INSERT INTO session_summaries_fts(rowid, summary, key_points)
                VALUES (NEW.rowid, NEW.summary, NEW.key_points);
            END;

            CREATE TRIGGER IF NOT EXISTS session_summaries_ad AFTER DELETE ON session_summaries BEGIN
                INSERT INTO session_summaries_fts(session_summaries_fts, rowid, summary, key_points)
                VALUES ('delete', OLD.rowid, OLD.summary, OLD.key_points);
            END;

            CREATE TRIGGER IF NOT EXISTS session_summaries_au AFTER UPDATE ON session_summaries BEGIN
                INSERT INTO session_summaries_fts(session_summaries_fts, rowid, summary, key_points)
                VALUES ('delete', OLD.rowid, OLD.summary, OLD.key_points);
                INSERT INTO session_summaries_fts(rowid, summary, key_points)
                VALUES (NEW.rowid, NEW.summary, NEW.key_points);
            END;
            "#,
        )?;

        tracing::debug!("FTS triggers 创建完成");
        Ok(())
    }

    /// 重建 FTS 索引
    ///
    /// 用于：
    /// 1. 切换 tokenizer 后重建索引
    /// 2. 修复损坏的索引
    pub async fn rebuild_fts(&self, tokenizer: &str) -> Result<()> {
        let conn = self.conn.lock().await;

        tracing::info!("重建 FTS 索引 (tokenizer={})", tokenizer);

        // 删除旧的 FTS 表
        conn.execute_batch(
            r#"
            DROP TABLE IF EXISTS observations_fts;
            DROP TABLE IF EXISTS talk_summaries_fts;
            DROP TABLE IF EXISTS session_summaries_fts;
            "#,
        )?;

        // 创建新的 FTS 表
        Self::create_fts_tables_if_needed(&conn, tokenizer)?;

        // 从主表重建索引数据
        // 对于 external content 模式，使用 INSERT INTO ... SELECT
        conn.execute_batch(
            r#"
            INSERT INTO observations_fts(observations_fts) VALUES('rebuild');
            INSERT INTO talk_summaries_fts(talk_summaries_fts) VALUES('rebuild');
            INSERT INTO session_summaries_fts(session_summaries_fts) VALUES('rebuild');
            "#,
        )?;

        tracing::info!("FTS 索引重建完成");
        Ok(())
    }

    /// 获取当前 FTS tokenizer 类型
    pub async fn get_fts_tokenizer(&self) -> Result<Option<String>> {
        let conn = self.conn.lock().await;
        let result: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='observations_fts'",
                [],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(sql) = result {
            // 从 CREATE 语句中提取 tokenize='xxx'
            if let Some(start) = sql.find("tokenize='") {
                let rest = &sql[start + 10..];
                if let Some(end) = rest.find('\'') {
                    return Ok(Some(rest[..end].to_string()));
                }
            }
            // 如果找不到 tokenize，说明使用默认的 unicode61
            return Ok(Some("unicode61".to_string()));
        }

        Ok(None)
    }

    // ==================== L1: Observations ====================

    /// 插入 Observation（重复时跳过）
    ///
    /// 返回 true 表示插入成功，false 表示已存在被跳过
    pub async fn insert_observation(&self, obs: &Observation) -> Result<bool> {
        let conn = self.conn.lock().await;
        conn.execute(
            r#"
            INSERT OR IGNORE INTO observations (
                id, session_id, prompt_number, source_offset,
                type, title, subtitle, facts, narrative,
                files_read, files_modified,
                provider, model, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            "#,
            params![
                obs.id,
                obs.session_id,
                obs.prompt_number,
                obs.source_offset,
                obs.observation_type.as_str(),
                obs.title,
                obs.subtitle,
                obs.facts.as_ref().and_then(|v| serde_json::to_string(v).ok()),
                obs.narrative,
                obs.files_read.as_ref().and_then(|v| serde_json::to_string(v).ok()),
                obs.files_modified.as_ref().and_then(|v| serde_json::to_string(v).ok()),
                obs.provider,
                obs.model,
                obs.created_at,
            ],
        )?;

        // 检查是否真正插入了（changes() 返回受影响的行数）
        // FTS 索引由 triggers 自动维护
        let inserted = conn.changes() > 0;
        Ok(inserted)
    }

    /// 获取会话的所有 Observations
    pub async fn get_observations(&self, session_id: &str) -> Result<Vec<Observation>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, session_id, prompt_number, source_offset,
                   type, title, subtitle, facts, narrative,
                   files_read, files_modified,
                   provider, model, created_at
            FROM observations
            WHERE session_id = ?1
            ORDER BY prompt_number, source_offset
            "#,
        )?;

        let rows = stmt.query_map(params![session_id], |row| {
            Ok(Observation {
                id: row.get(0)?,
                session_id: row.get(1)?,
                prompt_number: row.get(2)?,
                source_offset: row.get(3)?,
                observation_type: ObservationType::parse(&row.get::<_, String>(4)?)
                    .unwrap_or(ObservationType::Change),
                title: row.get(5)?,
                subtitle: row.get(6)?,
                facts: row
                    .get::<_, Option<String>>(7)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                narrative: row.get(8)?,
                files_read: row
                    .get::<_, Option<String>>(9)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                files_modified: row
                    .get::<_, Option<String>>(10)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                provider: row.get(11)?,
                model: row.get(12)?,
                created_at: row.get(13)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ==================== L2: Talk Summaries ====================

    /// 插入或更新 Talk Summary
    pub async fn upsert_talk_summary(&self, summary: &TalkSummary) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            r#"
            INSERT INTO talk_summaries (
                id, session_id, prompt_number,
                user_request, summary, completed, files_involved,
                provider, model, tokens_input, tokens_output, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(session_id, prompt_number) DO UPDATE SET
                user_request = excluded.user_request,
                summary = excluded.summary,
                completed = excluded.completed,
                files_involved = excluded.files_involved,
                provider = excluded.provider,
                model = excluded.model,
                tokens_input = excluded.tokens_input,
                tokens_output = excluded.tokens_output
            "#,
            params![
                summary.id,
                summary.session_id,
                summary.prompt_number,
                summary.user_request,
                summary.summary,
                summary.completed,
                summary.files_involved.as_ref().and_then(|v| serde_json::to_string(v).ok()),
                summary.provider,
                summary.model,
                summary.tokens_input,
                summary.tokens_output,
                summary.created_at,
            ],
        )?;

        // FTS 索引由 triggers 自动维护
        Ok(())
    }

    /// 获取会话的所有 Talk Summaries
    pub async fn get_talk_summaries(&self, session_id: &str) -> Result<Vec<TalkSummary>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, session_id, prompt_number,
                   user_request, summary, completed, files_involved,
                   provider, model, tokens_input, tokens_output, created_at
            FROM talk_summaries
            WHERE session_id = ?1
            ORDER BY prompt_number
            "#,
        )?;

        let rows = stmt.query_map(params![session_id], |row| {
            Ok(TalkSummary {
                id: row.get(0)?,
                session_id: row.get(1)?,
                prompt_number: row.get(2)?,
                user_request: row.get(3)?,
                summary: row.get(4)?,
                completed: row.get(5)?,
                files_involved: row
                    .get::<_, Option<String>>(6)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                provider: row.get(7)?,
                model: row.get(8)?,
                tokens_input: row.get(9)?,
                tokens_output: row.get(10)?,
                created_at: row.get(11)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// 获取会话的最大 prompt_number
    pub async fn get_max_prompt_number(&self, session_id: &str) -> Result<Option<i32>> {
        let conn = self.conn.lock().await;
        let result: Option<i32> = conn
            .query_row(
                "SELECT MAX(prompt_number) FROM talk_summaries WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        Ok(result)
    }

    // ==================== L3: Session Summaries ====================

    /// 插入或更新 Session Summary
    pub async fn upsert_session_summary(&self, summary: &SessionSummary) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            r#"
            INSERT INTO session_summaries (
                id, session_id,
                summary, key_points, files_involved, technologies,
                provider, model, tokens_input, tokens_output,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(session_id) DO UPDATE SET
                summary = excluded.summary,
                key_points = excluded.key_points,
                files_involved = excluded.files_involved,
                technologies = excluded.technologies,
                provider = excluded.provider,
                model = excluded.model,
                tokens_input = excluded.tokens_input,
                tokens_output = excluded.tokens_output,
                updated_at = excluded.updated_at
            "#,
            params![
                summary.id,
                summary.session_id,
                summary.summary,
                summary.key_points.as_ref().and_then(|v| serde_json::to_string(v).ok()),
                summary.files_involved.as_ref().and_then(|v| serde_json::to_string(v).ok()),
                summary.technologies.as_ref().and_then(|v| serde_json::to_string(v).ok()),
                summary.provider,
                summary.model,
                summary.tokens_input,
                summary.tokens_output,
                summary.created_at,
                summary.updated_at,
            ],
        )?;

        // FTS 索引由 triggers 自动维护
        Ok(())
    }

    /// 获取 Session Summary
    pub async fn get_session_summary(&self, session_id: &str) -> Result<Option<SessionSummary>> {
        let conn = self.conn.lock().await;
        let result = conn
            .query_row(
                r#"
                SELECT id, session_id,
                       summary, key_points, files_involved, technologies,
                       provider, model, tokens_input, tokens_output,
                       created_at, updated_at
                FROM session_summaries
                WHERE session_id = ?1
                "#,
                params![session_id],
                |row| {
                    Ok(SessionSummary {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        summary: row.get(2)?,
                        key_points: row
                            .get::<_, Option<String>>(3)?
                            .and_then(|s| serde_json::from_str(&s).ok()),
                        files_involved: row
                            .get::<_, Option<String>>(4)?
                            .and_then(|s| serde_json::from_str(&s).ok()),
                        technologies: row
                            .get::<_, Option<String>>(5)?
                            .and_then(|s| serde_json::from_str(&s).ok()),
                        provider: row.get(6)?,
                        model: row.get(7)?,
                        tokens_input: row.get(8)?,
                        tokens_output: row.get(9)?,
                        created_at: row.get(10)?,
                        updated_at: row.get(11)?,
                    })
                },
            )
            .optional()?;
        Ok(result)
    }

    // ==================== 处理进度 ====================

    /// 获取处理进度
    pub async fn get_progress(&self, session_id: &str) -> Result<ProcessingProgress> {
        let conn = self.conn.lock().await;
        let result = conn
            .query_row(
                r#"
                SELECT last_processed_offset, last_processed_prompt
                FROM compact_progress
                WHERE session_id = ?1
                "#,
                params![session_id],
                |row| {
                    Ok(ProcessingProgress {
                        last_processed_offset: row.get(0)?,
                        last_processed_prompt: row.get(1)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default();
        Ok(result)
    }

    /// 更新处理进度
    pub async fn update_progress(&self, session_id: &str, progress: &ProcessingProgress) -> Result<()> {
        let conn = self.conn.lock().await;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            r#"
            INSERT INTO compact_progress (session_id, last_processed_offset, last_processed_prompt, updated_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(session_id) DO UPDATE SET
                last_processed_offset = excluded.last_processed_offset,
                last_processed_prompt = excluded.last_processed_prompt,
                updated_at = excluded.updated_at
            "#,
            params![
                session_id,
                progress.last_processed_offset,
                progress.last_processed_prompt,
                now,
            ],
        )?;
        Ok(())
    }

    // ==================== 处理锁 ====================

    /// 锁超时时间（毫秒）- 5 分钟
    const LOCK_TIMEOUT_MS: i64 = 5 * 60 * 1000;

    /// 尝试获取处理锁（原子操作）
    ///
    /// 返回 true 表示获取成功，false 表示已有其他任务在处理
    /// 带超时保护：如果锁已超时，会自动抢占
    pub async fn try_lock(&self, session_id: &str) -> Result<bool> {
        let conn = self.conn.lock().await;
        let now_ms = current_time_ms();

        // 原子操作：仅当 processing=0 或锁超时时才能获取
        let rows = conn.execute(
            r#"
            INSERT INTO compact_progress (session_id, last_processed_offset, last_processed_prompt, updated_at, processing, processing_started_at)
            VALUES (?1, -1, 0, ?2, 1, ?3)
            ON CONFLICT(session_id) DO UPDATE SET
                processing = 1,
                processing_started_at = ?3
            WHERE processing = 0
               OR (?3 - processing_started_at) > ?4
            "#,
            params![
                session_id,
                chrono::Utc::now().to_rfc3339(),
                now_ms,
                Self::LOCK_TIMEOUT_MS,
            ],
        )?;

        Ok(rows > 0)
    }

    /// 释放处理锁
    pub async fn unlock(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE compact_progress SET processing = 0, processing_started_at = NULL WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    /// 更新进度并释放锁（原子操作）
    pub async fn update_progress_and_unlock(
        &self,
        session_id: &str,
        progress: &ProcessingProgress,
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            r#"
            UPDATE compact_progress SET
                last_processed_offset = ?2,
                last_processed_prompt = ?3,
                updated_at = ?4,
                processing = 0,
                processing_started_at = NULL
            WHERE session_id = ?1
            "#,
            params![
                session_id,
                progress.last_processed_offset,
                progress.last_processed_prompt,
                now,
            ],
        )?;
        Ok(())
    }

    /// 清理所有超时的锁（启动时调用）
    pub async fn unlock_stale_locks(&self) -> Result<usize> {
        let conn = self.conn.lock().await;
        let now_ms = current_time_ms();

        let rows = conn.execute(
            r#"
            UPDATE compact_progress SET
                processing = 0,
                processing_started_at = NULL
            WHERE processing = 1
              AND (?1 - processing_started_at) > ?2
            "#,
            params![now_ms, Self::LOCK_TIMEOUT_MS],
        )?;

        if rows > 0 {
            tracing::info!("清理了 {} 个超时的 compact 锁", rows);
        }

        Ok(rows)
    }

    // ==================== FTS 搜索 ====================

    /// 搜索 Observations
    pub async fn search_observations(&self, query: &str, limit: usize) -> Result<Vec<Observation>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            r#"
            SELECT o.id, o.session_id, o.prompt_number, o.source_offset,
                   o.type, o.title, o.subtitle, o.facts, o.narrative,
                   o.files_read, o.files_modified,
                   o.provider, o.model, o.created_at
            FROM observations o
            JOIN observations_fts fts ON o.rowid = fts.rowid
            WHERE observations_fts MATCH ?1
            ORDER BY fts.rank
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![query, limit as i64], |row| {
            Ok(Observation {
                id: row.get(0)?,
                session_id: row.get(1)?,
                prompt_number: row.get(2)?,
                source_offset: row.get(3)?,
                observation_type: ObservationType::parse(&row.get::<_, String>(4)?)
                    .unwrap_or(ObservationType::Change),
                title: row.get(5)?,
                subtitle: row.get(6)?,
                facts: row
                    .get::<_, Option<String>>(7)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                narrative: row.get(8)?,
                files_read: row
                    .get::<_, Option<String>>(9)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                files_modified: row
                    .get::<_, Option<String>>(10)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                provider: row.get(11)?,
                model: row.get(12)?,
                created_at: row.get(13)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// 搜索 Talk Summaries
    pub async fn search_talk_summaries(&self, query: &str, limit: usize) -> Result<Vec<TalkSummary>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            r#"
            SELECT t.id, t.session_id, t.prompt_number,
                   t.user_request, t.summary, t.completed, t.files_involved,
                   t.provider, t.model, t.tokens_input, t.tokens_output, t.created_at
            FROM talk_summaries t
            JOIN talk_summaries_fts fts ON t.rowid = fts.rowid
            WHERE talk_summaries_fts MATCH ?1
            ORDER BY fts.rank
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![query, limit as i64], |row| {
            Ok(TalkSummary {
                id: row.get(0)?,
                session_id: row.get(1)?,
                prompt_number: row.get(2)?,
                user_request: row.get(3)?,
                summary: row.get(4)?,
                completed: row.get(5)?,
                files_involved: row
                    .get::<_, Option<String>>(6)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                provider: row.get(7)?,
                model: row.get(8)?,
                tokens_input: row.get(9)?,
                tokens_output: row.get(10)?,
                created_at: row.get(11)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// 搜索 Session Summaries
    pub async fn search_session_summaries(&self, query: &str, limit: usize) -> Result<Vec<SessionSummary>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            r#"
            SELECT s.id, s.session_id,
                   s.summary, s.key_points, s.files_involved, s.technologies,
                   s.provider, s.model, s.tokens_input, s.tokens_output,
                   s.created_at, s.updated_at
            FROM session_summaries s
            JOIN session_summaries_fts fts ON s.rowid = fts.rowid
            WHERE session_summaries_fts MATCH ?1
            ORDER BY fts.rank
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![query, limit as i64], |row| {
            Ok(SessionSummary {
                id: row.get(0)?,
                session_id: row.get(1)?,
                summary: row.get(2)?,
                key_points: row
                    .get::<_, Option<String>>(3)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                files_involved: row
                    .get::<_, Option<String>>(4)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                technologies: row
                    .get::<_, Option<String>>(5)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                provider: row.get(6)?,
                model: row.get(7)?,
                tokens_input: row.get(8)?,
                tokens_output: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }
}

/// 获取当前时间戳（毫秒）
fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_migration() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let _db = CompactDB::connect(&db_path, None).unwrap();
        // Migration should succeed without error
    }

    #[tokio::test]
    async fn test_observation_crud() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = CompactDB::connect(&db_path, None).unwrap();

        let obs = Observation {
            id: "obs-1".to_string(),
            session_id: "session-1".to_string(),
            prompt_number: 1,
            source_offset: Some(100),
            observation_type: ObservationType::Bugfix,
            title: "Fix null pointer".to_string(),
            subtitle: Some("Fixed NPE in user service".to_string()),
            facts: Some(vec!["Added null check".to_string()]),
            narrative: Some("User reported crash...".to_string()),
            files_read: Some(vec!["src/user.rs".to_string()]),
            files_modified: Some(vec!["src/user.rs".to_string()]),
            provider: Some("ollama".to_string()),
            model: Some("qwen3:8b".to_string()),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };

        // 第一次插入应该成功
        let inserted = db.insert_observation(&obs).await.unwrap();
        assert!(inserted, "第一次插入应该成功");

        let observations = db.get_observations("session-1").await.unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].title, "Fix null pointer");

        // 重复插入应该被跳过（相同 session_id + source_offset）
        let mut dup_obs = obs.clone();
        dup_obs.id = "obs-2".to_string(); // 不同的 id
        dup_obs.title = "Different title".to_string();
        let inserted = db.insert_observation(&dup_obs).await.unwrap();
        assert!(!inserted, "重复插入应该被跳过");

        // 仍然只有 1 条记录
        let observations = db.get_observations("session-1").await.unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].title, "Fix null pointer"); // 保持原来的
    }

    #[tokio::test]
    async fn test_talk_summary_crud() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = CompactDB::connect(&db_path, None).unwrap();

        let summary = TalkSummary {
            id: "talk-1".to_string(),
            session_id: "session-1".to_string(),
            prompt_number: 1,
            user_request: Some("Fix the bug".to_string()),
            summary: "Fixed null pointer exception".to_string(),
            completed: Some("Bug fixed".to_string()),
            files_involved: Some(vec!["src/user.rs".to_string()]),
            provider: Some("ollama".to_string()),
            model: Some("qwen3:8b".to_string()),
            tokens_input: Some(100),
            tokens_output: Some(50),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };

        db.upsert_talk_summary(&summary).await.unwrap();

        let summaries = db.get_talk_summaries("session-1").await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].summary, "Fixed null pointer exception");
    }

    #[tokio::test]
    async fn test_session_summary_crud() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = CompactDB::connect(&db_path, None).unwrap();

        let summary = SessionSummary {
            id: "ss-1".to_string(),
            session_id: "session-1".to_string(),
            summary: "Implemented user authentication".to_string(),
            key_points: Some(vec!["Added JWT".to_string(), "Added login API".to_string()]),
            files_involved: Some(vec!["src/auth.rs".to_string()]),
            technologies: Some(vec!["JWT".to_string(), "bcrypt".to_string()]),
            provider: Some("ollama".to_string()),
            model: Some("qwen3:8b".to_string()),
            tokens_input: Some(500),
            tokens_output: Some(100),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        };

        db.upsert_session_summary(&summary).await.unwrap();

        let result = db.get_session_summary("session-1").await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().summary, "Implemented user authentication");
    }

    /// 验证 Codex CR 问题：FTS 索引与主表 id 不一致
    /// 场景：用不同的 id 调用 upsert，主表 ON CONFLICT 保留原 id，
    /// 但 FTS 删除用新 id，导致旧 FTS 条目残留
    #[tokio::test]
    async fn test_fts_consistency_bug_talk_summary() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = CompactDB::connect(&db_path, None).unwrap();

        // 第一次插入
        let summary1 = TalkSummary {
            id: "talk-original-id".to_string(),
            session_id: "session-1".to_string(),
            prompt_number: 1,
            user_request: Some("First request".to_string()),
            summary: "First summary".to_string(),
            completed: None,
            files_involved: None,
            provider: None,
            model: None,
            tokens_input: None,
            tokens_output: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        db.upsert_talk_summary(&summary1).await.unwrap();

        // 用不同的 id 但相同的 (session_id, prompt_number) 调用 upsert
        // 这会触发 ON CONFLICT，主表保留原 id，但 FTS 用新 id 操作
        let summary2 = TalkSummary {
            id: "talk-new-id".to_string(), // 不同的 id
            session_id: "session-1".to_string(),
            prompt_number: 1, // 相同的 unique key
            user_request: Some("Updated request".to_string()),
            summary: "Updated summary".to_string(),
            completed: None,
            files_involved: None,
            provider: None,
            model: None,
            tokens_input: None,
            tokens_output: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        db.upsert_talk_summary(&summary2).await.unwrap();

        // 搜索应该只返回一条结果
        let results = db.search_talk_summaries("summary", 10).await.unwrap();
        println!("FTS search results count: {}", results.len());
        assert_eq!(results.len(), 1, "FTS should return exactly 1 result, but got {}", results.len());
    }

    /// 更精确地验证 FTS 数据膨胀问题
    /// 检查 FTS 表实际记录数量
    #[tokio::test]
    async fn test_fts_data_bloat_talk_summary() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = CompactDB::connect(&db_path, None).unwrap();

        // 第一次插入
        let summary1 = TalkSummary {
            id: "talk-original-id".to_string(),
            session_id: "session-1".to_string(),
            prompt_number: 1,
            user_request: Some("First request".to_string()),
            summary: "First summary unique".to_string(),
            completed: None,
            files_involved: None,
            provider: None,
            model: None,
            tokens_input: None,
            tokens_output: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        db.upsert_talk_summary(&summary1).await.unwrap();

        // 用不同的 id 调用 upsert
        let summary2 = TalkSummary {
            id: "talk-new-id".to_string(),
            session_id: "session-1".to_string(),
            prompt_number: 1,
            user_request: Some("Updated request".to_string()),
            summary: "Updated summary unique".to_string(),
            completed: None,
            files_involved: None,
            provider: None,
            model: None,
            tokens_input: None,
            tokens_output: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        db.upsert_talk_summary(&summary2).await.unwrap();

        // 直接查询 FTS 表记录数量
        let conn = db.conn.lock().await;
        let fts_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM talk_summaries_fts", [], |row| row.get(0))
            .unwrap();

        let main_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM talk_summaries", [], |row| row.get(0))
            .unwrap();

        println!("Main table count: {}, FTS table count: {}", main_count, fts_count);

        // BUG: 如果 FTS 有数据膨胀，fts_count 会大于 main_count
        // 主表应该只有 1 条（ON CONFLICT 更新）
        // FTS 表也应该只有 1 条
        assert_eq!(main_count, 1, "Main table should have exactly 1 record");
        assert_eq!(
            fts_count, main_count,
            "FTS table count ({}) should equal main table count ({})",
            fts_count, main_count
        );
    }

    /// 验证 Codex CR 问题：FTS 索引与主表 id 不一致 (session_summaries)
    #[tokio::test]
    async fn test_fts_consistency_bug_session_summary() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = CompactDB::connect(&db_path, None).unwrap();

        // 第一次插入
        let summary1 = SessionSummary {
            id: "ss-original-id".to_string(),
            session_id: "session-1".to_string(),
            summary: "Original session summary".to_string(),
            key_points: None,
            files_involved: None,
            technologies: None,
            provider: None,
            model: None,
            tokens_input: None,
            tokens_output: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        };
        db.upsert_session_summary(&summary1).await.unwrap();

        // 用不同的 id 但相同的 session_id 调用 upsert
        let summary2 = SessionSummary {
            id: "ss-new-id".to_string(), // 不同的 id
            session_id: "session-1".to_string(), // 相同的 unique key
            summary: "Updated session summary".to_string(),
            key_points: None,
            files_involved: None,
            technologies: None,
            provider: None,
            model: None,
            tokens_input: None,
            tokens_output: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-02T00:00:00Z".to_string(),
        };
        db.upsert_session_summary(&summary2).await.unwrap();

        // 搜索应该只返回一条结果
        let results = db.search_session_summaries("summary", 10).await.unwrap();

        println!("FTS search results count: {}", results.len());
        assert_eq!(results.len(), 1, "FTS should return exactly 1 result, but got {}", results.len());
    }
}
