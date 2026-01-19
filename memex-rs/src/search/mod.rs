//! 混合检索模块 - FTS + 向量搜索 + RRF 融合
//!
//! 支持多级别搜索：
//! - Raw (L0): 原文搜索
//! - Observations (L1): 操作级摘要
//! - Talks (L2): 对话轮摘要
//! - Sessions (L3): 会话级摘要
//! - All: 全部级别

#![allow(dead_code)] // 预留 API: is_semantic_available

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::compact::{CompactDB, CompactLevel, CompactVectorStore, Observation, SessionSummary, TalkSummary};
use crate::domain::ms_to_local_iso;
use crate::llm::EmbeddingProvider;
use crate::shared_adapter::SharedDbAdapter;
use crate::vector::VectorStore;

/// RRF 融合常数 (标准值为 60)
const RRF_K: f64 = 60.0;

/// 将日期字符串 (YYYY-MM-DD) 转换为时间戳（毫秒）
///
/// - `is_start`: true 表示一天的开始 (00:00:00)，false 表示一天的结束 (23:59:59.999)
fn date_to_timestamp(date: &str, is_start: bool) -> Option<i64> {
    use chrono::{Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};

    let parsed = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let time = if is_start {
        NaiveTime::from_hms_opt(0, 0, 0)?
    } else {
        NaiveTime::from_hms_milli_opt(23, 59, 59, 999)?
    };
    let datetime = NaiveDateTime::new(parsed, time);

    // 转换为本地时区的时间戳
    let local_dt = Local.from_local_datetime(&datetime).single()?;
    Some(local_dt.timestamp_millis())
}

/// 混合搜索结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridSearchResult {
    /// 消息 ID (仅 Raw 级别有值，Compact 级别为 None)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<i64>,
    /// 会话 ID
    pub session_id: String,
    /// 项目 ID
    pub project_id: i64,
    /// 项目名称
    pub project_name: String,
    /// 消息类型
    pub message_type: String,
    /// 消息内容
    pub content: String,
    /// 匹配片段 (FTS 高亮或向量匹配的 chunk)
    pub snippet: Option<String>,
    /// RRF 融合得分
    pub score: f64,
    /// 消息时间戳
    pub timestamp: Option<String>,
    /// 来源标记
    pub sources: SearchSources,
    /// FTS 排名 (如果来自 FTS)
    pub fts_rank: Option<usize>,
    /// 向量距离 (如果来自向量搜索，越小越相似)
    pub vector_distance: Option<f32>,
    /// 匹配的分片索引
    pub chunk_index: Option<i64>,
    /// 搜索级别来源 (raw/l1/l2/l3)
    pub search_level: String,
    /// Compact 源记录 ID (仅 Compact 级别有值)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
}

/// 搜索来源标记
#[derive(Debug, Clone, Serialize, Default)]
pub struct SearchSources {
    pub fts: bool,
    pub vector: bool,
}

/// 搜索排序方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchOrderBy {
    /// 按相关性分数排序（默认）
    #[default]
    Score,
    /// 按时间倒序（最新优先）
    TimeDesc,
    /// 按时间正序（最早优先）
    TimeAsc,
}

impl From<SearchOrderBy> for claude_session_db::SearchOrderBy {
    fn from(order: SearchOrderBy) -> Self {
        match order {
            SearchOrderBy::Score => claude_session_db::SearchOrderBy::Score,
            SearchOrderBy::TimeDesc => claude_session_db::SearchOrderBy::TimeDesc,
            SearchOrderBy::TimeAsc => claude_session_db::SearchOrderBy::TimeAsc,
        }
    }
}

/// 混合搜索选项
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridSearchOptions {
    /// 搜索查询
    pub query: String,
    /// 返回数量
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// 项目 ID 过滤
    pub project_id: Option<i64>,
    /// 搜索模式: fts | vector | hybrid
    #[serde(default = "default_mode")]
    pub mode: SearchMode,
    /// 搜索级别: raw | observations | talks | sessions | all
    #[serde(default)]
    pub level: SearchLevel,
    /// 排序方式: score | time_desc | time_asc
    /// 注意：time_desc/time_asc 会自动降级为 FTS-only 模式
    #[serde(default)]
    pub order_by: SearchOrderBy,
    /// 开始日期 (YYYY-MM-DD)
    pub start_date: Option<String>,
    /// 结束日期 (YYYY-MM-DD)
    pub end_date: Option<String>,
}

fn default_limit() -> usize {
    20
}

fn default_mode() -> SearchMode {
    SearchMode::Hybrid
}

/// 搜索模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// 仅 FTS 全文搜索
    Fts,
    /// 仅向量语义搜索
    Vector,
    /// 混合搜索 (FTS + 向量 + RRF 融合)
    Hybrid,
}

/// 搜索级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchLevel {
    /// L0: 原文搜索（默认）
    #[default]
    Raw,
    /// L1: Observations 搜索
    Observations,
    /// L2: Talk Summaries 搜索
    Talks,
    /// L3: Session Summaries 搜索
    Sessions,
    /// 全部级别（L0 + L1 + L2 + L3）
    All,
}

impl SearchLevel {
    /// 转换为 CompactLevel（仅对 L1/L2/L3 有效）
    pub fn to_compact_level(&self) -> Option<CompactLevel> {
        match self {
            SearchLevel::Observations => Some(CompactLevel::L1),
            SearchLevel::Talks => Some(CompactLevel::L2),
            SearchLevel::Sessions => Some(CompactLevel::L3),
            _ => None,
        }
    }
}

/// 混合检索服务
pub struct HybridSearchService {
    db: Arc<SharedDbAdapter>,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
    /// L0 原文向量存储
    vector: Option<Arc<RwLock<VectorStore>>>,
    /// L1/L2/L3 摘要向量存储
    compact_vector: Option<Arc<RwLock<CompactVectorStore>>>,
    /// Compact 数据库 (L1/L2/L3 FTS) - 使用 std::sync::Mutex 因为 rusqlite::Connection 不是 Sync
    compact_db: Option<Arc<Mutex<CompactDB>>>,
}

impl HybridSearchService {
    /// 创建混合检索服务
    pub fn new(
        db: Arc<SharedDbAdapter>,
        embedding: Option<Arc<dyn EmbeddingProvider>>,
        vector: Option<Arc<RwLock<VectorStore>>>,
    ) -> Self {
        Self {
            db,
            embedding,
            vector,
            compact_vector: None,
            compact_db: None,
        }
    }

    /// 设置 Compact 向量存储
    pub fn with_compact_vector(mut self, compact_vector: Arc<RwLock<CompactVectorStore>>) -> Self {
        self.compact_vector = Some(compact_vector);
        self
    }

    /// 设置 Compact 数据库 (用于 FTS)
    pub fn with_compact_db(mut self, compact_db: Arc<Mutex<CompactDB>>) -> Self {
        self.compact_db = Some(compact_db);
        self
    }

    /// 执行混合搜索
    pub async fn search(&self, options: HybridSearchOptions) -> Result<Vec<HybridSearchResult>> {
        let HybridSearchOptions {
            query,
            limit,
            project_id,
            mode,
            level,
            order_by,
            start_date,
            end_date,
        } = options;

        if query.trim().is_empty() {
            return Ok(vec![]);
        }

        // All 级别：并行搜索 Raw + Compact，合并结果
        if level == SearchLevel::All {
            return self
                .search_all_levels(&query, mode, limit, project_id, order_by, start_date, end_date)
                .await;
        }

        // 非 Raw 级别的搜索路由到 compact 搜索
        if level != SearchLevel::Raw {
            return self
                .search_compact(&query, level, mode, limit, project_id, start_date, end_date)
                .await;
        }

        // Raw 级别搜索
        self.search_raw(&query, mode, limit, project_id, order_by, start_date, end_date).await
    }

    /// Raw (L0) 原文搜索
    #[allow(clippy::too_many_arguments)]
    async fn search_raw(
        &self,
        query: &str,
        mode: SearchMode,
        limit: usize,
        project_id: Option<i64>,
        order_by: SearchOrderBy,
        start_date: Option<String>,
        end_date: Option<String>,
    ) -> Result<Vec<HybridSearchResult>> {
        // 时间排序时自动降级为 FTS-only 模式
        let effective_mode = if order_by != SearchOrderBy::Score {
            tracing::info!(
                "[Raw Search] order_by={:?}, 降级为 FTS-only 模式",
                order_by
            );
            SearchMode::Fts
        } else {
            mode
        };

        tracing::info!(
            "[Raw Search] query=\"{}\", mode={:?}, order_by={:?}, limit={}, date_range={:?}~{:?}",
            query,
            effective_mode,
            order_by,
            limit,
            start_date,
            end_date
        );

        // 将日期字符串转换为时间戳（毫秒）
        let start_timestamp = start_date.as_ref().and_then(|d| date_to_timestamp(d, true));
        let end_timestamp = end_date.as_ref().and_then(|d| date_to_timestamp(d, false));

        // 根据模式执行搜索
        let mut fts_results: Vec<crate::domain::SearchResult> = Vec::new();
        let mut vector_results = Vec::new();

        // FTS 搜索（日期过滤在 SQL 层完成）
        if effective_mode == SearchMode::Fts || effective_mode == SearchMode::Hybrid {
            let db_order_by: claude_session_db::SearchOrderBy = order_by.into();
            match self
                .db
                .search_fts_full(
                    query,
                    limit * 2,
                    project_id,
                    db_order_by,
                    start_timestamp,
                    end_timestamp,
                )
                .await
            {
                Ok(results) => {
                    tracing::debug!("[Raw FTS] Returned {} results", results.len());
                    fts_results = results.into_iter().map(Into::into).collect();
                }
                Err(e) => {
                    tracing::warn!("[Raw FTS] Search failed: {}", e);
                }
            }
        }

        // Vector search (只在 Score 排序时执行)
        if effective_mode == SearchMode::Vector || effective_mode == SearchMode::Hybrid {
            if let (Some(embedding), Some(vector)) = (&self.embedding, &self.vector) {
                match self.vector_search(embedding, vector, query, limit * 2).await {
                    Ok(results) => {
                        tracing::debug!("[Raw Vector] Returned {} results", results.len());
                        vector_results = results;
                    }
                    Err(e) => {
                        tracing::warn!("[Raw Vector] Search failed: {}", e);
                    }
                }
            } else {
                tracing::debug!("[Raw Vector] Embedding/VectorStore unavailable");
            }
        }

        // 如果都没结果
        if fts_results.is_empty() && vector_results.is_empty() {
            return Ok(vec![]);
        }

        // RRF 融合（时间排序时跳过，直接使用 FTS 结果）
        let fused = if order_by != SearchOrderBy::Score {
            fts_results
                .into_iter()
                .enumerate()
                .map(|(idx, r)| HybridSearchResult {
                    message_id: Some(r.message_id),
                    session_id: r.session_id,
                    project_id: r.project_id,
                    project_name: r.project_name,
                    message_type: r.r#type,
                    content: r.content,
                    snippet: Some(r.snippet),
                    score: r.score,
                    timestamp: r.timestamp,
                    sources: SearchSources {
                        fts: true,
                        vector: false,
                    },
                    fts_rank: Some(idx + 1),
                    vector_distance: None,
                    chunk_index: None,
                    search_level: "raw".to_string(),
                    source_id: None,
                })
                .collect()
        } else {
            self.rrf_fusion(&fts_results, &vector_results, project_id)
        };

        // 返回 top N（日期过滤已在 SQL 层完成）
        Ok(fused.into_iter().take(limit).collect())
    }

    /// 向量搜索
    async fn vector_search(
        &self,
        embedding: &Arc<dyn EmbeddingProvider>,
        vector: &RwLock<VectorStore>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<VectorSearchItem>> {
        // 生成查询向量
        let query_embedding = embedding.embed(query).await?;

        // 执行向量搜索
        let vector_store = vector.read().await;
        let results = vector_store.search(&query_embedding, limit).await?;
        drop(vector_store);

        // 获取消息详情
        let message_ids: Vec<i64> = results.iter().map(|r| r.message_id).collect();
        let messages = self.db.get_messages_by_ids(&message_ids).await?;

        // 构建结果 (需要关联会话和项目信息)
        let mut items = Vec::new();
        for result in results {
            // 查找对应的消息
            let msg = messages.iter().find(|m| m.id == result.message_id);
            if let Some(msg) = msg {
                // 获取会话信息
                if let Ok(Some(session)) = self.db.get_session(&msg.session_id).await {
                    // 获取项目名称
                    let project_name =
                        if let Ok(Some(project)) = self.db.get_project(session.project_id).await {
                            project.name
                        } else {
                            "Unknown".to_string()
                        };

                    items.push(VectorSearchItem {
                        message_id: result.message_id,
                        session_id: msg.session_id.clone(),
                        project_id: session.project_id,
                        project_name,
                        message_type: msg.r#type.to_string(),
                        content: msg.content_text.clone(),
                        chunk_content: result.content,
                        chunk_index: result.chunk_index,
                        distance: result.distance,
                        timestamp: Some(ms_to_local_iso(msg.timestamp)),
                    });
                }
            }
        }

        Ok(items)
    }

    /// RRF (Reciprocal Rank Fusion) 排序融合
    ///
    /// 公式: score(d) = Σ 1/(k + rank_i(d))
    /// k = 60 (标准值)
    fn rrf_fusion(
        &self,
        fts_results: &[crate::domain::SearchResult],
        vector_results: &[VectorSearchItem],
        _project_id: Option<i64>,
    ) -> Vec<HybridSearchResult> {
        // 用 message_id 作为 key 聚合结果
        let mut score_map: HashMap<i64, HybridSearchResult> = HashMap::new();

        // 处理 FTS 结果
        for (index, result) in fts_results.iter().enumerate() {
            let rank = index + 1;
            let rrf_score = 1.0 / (RRF_K + rank as f64);

            score_map
                .entry(result.message_id)
                .and_modify(|existing| {
                    existing.score += rrf_score;
                    existing.sources.fts = true;
                    existing.fts_rank = Some(rank);
                    // 优先使用 FTS 的 snippet (带高亮)
                    if existing.snippet.is_none() {
                        existing.snippet = Some(result.snippet.clone());
                    }
                })
                .or_insert_with(|| HybridSearchResult {
                    message_id: Some(result.message_id),
                    session_id: result.session_id.clone(),
                    project_id: result.project_id,
                    project_name: result.project_name.clone(),
                    message_type: result.r#type.clone(),
                    content: result.content.clone(),
                    snippet: Some(result.snippet.clone()),
                    score: rrf_score,
                    timestamp: result.timestamp.clone(),
                    sources: SearchSources {
                        fts: true,
                        vector: false,
                    },
                    fts_rank: Some(rank),
                    vector_distance: None,
                    chunk_index: None,
                    search_level: "raw".to_string(),
                    source_id: None,
                });
        }

        // 处理向量搜索结果
        for (index, result) in vector_results.iter().enumerate() {
            let rank = index + 1;
            let rrf_score = 1.0 / (RRF_K + rank as f64);

            score_map
                .entry(result.message_id)
                .and_modify(|existing| {
                    existing.score += rrf_score;
                    existing.sources.vector = true;
                    // 如果向量距离更近，更新 snippet
                    if existing.vector_distance.is_none()
                        || result.distance < existing.vector_distance.unwrap()
                    {
                        existing.vector_distance = Some(result.distance);
                        existing.chunk_index = Some(result.chunk_index);
                        // 如果没有 FTS snippet，使用 chunk 内容
                        if existing.snippet.is_none() {
                            existing.snippet = Some(result.chunk_content.clone());
                        }
                    }
                })
                .or_insert_with(|| HybridSearchResult {
                    message_id: Some(result.message_id),
                    session_id: result.session_id.clone(),
                    project_id: result.project_id,
                    project_name: result.project_name.clone(),
                    message_type: result.message_type.clone(),
                    content: result.content.clone(),
                    snippet: Some(result.chunk_content.clone()),
                    score: rrf_score,
                    timestamp: result.timestamp.clone(),
                    sources: SearchSources {
                        fts: false,
                        vector: true,
                    },
                    fts_rank: None,
                    vector_distance: Some(result.distance),
                    chunk_index: Some(result.chunk_index),
                    search_level: "raw".to_string(),
                    source_id: None,
                });
        }

        // 按 RRF 得分排序
        let mut results: Vec<HybridSearchResult> = score_map.into_values().collect();
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results
    }

    /// 检查语义搜索是否可用
    pub async fn is_semantic_available(&self) -> bool {
        let embedding_ok = if let Some(embedding) = &self.embedding {
            embedding.is_available().await
        } else {
            false
        };

        let vector_ok = if let Some(vector) = &self.vector {
            vector.read().await.count().await.unwrap_or(0) > 0
        } else {
            false
        };

        embedding_ok && vector_ok
    }

    /// All 级别搜索：并行搜索 Raw + Compact，合并结果
    #[allow(clippy::too_many_arguments)]
    async fn search_all_levels(
        &self,
        query: &str,
        mode: SearchMode,
        limit: usize,
        project_id: Option<i64>,
        order_by: SearchOrderBy,
        start_date: Option<String>,
        end_date: Option<String>,
    ) -> Result<Vec<HybridSearchResult>> {
        tracing::info!(
            "[All Levels Search] query=\"{}\", mode={:?}, limit={}",
            query,
            mode,
            limit
        );

        // 并行搜索 Raw 和 Compact
        let (raw_results, compact_results) = tokio::join!(
            self.search_raw(query, mode, limit * 2, project_id, order_by, start_date.clone(), end_date.clone()),
            self.search_compact(query, SearchLevel::All, mode, limit * 2, project_id, start_date.clone(), end_date.clone())
        );

        let raw_results = raw_results.unwrap_or_default();
        let compact_results = compact_results.unwrap_or_default();

        tracing::debug!(
            "[All Levels Search] Raw: {} results, Compact: {} results",
            raw_results.len(),
            compact_results.len()
        );

        // 合并并按分数排序
        let mut all_results: Vec<HybridSearchResult> = raw_results
            .into_iter()
            .chain(compact_results.into_iter())
            .collect();

        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(all_results.into_iter().take(limit).collect())
    }

    /// Compact 层搜索 (L1/L2/L3)
    ///
    /// 支持 FTS + Vector + Hybrid 三种模式
    /// 支持 project_id 和日期范围过滤
    #[allow(clippy::too_many_arguments)]
    async fn search_compact(
        &self,
        query: &str,
        level: SearchLevel,
        mode: SearchMode,
        limit: usize,
        project_id: Option<i64>,
        start_date: Option<String>,
        end_date: Option<String>,
    ) -> Result<Vec<HybridSearchResult>> {
        tracing::info!(
            "[Compact Search] query=\"{}\", level={:?}, mode={:?}, limit={}, project_id={:?}, date_range={:?}~{:?}",
            query,
            level,
            mode,
            limit,
            project_id,
            start_date,
            end_date
        );

        let mut fts_results: Vec<CompactFtsItem> = Vec::new();
        let mut vector_results: Vec<CompactVectorItem> = Vec::new();

        // FTS 搜索
        if mode == SearchMode::Fts || mode == SearchMode::Hybrid {
            if let Some(compact_db) = &self.compact_db {
                let db = compact_db.lock().await;
                fts_results = self.compact_fts_search(&db, query, level, limit * 2).await?;
                tracing::debug!("[Compact FTS] Found {} results", fts_results.len());
            } else {
                tracing::warn!("[Compact Search] CompactDB not available for FTS");
            }
        }

        // Vector 搜索
        if mode == SearchMode::Vector || mode == SearchMode::Hybrid {
            if let (Some(embedding), Some(compact_vector)) = (&self.embedding, &self.compact_vector) {
                vector_results = self.compact_vector_search(embedding, compact_vector, query, level, limit * 2).await?;
                tracing::debug!("[Compact Vector] Found {} results", vector_results.len());
            } else {
                tracing::warn!("[Compact Search] Embedding/CompactVector not available");
            }
        }

        // 如果都没结果
        if fts_results.is_empty() && vector_results.is_empty() {
            return Ok(vec![]);
        }

        // RRF 融合
        let fused = self.compact_rrf_fusion(&fts_results, &vector_results).await;

        // 应用过滤（project_id 和日期范围）
        let filtered = self.filter_compact_results(fused, project_id, start_date, end_date).await;

        Ok(filtered.into_iter().take(limit).collect())
    }

    /// 过滤 Compact 搜索结果
    ///
    /// 通过 session_id 查找 project_id 进行过滤，
    /// 通过 timestamp 进行日期范围过滤
    async fn filter_compact_results(
        &self,
        results: Vec<HybridSearchResult>,
        project_id: Option<i64>,
        start_date: Option<String>,
        end_date: Option<String>,
    ) -> Vec<HybridSearchResult> {
        // 如果没有过滤条件，直接返回
        if project_id.is_none() && start_date.is_none() && end_date.is_none() {
            return results;
        }

        // 转换日期为时间戳（用于比较）
        let start_ts = start_date.as_ref().and_then(|d| date_to_timestamp(d, true));
        let end_ts = end_date.as_ref().and_then(|d| date_to_timestamp(d, false));

        let original_count = results.len();
        let mut filtered = Vec::new();

        for result in results {
            // project_id 过滤
            if let Some(target_project_id) = project_id {
                if result.project_id != target_project_id {
                    continue;
                }
            }

            // 日期过滤（通过 timestamp 字段）
            if let Some(ref ts_str) = result.timestamp {
                // 解析 ISO 时间戳
                if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                    let ts_millis = ts.timestamp_millis();

                    if let Some(start) = start_ts {
                        if ts_millis < start {
                            continue;
                        }
                    }

                    if let Some(end) = end_ts {
                        if ts_millis > end {
                            continue;
                        }
                    }
                }
            }

            filtered.push(result);
        }

        tracing::debug!(
            "[Compact Filter] Filtered {} -> {} results (project_id={:?}, date={:?}~{:?})",
            original_count,
            filtered.len(),
            project_id,
            start_date,
            end_date
        );

        filtered
    }

    /// Compact FTS 搜索
    async fn compact_fts_search(
        &self,
        compact_db: &CompactDB,
        query: &str,
        level: SearchLevel,
        limit: usize,
    ) -> Result<Vec<CompactFtsItem>> {
        let mut results = Vec::new();

        // 根据级别搜索对应的 FTS 表
        match level {
            SearchLevel::Observations => {
                let obs = compact_db.search_observations(query, limit).await?;
                results.extend(obs.into_iter().map(CompactFtsItem::from_observation));
            }
            SearchLevel::Talks => {
                let talks = compact_db.search_talk_summaries(query, limit).await?;
                results.extend(talks.into_iter().map(CompactFtsItem::from_talk_summary));
            }
            SearchLevel::Sessions => {
                let sessions = compact_db.search_session_summaries(query, limit).await?;
                results.extend(sessions.into_iter().map(CompactFtsItem::from_session_summary));
            }
            SearchLevel::All => {
                // 搜索所有 compact 级别
                let per_level_limit = limit / 3 + 1;
                let obs = compact_db.search_observations(query, per_level_limit).await?;
                let talks = compact_db.search_talk_summaries(query, per_level_limit).await?;
                let sessions = compact_db.search_session_summaries(query, per_level_limit).await?;
                results.extend(obs.into_iter().map(CompactFtsItem::from_observation));
                results.extend(talks.into_iter().map(CompactFtsItem::from_talk_summary));
                results.extend(sessions.into_iter().map(CompactFtsItem::from_session_summary));
            }
            SearchLevel::Raw => {
                // 不应该到这里，Raw 级别不走 compact 搜索
            }
        }

        Ok(results)
    }

    /// Compact Vector 搜索
    async fn compact_vector_search(
        &self,
        embedding: &Arc<dyn EmbeddingProvider>,
        compact_vector: &Arc<RwLock<CompactVectorStore>>,
        query: &str,
        level: SearchLevel,
        limit: usize,
    ) -> Result<Vec<CompactVectorItem>> {
        let query_embedding = embedding.embed(query).await?;
        let compact_level = level.to_compact_level();

        let store = compact_vector.read().await;
        let results = store.search(&query_embedding, compact_level, limit).await?;
        drop(store);

        Ok(results.into_iter().map(|r| CompactVectorItem {
            source_id: r.source_id,
            session_id: r.session_id,
            level: r.level,
            text: r.text,
            prompt_number: r.prompt_number,
            distance: r.distance,
        }).collect())
    }

    /// Compact RRF 融合
    async fn compact_rrf_fusion(
        &self,
        fts_results: &[CompactFtsItem],
        vector_results: &[CompactVectorItem],
    ) -> Vec<HybridSearchResult> {
        // 用 source_id 作为 key 聚合
        let mut score_map: HashMap<String, HybridSearchResult> = HashMap::new();

        // 处理 FTS 结果
        for (idx, item) in fts_results.iter().enumerate() {
            let rank = idx + 1;
            let rrf_score = 1.0 / (RRF_K + rank as f64);

            let (project_id, project_name) = self.get_project_info(&item.session_id).await;

            score_map
                .entry(item.source_id.clone())
                .and_modify(|existing| {
                    existing.score += rrf_score;
                    existing.sources.fts = true;
                    existing.fts_rank = Some(rank);
                })
                .or_insert_with(|| HybridSearchResult {
                    message_id: None,
                    session_id: item.session_id.clone(),
                    project_id,
                    project_name,
                    message_type: format!("compact_{}", item.level),
                    content: item.content.clone(),
                    snippet: Some(item.snippet.clone()),
                    score: rrf_score,
                    timestamp: Some(item.created_at.clone()),
                    sources: SearchSources { fts: true, vector: false },
                    fts_rank: Some(rank),
                    vector_distance: None,
                    chunk_index: item.prompt_number.map(|n| n as i64),
                    search_level: item.level.clone(),
                    source_id: Some(item.source_id.clone()),
                });
        }

        // 处理 Vector 结果
        for (idx, item) in vector_results.iter().enumerate() {
            let rank = idx + 1;
            let rrf_score = 1.0 / (RRF_K + rank as f64);

            let (project_id, project_name) = self.get_project_info(&item.session_id).await;

            score_map
                .entry(item.source_id.clone())
                .and_modify(|existing| {
                    existing.score += rrf_score;
                    existing.sources.vector = true;
                    if existing.vector_distance.is_none() || item.distance < existing.vector_distance.unwrap() {
                        existing.vector_distance = Some(item.distance);
                    }
                })
                .or_insert_with(|| HybridSearchResult {
                    message_id: None,
                    session_id: item.session_id.clone(),
                    project_id,
                    project_name,
                    message_type: format!("compact_{}", item.level),
                    content: item.text.clone(),
                    snippet: Some(item.text.clone()),
                    score: rrf_score,
                    timestamp: None,
                    sources: SearchSources { fts: false, vector: true },
                    fts_rank: None,
                    vector_distance: Some(item.distance),
                    chunk_index: item.prompt_number.map(|n| n as i64),
                    search_level: item.level.clone(),
                    source_id: Some(item.source_id.clone()),
                });
        }

        // 按分数排序
        let mut results: Vec<HybridSearchResult> = score_map.into_values().collect();
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    /// 获取项目信息
    async fn get_project_info(&self, session_id: &str) -> (i64, String) {
        if let Ok(Some(session)) = self.db.get_session(session_id).await {
            let name = if let Ok(Some(project)) = self.db.get_project(session.project_id).await {
                project.name
            } else {
                "Unknown".to_string()
            };
            (session.project_id, name)
        } else {
            (0, "Unknown".to_string())
        }
    }
}

/// Compact FTS 搜索中间结果
struct CompactFtsItem {
    source_id: String,
    session_id: String,
    level: String,
    content: String,
    snippet: String,
    prompt_number: Option<i32>,
    created_at: String,
}

impl CompactFtsItem {
    fn from_observation(obs: Observation) -> Self {
        let content = format!(
            "{}\n{}",
            obs.title,
            obs.narrative.as_deref().unwrap_or("")
        );
        Self {
            source_id: obs.id,
            session_id: obs.session_id,
            level: "l1".to_string(),
            content: content.clone(),
            snippet: content,
            prompt_number: Some(obs.prompt_number),
            created_at: obs.created_at,
        }
    }

    fn from_talk_summary(talk: TalkSummary) -> Self {
        let content = format!(
            "{}\n{}",
            talk.user_request.as_deref().unwrap_or(""),
            talk.summary
        );
        Self {
            source_id: talk.id,
            session_id: talk.session_id,
            level: "l2".to_string(),
            content: content.clone(),
            snippet: content,
            prompt_number: Some(talk.prompt_number),
            created_at: talk.created_at,
        }
    }

    fn from_session_summary(session: SessionSummary) -> Self {
        Self {
            source_id: session.id,
            session_id: session.session_id,
            level: "l3".to_string(),
            content: session.summary.clone(),
            snippet: session.summary,
            prompt_number: None,
            created_at: session.created_at,
        }
    }
}

/// Compact Vector 搜索中间结果
struct CompactVectorItem {
    source_id: String,
    session_id: String,
    level: String,
    text: String,
    prompt_number: Option<i32>,
    distance: f32,
}

/// 向量搜索中间结果
struct VectorSearchItem {
    message_id: i64,
    session_id: String,
    project_id: i64,
    project_name: String,
    message_type: String,
    content: String,
    chunk_content: String,
    chunk_index: i64,
    distance: f32,
    timestamp: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rrf_score() {
        // rank 1: 1/(60+1) ≈ 0.0164
        // rank 2: 1/(60+2) ≈ 0.0161
        let score1 = 1.0 / (RRF_K + 1.0);
        let score2 = 1.0 / (RRF_K + 2.0);
        assert!(score1 > score2);
        assert!((score1 - 0.0164).abs() < 0.001);
    }
}
