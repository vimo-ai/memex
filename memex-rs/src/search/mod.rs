//! 混合检索模块 - FTS + 向量搜索 + RRF 融合

#![allow(dead_code)] // 预留 API: is_semantic_available

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::domain::ms_to_local_iso;
use crate::embedding::OllamaClient;
use crate::shared_adapter::SharedDbAdapter;
use crate::vector::VectorStore;

/// RRF 融合常数 (标准值为 60)
const RRF_K: f64 = 60.0;

/// 将日期字符串 (YYYY-MM-DD) 转换为时间戳（毫秒）
///
/// - `is_start`: true 表示一天的开始 (00:00:00)，false 表示一天的结束 (23:59:59.999)
fn date_to_timestamp(date: &str, is_start: bool) -> Option<i64> {
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime, Local, TimeZone};

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
    /// 消息 ID
    pub message_id: i64,
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

/// 混合检索服务
pub struct HybridSearchService {
    db: Arc<SharedDbAdapter>,
    ollama: Option<Arc<OllamaClient>>,
    vector: Option<Arc<RwLock<VectorStore>>>,
}

impl HybridSearchService {
    /// 创建混合检索服务
    pub fn new(
        db: Arc<SharedDbAdapter>,
        ollama: Option<Arc<OllamaClient>>,
        vector: Option<Arc<RwLock<VectorStore>>>,
    ) -> Self {
        Self { db, ollama, vector }
    }

    /// 执行混合搜索
    pub async fn search(&self, options: HybridSearchOptions) -> Result<Vec<HybridSearchResult>> {
        let HybridSearchOptions {
            query,
            limit,
            project_id,
            mode,
            order_by,
            start_date,
            end_date,
        } = options;

        if query.trim().is_empty() {
            return Ok(vec![]);
        }

        // 时间排序时自动降级为 FTS-only 模式
        let effective_mode = if order_by != SearchOrderBy::Score {
            tracing::info!(
                "[Hybrid Search] order_by={:?}, 降级为 FTS-only 模式",
                order_by
            );
            SearchMode::Fts
        } else {
            mode
        };

        tracing::info!(
            "[Hybrid Search] query=\"{}\", mode={:?}, order_by={:?}, limit={}, date_range={:?}~{:?}",
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
            match self.db.search_fts_full(&query, limit * 2, project_id, db_order_by, start_timestamp, end_timestamp).await {
                Ok(results) => {
                    tracing::debug!("[FTS] Returned {} results", results.len());
                    // Convert to domain::SearchResult
                    fts_results = results.into_iter().map(Into::into).collect();
                }
                Err(e) => {
                    tracing::warn!("[FTS] Search failed: {}", e);
                }
            }
        }

        // Vector search (只在 Score 排序时执行，且日期过滤需要在后续处理)
        if effective_mode == SearchMode::Vector || effective_mode == SearchMode::Hybrid {
            if let (Some(ollama), Some(vector)) = (&self.ollama, &self.vector) {
                match self.vector_search(ollama, vector, &query, limit * 2).await {
                    Ok(results) => {
                        tracing::debug!("[Vector] Returned {} results", results.len());
                        vector_results = results;
                    }
                    Err(e) => {
                        tracing::warn!("[Vector] Search failed, falling back to FTS: {}", e);
                    }
                }
            } else {
                tracing::debug!("[Vector] Ollama/VectorStore unavailable");
            }
        }

        // 如果都没结果
        if fts_results.is_empty() && vector_results.is_empty() {
            return Ok(vec![]);
        }

        // RRF 融合（时间排序时跳过，直接使用 FTS 结果）
        let fused = if order_by != SearchOrderBy::Score {
            // 时间排序：直接将 FTS 结果转换为 HybridSearchResult
            fts_results
                .into_iter()
                .enumerate()
                .map(|(idx, r)| HybridSearchResult {
                    message_id: r.message_id,
                    session_id: r.session_id,
                    project_id: r.project_id,
                    project_name: r.project_name,
                    message_type: r.r#type,
                    content: r.content,
                    snippet: Some(r.snippet),
                    score: r.score,
                    timestamp: r.timestamp,
                    sources: SearchSources { fts: true, vector: false },
                    fts_rank: Some(idx + 1),
                    vector_distance: None,
                    chunk_index: None,
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
        ollama: &OllamaClient,
        vector: &RwLock<VectorStore>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<VectorSearchItem>> {
        // 生成查询向量
        let query_embedding = ollama.embed(query).await?;

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
                    let project_name = if let Ok(Some(project)) = self.db.get_project(session.project_id).await {
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
                    message_id: result.message_id,
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
                    message_id: result.message_id,
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
                });
        }

        // 按 RRF 得分排序
        let mut results: Vec<HybridSearchResult> = score_map.into_values().collect();
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        results
    }

    /// 检查语义搜索是否可用
    pub async fn is_semantic_available(&self) -> bool {
        let ollama_ok = if let Some(ollama) = &self.ollama {
            ollama.is_available().await
        } else {
            false
        };

        let vector_ok = if let Some(vector) = &self.vector {
            vector.read().await.count().await.unwrap_or(0) > 0
        } else {
            false
        };

        ollama_ok && vector_ok
    }
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
