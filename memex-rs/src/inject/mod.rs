//! 注入模块 - Claude Code Hook 上下文注入
//!
//! 支持两种 Hook:
//! - SessionStart: 会话开始时注入最近历史，按 sources 优先级 fallback
//! - UserPromptSubmit: 根据用户 prompt 进行向量搜索
//!
//! 数据源 (sources):
//! - messages: 原始消息（L0）
//! - observations: 工具调用观察（L1）
//! - talks: 对话摘要（L2）
//! - sessions: 会话摘要（L3）
//! - summaries: L3+L2+L1 的快捷方式（自动展开）

use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::compact::{
    CompactDB, CompactLevel, CompactVectorStore, InjectConfig, InjectSource, Observation,
    SessionStartConfig, SessionSummary, TalkSummary, UserPromptConfig, UserPromptSearchMode,
    VectorDistanceType,
};
use crate::db_reader::DbReader;
use crate::llm::EmbeddingProvider;
use crate::vector::VectorStore;
use ai_cli_session_db::Message;

/// Hook 输出格式（Claude Code 期望的 JSON 结构）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookOutput {
    /// Hook 事件名称
    pub hook_event_name: String,
    /// 注入的上下文内容
    pub additional_context: String,
}

/// 注入结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InjectResult {
    /// 注入的上下文内容（Markdown 格式）
    pub context: String,
    /// 注入的条目数量
    pub count: usize,
    /// 注入模式
    pub mode: String,
    /// 估算的 token 数（粗略估计：字符数 / 4）
    pub estimated_tokens: usize,
}

impl InjectResult {
    /// 转换为 Hook 输出格式
    pub fn to_hook_output(&self, event_name: &str) -> HookOutput {
        HookOutput {
            hook_event_name: event_name.to_string(),
            additional_context: self.context.clone(),
        }
    }

    /// 输出为 JSON（供 CLI 使用）
    pub fn to_json(&self, event_name: &str) -> String {
        let output = self.to_hook_output(event_name);
        serde_json::json!({
            "hookSpecificOutput": output
        })
        .to_string()
    }
}

/// 注入服务
pub struct InjectService {
    db: Arc<DbReader>,
    compact_db: Arc<CompactDB>,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
    /// L0 原文向量存储
    l0_vector: Option<Arc<RwLock<VectorStore>>>,
    /// Compact 向量存储（L1/L2/L3）
    compact_vector: Option<Arc<RwLock<CompactVectorStore>>>,
    config: InjectConfig,
}

impl InjectService {
    /// 创建注入服务
    pub fn new(db: Arc<DbReader>, compact_db: Arc<CompactDB>, config: InjectConfig) -> Self {
        Self {
            db,
            compact_db,
            embedding: None,
            l0_vector: None,
            compact_vector: None,
            config,
        }
    }

    /// 设置 Embedding Provider（向量模式需要）
    pub fn with_embedding(mut self, embedding: Arc<dyn EmbeddingProvider>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// 设置 L0 原文向量存储
    pub fn with_l0_vector(mut self, vector: Arc<RwLock<VectorStore>>) -> Self {
        self.l0_vector = Some(vector);
        self
    }

    /// 设置 Compact Vector Store（L1/L2/L3）
    pub fn with_compact_vector(mut self, compact_vector: Arc<RwLock<CompactVectorStore>>) -> Self {
        self.compact_vector = Some(compact_vector);
        self
    }

    /// 执行 SessionStart 注入
    ///
    /// - `config_override`: 可选的配置覆盖
    /// - `project_path`: 项目路径（可选，用于项目过滤）
    pub async fn inject_session_start(
        &self,
        config_override: Option<SessionStartConfig>,
        project_path: Option<&str>,
    ) -> Result<InjectResult> {
        // 获取有效配置（覆盖 > 详细配置 > 顶层 mode > 默认）
        let config = config_override.unwrap_or_else(|| self.config.effective_session_start());

        if !config.enabled {
            return Ok(InjectResult {
                context: String::new(),
                count: 0,
                mode: "none".to_string(),
                estimated_tokens: 0,
            });
        }

        self.inject_full(&config, project_path).await
    }

    /// 执行 UserPromptSubmit 注入
    ///
    /// - `query`: 用户查询（必须）
    /// - `config_override`: 可选的配置覆盖
    /// - `project_path`: 项目路径（可选，用于项目过滤）
    pub async fn inject_user_prompt(
        &self,
        query: &str,
        config_override: Option<UserPromptConfig>,
        project_path: Option<&str>,
    ) -> Result<InjectResult> {
        // 获取有效配置（覆盖 > 详细配置 > 顶层 mode > 默认）
        let config = config_override.unwrap_or_else(|| self.config.effective_user_prompt());

        if !config.enabled {
            return Ok(InjectResult {
                context: String::new(),
                count: 0,
                mode: "none".to_string(),
                estimated_tokens: 0,
            });
        }

        match config.mode() {
            UserPromptSearchMode::Combine => {
                self.inject_combine(query, &config, project_path).await
            }
            UserPromptSearchMode::Fallback => {
                self.inject_fallback(query, &config, project_path).await
            }
        }
    }

    /// 通过路径查找项目 ID
    async fn find_project_id_by_path(&self, path: &str) -> Result<Option<i64>> {
        let projects = self.db.list_projects().await?;
        for project in projects {
            if project.path == path || project.path.ends_with(path) || path.ends_with(&project.path)
            {
                return Ok(Some(project.id));
            }
        }
        Ok(None)
    }

    /// SessionStart 注入
    ///
    /// 按 sources 优先级 fallback：有高层数据就用高层，没有就降级
    /// 默认顺序: L3 (sessions) → L2 (talks) → L0 (messages)
    async fn inject_full(
        &self,
        config: &SessionStartConfig,
        project_path: Option<&str>,
    ) -> Result<InjectResult> {
        let sources = config.sources();
        let max_items = config.max_items();
        let max_tokens = config.max_tokens();

        // 获取项目 ID（如果指定了项目路径）
        let project_id = if let Some(path) = project_path {
            self.find_project_id_by_path(path).await?
        } else {
            None
        };

        // 按 sources 优先级 fallback
        for source in &sources {
            let (items, source_name) = match source {
                InjectSource::Sessions => {
                    let summaries = self
                        .compact_db
                        .get_recent_session_summaries(project_id, max_items)
                        .await?;
                    if summaries.is_empty() {
                        continue; // fallback 到下一个 source
                    }
                    let items: Vec<ContextItem> = summaries
                        .into_iter()
                        .map(ContextItem::SessionSummary)
                        .collect();
                    (items, "sessions")
                }
                InjectSource::Talks => {
                    let talks = self
                        .compact_db
                        .get_recent_talk_summaries(project_id, max_items)
                        .await?;
                    if talks.is_empty() {
                        continue;
                    }
                    let items: Vec<ContextItem> =
                        talks.into_iter().map(ContextItem::TalkSummary).collect();
                    (items, "talks")
                }
                InjectSource::Observations => {
                    let obs = self
                        .compact_db
                        .get_recent_observations(project_id, max_items)
                        .await?;
                    if obs.is_empty() {
                        continue;
                    }
                    let items: Vec<ContextItem> =
                        obs.into_iter().map(ContextItem::Observation).collect();
                    (items, "observations")
                }
                InjectSource::Messages => {
                    let messages = self.db.get_recent_messages(project_id, max_items).await?;
                    if messages.is_empty() {
                        continue;
                    }
                    let items: Vec<ContextItem> =
                        messages.into_iter().map(ContextItem::Message).collect();
                    (items, "messages")
                }
                InjectSource::Summaries => {
                    // Summaries 已在 config.sources() 中展开为具体类型，不会到达这里
                    unreachable!("Summaries should be expanded by config.sources()")
                }
            };

            // 找到数据了，格式化输出
            return self
                .format_context_items(items, source_name, max_tokens)
                .await;
        }

        // 所有 sources 都没有数据
        Ok(InjectResult {
            context: String::new(),
            count: 0,
            mode: "fallback".to_string(),
            estimated_tokens: 0,
        })
    }

    /// 格式化上下文条目
    async fn format_context_items(
        &self,
        items: Vec<ContextItem>,
        source_name: &str,
        max_tokens: usize,
    ) -> Result<InjectResult> {
        let title = match source_name {
            "sessions" => "Memory Context (Recent Sessions)",
            "talks" => "Memory Context (Recent Conversations)",
            "observations" => "Memory Context (Recent Operations)",
            "messages" => "Memory Context (Recent Messages)",
            _ => "Memory Context",
        };

        let mut context = format!("# {}\n\n", title);
        let mut total_chars = context.len();
        let mut count = 0;

        for item in items {
            let entry = match &item {
                ContextItem::SessionSummary(s) => self.format_session_summary(s).await,
                ContextItem::TalkSummary(t) => self.format_talk_summary(t).await,
                ContextItem::Observation(o) => self.format_observation(o),
                ContextItem::Message(m) => self.format_message(m).await,
            };
            let entry_chars = entry.len();

            // 粗略估计 token（字符数 / 4）
            if total_chars + entry_chars > max_tokens * 4 {
                break;
            }

            context.push_str(&entry);
            context.push_str("\n---\n\n");
            total_chars += entry_chars + 6;
            count += 1;
        }

        Ok(InjectResult {
            context,
            count,
            mode: source_name.to_string(),
            estimated_tokens: total_chars / 4,
        })
    }

    /// Combine 模式注入
    ///
    /// 向量匹配，合并所有 sources 结果
    async fn inject_combine(
        &self,
        query: &str,
        config: &UserPromptConfig,
        project_path: Option<&str>,
    ) -> Result<InjectResult> {
        let sources = config.expanded_sources();
        if sources.is_empty() {
            return Err(anyhow::anyhow!("Combine 模式需要配置 sources"));
        }

        // 生成查询向量
        let embedding = self
            .embedding
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("向量模式需要 EmbeddingProvider"))?;
        let query_embedding = embedding.embed(query).await?;

        // 获取项目 ID
        let project_id = if config.project_scope() {
            if let Some(path) = project_path {
                self.find_project_id_by_path(path).await?
            } else {
                None
            }
        } else {
            None
        };

        // 从所有 sources 收集结果
        let mut all_results: Vec<ScoredResult> = Vec::new();

        for source in &sources {
            let results = self
                .search_source(*source, &query_embedding, project_id, config)
                .await?;
            all_results.extend(results);
        }

        // 应用过滤和排序
        self.filter_and_rank(&mut all_results, config)?;

        // 格式化输出
        self.format_vector_results(all_results, config, "combine")
            .await
    }

    /// Fallback 模式注入
    ///
    /// 按 sources 顺序尝试，有足够结果即停
    async fn inject_fallback(
        &self,
        query: &str,
        config: &UserPromptConfig,
        project_path: Option<&str>,
    ) -> Result<InjectResult> {
        let sources = config.expanded_sources();
        if sources.is_empty() {
            return Err(anyhow::anyhow!("Fallback 模式需要配置 sources"));
        }

        // 生成查询向量
        let embedding = self
            .embedding
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("向量模式需要 EmbeddingProvider"))?;
        let query_embedding = embedding.embed(query).await?;

        // 获取项目 ID
        let project_id = if config.project_scope() {
            if let Some(path) = project_path {
                self.find_project_id_by_path(path).await?
            } else {
                None
            }
        } else {
            None
        };

        // 按顺序尝试每个 source
        for source in &sources {
            let mut results = self
                .search_source(*source, &query_embedding, project_id, config)
                .await?;

            // 应用过滤
            self.filter_and_rank(&mut results, config)?;

            // 如果有结果，返回
            if !results.is_empty() {
                return self
                    .format_vector_results(results, config, "fallback")
                    .await;
            }
        }

        // 所有 sources 都没有结果
        Ok(InjectResult {
            context: String::new(),
            count: 0,
            mode: "fallback".to_string(),
            estimated_tokens: 0,
        })
    }

    /// 搜索单个数据源
    async fn search_source(
        &self,
        source: InjectSource,
        query_embedding: &[f32],
        _project_id: Option<i64>,
        config: &UserPromptConfig,
    ) -> Result<Vec<ScoredResult>> {
        let limit = config.limit_per_source();
        let distance_type = config.distance_type();

        match source {
            InjectSource::Messages => {
                // L0 原文向量搜索
                let store = self
                    .l0_vector
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Messages 源需要 L0 向量存储"))?;
                let store = store.read().await;
                let results = store
                    .search_with_distance_type(query_embedding, limit, distance_type)
                    .await?;

                // 收集 message_ids 并批量查询
                let message_ids: Vec<i64> = results.iter().map(|r| r.message_id).collect();
                let messages = self.db.get_messages_by_ids(&message_ids).await?;

                // 构建 message_id -> message 的映射
                let msg_map: std::collections::HashMap<i64, _> =
                    messages.into_iter().map(|m| (m.id, m)).collect();

                // 组装结果
                let scored_results: Vec<ScoredResult> = results
                    .into_iter()
                    .filter_map(|r| {
                        msg_map.get(&r.message_id).map(|msg| {
                            // timestamp 是 Unix 毫秒，转换为 RFC3339
                            let created_at = DateTime::from_timestamp_millis(msg.timestamp)
                                .map(|dt| dt.to_rfc3339());
                            ScoredResult {
                                source: "messages".to_string(),
                                session_id: msg.session_id.clone(),
                                text: r.content,
                                distance: r.distance,
                                score: 1.0,
                                created_at,
                            }
                        })
                    })
                    .collect();

                Ok(scored_results)
            }
            InjectSource::Observations => {
                self.search_compact_level(CompactLevel::L1, query_embedding, limit, config)
                    .await
            }
            InjectSource::Talks => {
                self.search_compact_level(CompactLevel::L2, query_embedding, limit, config)
                    .await
            }
            InjectSource::Sessions => {
                self.search_compact_level(CompactLevel::L3, query_embedding, limit, config)
                    .await
            }
            InjectSource::Summaries => {
                // 不应该到这里，expand() 已经展开了
                Ok(vec![])
            }
        }
    }

    /// 搜索 Compact 层级
    async fn search_compact_level(
        &self,
        level: CompactLevel,
        query_embedding: &[f32],
        limit: usize,
        config: &UserPromptConfig,
    ) -> Result<Vec<ScoredResult>> {
        let distance_type = config.distance_type();
        let store = self
            .compact_vector
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("{:?} 源需要 Compact 向量存储", level))?;
        let store = store.read().await;
        let results = store
            .search_with_distance_type(query_embedding, Some(level), limit, distance_type)
            .await?;

        let source_name = match level {
            CompactLevel::L1 => "observations",
            CompactLevel::L2 => "talks",
            CompactLevel::L3 => "sessions",
        };

        Ok(results
            .into_iter()
            .map(|r| ScoredResult {
                source: source_name.to_string(),
                session_id: r.session_id,
                text: r.text,
                distance: r.distance,
                score: 1.0,
                created_at: r.created_at,
            })
            .collect())
    }

    /// 过滤和排序结果
    fn filter_and_rank(
        &self,
        results: &mut Vec<ScoredResult>,
        config: &UserPromptConfig,
    ) -> Result<()> {
        let threshold = config.similarity_threshold();
        let distance_type = config.distance_type();
        let time_window = config.time_window_days();
        let time_decay = config.time_decay();
        let halflife = config.time_decay_halflife() as f64;

        // 过滤：相似度阈值
        // Cosine distance: range [0, 2], similarity = 1 - distance/2
        // Euclidean distance: range [0, +∞)，不使用 similarity 过滤
        // Dot distance: 同 cosine（要求向量已归一化）
        match distance_type {
            VectorDistanceType::Cosine | VectorDistanceType::Dot => {
                results.retain(|r| {
                    let similarity = 1.0 - (r.distance / 2.0);
                    similarity >= threshold
                });
            }
            VectorDistanceType::Euclidean => {
                // 欧氏距离不使用 similarity 过滤，因为范围不固定
                // 如果需要，可以用 distance_threshold 配置项
            }
        }

        // 过滤：时间窗口
        if time_window > 0 {
            let cutoff = Local::now() - chrono::Duration::days(time_window as i64);
            results.retain(|r| {
                r.created_at
                    .as_ref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Local) > cutoff)
                    .unwrap_or(true)
            });
        }

        // 应用时间衰减
        if time_decay {
            let now = Utc::now();
            for result in results.iter_mut() {
                if let Some(created_at) = &result.created_at {
                    if let Ok(dt) = DateTime::parse_from_rfc3339(created_at) {
                        let days_ago = (now - dt.with_timezone(&Utc)).num_days() as f64;
                        let decay = 0.5_f64.powf(days_ago / halflife);
                        result.score *= decay as f32;
                    }
                }
            }
        }

        // 按距离排序（越小越好）
        results.sort_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(())
    }

    /// 格式化向量搜索结果
    async fn format_vector_results(
        &self,
        results: Vec<ScoredResult>,
        config: &UserPromptConfig,
        mode: &str,
    ) -> Result<InjectResult> {
        if results.is_empty() {
            return Ok(InjectResult {
                context: String::new(),
                count: 0,
                mode: mode.to_string(),
                estimated_tokens: 0,
            });
        }

        let max_tokens = config.max_tokens();

        let mut context = String::from("# Relevant Memory Context\n\n");
        context.push_str("> *Matched based on your current query*\n\n");
        let mut total_chars = context.len();
        let mut count = 0;

        for result in results {
            let entry = self.format_scored_result(&result).await;
            let entry_chars = entry.len();

            if total_chars + entry_chars > max_tokens * 4 {
                break;
            }

            context.push_str(&entry);
            context.push_str("\n---\n\n");
            total_chars += entry_chars + 6;
            count += 1;
        }

        if count > 0 {
            context.push_str("\n> 💡 *Use `get_session(session_id)` to get full context*\n");
        }

        Ok(InjectResult {
            context,
            count,
            mode: mode.to_string(),
            estimated_tokens: total_chars / 4,
        })
    }

    /// 格式化 Session Summary 为 Markdown
    async fn format_session_summary(&self, summary: &SessionSummary) -> String {
        let project_name = if let Ok(Some(session)) = self.db.get_session(&summary.session_id).await
        {
            if let Ok(Some(project)) = self.db.get_project(session.project_id).await {
                project.name
            } else {
                "Unknown".to_string()
            }
        } else {
            "Unknown".to_string()
        };

        let time_ago = format_time_ago(&summary.created_at);

        let mut output = format!(
            "## {} ({})\n**Session**: `{}`\n\n{}\n",
            project_name,
            time_ago,
            &summary.session_id[..8.min(summary.session_id.len())],
            summary.summary
        );

        if let Some(key_points) = &summary.key_points {
            if !key_points.is_empty() {
                output.push_str("\n**Key Points:**\n");
                for point in key_points {
                    output.push_str(&format!("- {}\n", point));
                }
            }
        }

        if let Some(files) = &summary.files_involved {
            if !files.is_empty() && files.len() <= 5 {
                output.push_str(&format!("\n**Files:** {}\n", files.join(", ")));
            }
        }

        output
    }

    /// 格式化 Talk Summary 为 Markdown
    async fn format_talk_summary(&self, summary: &TalkSummary) -> String {
        let project_name = if let Ok(Some(session)) = self.db.get_session(&summary.session_id).await
        {
            if let Ok(Some(project)) = self.db.get_project(session.project_id).await {
                project.name
            } else {
                "Unknown".to_string()
            }
        } else {
            "Unknown".to_string()
        };

        let time_ago = format_time_ago(&summary.created_at);

        let mut output = format!(
            "## {} ({})\n**Session**: `{}` | **Turn**: #{}\n\n",
            project_name,
            time_ago,
            &summary.session_id[..8.min(summary.session_id.len())],
            summary.prompt_number
        );

        if let Some(user_request) = &summary.user_request {
            output.push_str(&format!("**User**: {}\n\n", user_request));
        }

        output.push_str(&format!("{}\n", summary.summary));

        if let Some(completed) = &summary.completed {
            output.push_str(&format!("\n**Completed**: {}\n", completed));
        }

        if let Some(files) = &summary.files_involved {
            if !files.is_empty() && files.len() <= 5 {
                output.push_str(&format!("\n**Files:** {}\n", files.join(", ")));
            }
        }

        output
    }

    /// 格式化 Observation 为 Markdown
    fn format_observation(&self, obs: &Observation) -> String {
        let time_ago = format_time_ago(&obs.created_at);

        let mut output = format!(
            "## [{}] {} ({})\n**Session**: `{}` | **Turn**: #{}\n\n",
            obs.observation_type.as_str().to_uppercase(),
            obs.title,
            time_ago,
            &obs.session_id[..8.min(obs.session_id.len())],
            obs.prompt_number
        );

        if let Some(subtitle) = &obs.subtitle {
            output.push_str(&format!("{}\n\n", subtitle));
        }

        if let Some(narrative) = &obs.narrative {
            output.push_str(&format!("{}\n", narrative));
        }

        if let Some(facts) = &obs.facts {
            if !facts.is_empty() {
                output.push_str("\n**Facts:**\n");
                for fact in facts {
                    output.push_str(&format!("- {}\n", fact));
                }
            }
        }

        if let Some(files) = &obs.files_modified {
            if !files.is_empty() && files.len() <= 5 {
                output.push_str(&format!("\n**Files modified:** {}\n", files.join(", ")));
            }
        }

        output
    }

    /// 格式化 Message 为 Markdown
    async fn format_message(&self, msg: &Message) -> String {
        let project_name = if let Ok(Some(session)) = self.db.get_session(&msg.session_id).await {
            if let Ok(Some(project)) = self.db.get_project(session.project_id).await {
                project.name
            } else {
                "Unknown".to_string()
            }
        } else {
            "Unknown".to_string()
        };

        let time_ago = DateTime::from_timestamp_millis(msg.timestamp)
            .map(|dt| format_time_ago(&dt.to_rfc3339()))
            .unwrap_or_else(|| "Unknown".to_string());

        // 使用 type 字段的字符串表示
        let type_str = format!("{:?}", msg.r#type);
        let role_label = if type_str.contains("User") {
            "User"
        } else {
            "Assistant"
        };

        // 截断过长的内容
        let content = if msg.content_text.len() > 500 {
            format!("{}...", &msg.content_text[..500])
        } else {
            msg.content_text.clone()
        };

        format!(
            "## {} ({})\n**[{}]** Session: `{}`\n\n{}\n",
            project_name,
            time_ago,
            role_label,
            &msg.session_id[..8.min(msg.session_id.len())],
            content
        )
    }

    /// 格式化搜索结果为 Markdown
    async fn format_scored_result(&self, result: &ScoredResult) -> String {
        let project_name = if let Ok(Some(session)) = self.db.get_session(&result.session_id).await
        {
            if let Ok(Some(project)) = self.db.get_project(session.project_id).await {
                project.name
            } else {
                "Unknown".to_string()
            }
        } else {
            "Unknown".to_string()
        };

        let time_ago = result
            .created_at
            .as_ref()
            .map(|s| format_time_ago(s))
            .unwrap_or_else(|| "Unknown".to_string());

        let source_label = match result.source.as_str() {
            "messages" => "Message",
            "observations" => "Observation",
            "talks" => "Talk",
            "sessions" => "Session",
            _ => "Unknown",
        };

        format!(
            "## {} ({})\n**[{}]** from `{}`\n\n{}\n",
            project_name,
            time_ago,
            source_label,
            &result.session_id[..8.min(result.session_id.len())],
            result.text
        )
    }
}

/// 带分数的搜索结果
struct ScoredResult {
    source: String,
    session_id: String,
    text: String,
    distance: f32,
    score: f32,
    created_at: Option<String>,
}

/// 上下文条目（用于 SessionStart fallback）
enum ContextItem {
    SessionSummary(SessionSummary),
    TalkSummary(TalkSummary),
    Observation(Observation),
    Message(Message),
}

/// 格式化时间为 "X ago" 格式
fn format_time_ago(iso_time: &str) -> String {
    if let Ok(dt) = DateTime::parse_from_rfc3339(iso_time) {
        let now = Utc::now();
        let diff = now - dt.with_timezone(&Utc);

        if diff.num_days() > 365 {
            format!("{} years ago", diff.num_days() / 365)
        } else if diff.num_days() > 30 {
            format!("{} months ago", diff.num_days() / 30)
        } else if diff.num_days() > 0 {
            format!("{} days ago", diff.num_days())
        } else if diff.num_hours() > 0 {
            format!("{} hours ago", diff.num_hours())
        } else {
            "just now".to_string()
        }
    } else {
        "Unknown".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_time_ago_just_now() {
        let now = Utc::now();
        let iso = now.to_rfc3339();
        assert_eq!(format_time_ago(&iso), "just now");
    }

    #[test]
    fn test_format_time_ago_hours() {
        let now = Utc::now();
        let hours_ago = now - chrono::Duration::hours(5);
        let iso = hours_ago.to_rfc3339();
        assert_eq!(format_time_ago(&iso), "5 hours ago");
    }

    #[test]
    fn test_format_time_ago_days() {
        let now = Utc::now();
        let days_ago = now - chrono::Duration::days(3);
        let iso = days_ago.to_rfc3339();
        assert_eq!(format_time_ago(&iso), "3 days ago");
    }

    #[test]
    fn test_inject_result_to_json() {
        let result = InjectResult {
            context: "# Test Context".to_string(),
            count: 3,
            mode: "combine".to_string(),
            estimated_tokens: 100,
        };

        let json = result.to_json("UserPromptSubmit");
        assert!(json.contains("hookSpecificOutput"));
        assert!(json.contains("UserPromptSubmit"));
        assert!(json.contains("Test Context"));
    }

    #[test]
    fn test_inject_result_to_hook_output() {
        let result = InjectResult {
            context: "Test content".to_string(),
            count: 1,
            mode: "full".to_string(),
            estimated_tokens: 50,
        };

        let output = result.to_hook_output("SessionStart");
        assert_eq!(output.hook_event_name, "SessionStart");
        assert_eq!(output.additional_context, "Test content");
    }

    #[test]
    fn test_time_decay_calculation() {
        let halflife = 30.0_f64;

        let decay_0 = 0.5_f64.powf(0.0 / halflife);
        assert!((decay_0 - 1.0).abs() < 0.001);

        let decay_30 = 0.5_f64.powf(30.0 / halflife);
        assert!((decay_30 - 0.5).abs() < 0.001);

        let decay_60 = 0.5_f64.powf(60.0 / halflife);
        assert!((decay_60 - 0.25).abs() < 0.001);
    }

    #[test]
    fn test_similarity_threshold_filtering() {
        let threshold = 0.65_f32;

        let distance_low = 0.5_f32;
        let similarity_low = 1.0 - (distance_low / 2.0);
        assert!(similarity_low >= threshold);

        let distance_high = 1.0_f32;
        let similarity_high = 1.0 - (distance_high / 2.0);
        assert!(similarity_high < threshold);
    }

    #[test]
    fn test_hook_output_serialization() {
        let output = HookOutput {
            hook_event_name: "SessionStart".to_string(),
            additional_context: "# Context\n\nSome content".to_string(),
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("hookEventName"));
        assert!(json.contains("SessionStart"));
    }
}
