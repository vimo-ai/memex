//! Server 搜索 API
//!
//! 复用 HybridSearchService，对 server DB 执行 FTS / 向量 / 混合搜索。
//! Server 端没有 embedding provider，向量搜索依赖客户端推上来的向量数据。

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::search::{HybridSearchOptions, HybridSearchService, SearchLevel, SearchMode};

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    project_id: Option<i64>,
    #[serde(default)]
    start_date: Option<String>,
    #[serde(default)]
    end_date: Option<String>,
}

fn default_limit() -> usize {
    20
}

async fn handle_search(
    State(search): State<Arc<HybridSearchService>>,
    Query(params): Query<SearchQuery>,
) -> impl IntoResponse {
    let mode = match params.mode.as_deref() {
        Some("fts") => SearchMode::Fts,
        Some("vector") => SearchMode::Vector,
        _ => SearchMode::Hybrid,
    };

    let level = match params.level.as_deref() {
        Some("observations") => SearchLevel::Observations,
        Some("talks") => SearchLevel::Talks,
        Some("sessions") => SearchLevel::Sessions,
        Some("all") => SearchLevel::All,
        _ => SearchLevel::Raw,
    };

    let options = HybridSearchOptions {
        query: params.q,
        limit: params.limit.min(100),
        project_id: params.project_id,
        mode,
        level,
        order_by: Default::default(),
        start_date: params.start_date,
        end_date: params.end_date,
    };

    match search.search(options).await {
        Ok(results) => {
            let total = results.len();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "results": results,
                    "total": total,
                })),
            )
        }
        Err(e) => {
            tracing::error!("搜索失败: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("{e}"),
                })),
            )
        }
    }
}

pub fn create_search_router(search: Arc<HybridSearchService>) -> Router {
    Router::new()
        .route("/api/search", get(handle_search))
        .with_state(search)
}
