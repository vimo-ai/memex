//! 注入模块 - Claude Code Hook 上下文注入
//!
//! 支持四种模式：
//! - None: 不自动注入，纯 Pull 模式（MCP 主动查询）
//! - Full: SessionStart 时全量注入最近 L3 摘要
//! - Combine: 向量匹配，合并所有 sources 结果
//! - Fallback: 向量匹配，按 sources 顺序尝试，有结果即停
//!
//! 数据源 (sources):
//! - messages: 原始消息（L0）
//! - observations: 工具调用观察（L1）
//! - talks: 对话摘要（L2）
//! - sessions: 会话摘要（L3）
//! - summaries: L1+L2+L3 的快捷方式

use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::compact::{
    CompactDB, CompactLevel, CompactVectorStore, InjectConfig, InjectMode, InjectSource,
    SessionSummary, VectorDistanceType,
};
use crate::llm::EmbeddingProvider;
use crate::shared_adapter::SharedDbAdapter;
use crate::vector::VectorStore;

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
    db: Arc<SharedDbAdapter>,
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
    pub fn new(
        db: Arc<SharedDbAdapter>,
        compact_db: Arc<CompactDB>,
        config: InjectConfig,
    ) -> Self {
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

    /// 执行注入
    ///
    /// - `mode`: 注入模式（如果为 None，使用配置中的默认模式）
    /// - `query`: 用户查询（Combine/Fallback 模式需要）
    /// - `project_path`: 项目路径（可选，用于项目过滤）
    pub async fn inject(
        &self,
        mode: Option<InjectMode>,
        query: Option<&str>,
        project_path: Option<&str>,
    ) -> Result<InjectResult> {
        let mode = mode.unwrap_or(self.config.mode);

        match mode {
            InjectMode::None => Ok(InjectResult {
                context: String::new(),
                count: 0,
                mode: "none".to_string(),
                estimated_tokens: 0,
            }),
            InjectMode::Full => self.inject_full(project_path).await,
            InjectMode::Combine => {
                let query = query.ok_or_else(|| anyhow::anyhow!("Combine 模式需要提供 query"))?;
                self.inject_combine(query, project_path).await
            }
            InjectMode::Fallback => {
                let query = query.ok_or_else(|| anyhow::anyhow!("Fallback 模式需要提供 query"))?;
                self.inject_fallback(query, project_path).await
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

    /// Full 模式注入
    ///
    /// 获取最近的 Session Summaries (L3)，格式化为 Markdown
    async fn inject_full(&self, project_path: Option<&str>) -> Result<InjectResult> {
        let max_sessions = self.config.max_sessions();
        let max_tokens = self.config.max_tokens();

        // 获取项目 ID（如果指定了项目路径）
        let project_id = if let Some(path) = project_path {
            self.find_project_id_by_path(path).await?
        } else {
            None
        };

        // 获取最近的 Session Summaries
        let summaries = self
            .compact_db
            .get_recent_session_summaries(project_id, max_sessions)
            .await?;

        if summaries.is_empty() {
            return Ok(InjectResult {
                context: String::new(),
                count: 0,
                mode: "full".to_string(),
                estimated_tokens: 0,
            });
        }

        // 格式化为 Markdown，控制 token 数
        let mut context = String::from("# Memory Context (Recent Sessions)\n\n");
        let mut total_chars = context.len();
        let mut count = 0;

        for summary in summaries {
            let entry = self.format_session_summary(&summary).await;
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
            mode: "full".to_string(),
            estimated_tokens: total_chars / 4,
        })
    }

    /// Combine 模式注入
    ///
    /// 向量匹配，合并所有 sources 结果
    async fn inject_combine(&self, query: &str, project_path: Option<&str>) -> Result<InjectResult> {
        let sources = self.config.expanded_sources();
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
        let project_id = if self.config.project_scope() {
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
                .search_source(*source, &query_embedding, project_id)
                .await?;
            all_results.extend(results);
        }

        // 应用过滤和排序
        self.filter_and_rank(&mut all_results)?;

        // 格式化输出
        self.format_vector_results(all_results, "combine").await
    }

    /// Fallback 模式注入
    ///
    /// 按 sources 顺序尝试，有足够结果即停
    async fn inject_fallback(&self, query: &str, project_path: Option<&str>) -> Result<InjectResult> {
        let sources = self.config.expanded_sources();
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
        let project_id = if self.config.project_scope() {
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
                .search_source(*source, &query_embedding, project_id)
                .await?;

            // 应用过滤
            self.filter_and_rank(&mut results)?;

            // 如果有结果，返回
            if !results.is_empty() {
                return self.format_vector_results(results, "fallback").await;
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
    ) -> Result<Vec<ScoredResult>> {
        let limit = self.config.limit_per_source();
        let distance_type = self.config.distance_type();

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
                let msg_map: std::collections::HashMap<i64, _> = messages
                    .into_iter()
                    .map(|m| (m.id, m))
                    .collect();

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
                self.search_compact_level(CompactLevel::L1, query_embedding, limit)
                    .await
            }
            InjectSource::Talks => {
                self.search_compact_level(CompactLevel::L2, query_embedding, limit)
                    .await
            }
            InjectSource::Sessions => {
                self.search_compact_level(CompactLevel::L3, query_embedding, limit)
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
    ) -> Result<Vec<ScoredResult>> {
        let distance_type = self.config.distance_type();
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
                created_at: None, // TODO: 需要从数据库获取
            })
            .collect())
    }

    /// 过滤和排序结果
    fn filter_and_rank(&self, results: &mut Vec<ScoredResult>) -> Result<()> {
        let threshold = self.config.similarity_threshold();
        let distance_type = self.config.distance_type();
        let time_window = self.config.time_window_days();
        let time_decay = self.config.time_decay();
        let halflife = self.config.time_decay_halflife() as f64;

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

        let max_tokens = self.config.max_tokens();

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
