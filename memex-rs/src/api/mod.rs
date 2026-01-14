//! HTTP API 层 - Axum 路由

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::backup::BackupService;
use crate::collector::Collector;
use crate::config::Config;
use crate::domain::{MessageListDto, ProjectListDto, SessionListDto, SessionSearchDto};
use crate::embedding::OllamaClient;
use crate::indexer::VectorIndexer;
use crate::rag::{RagOptions, RagService};
use crate::search::{HybridSearchOptions, HybridSearchResult, HybridSearchService};
use crate::shared_adapter::SharedDbAdapter;
use crate::vector::VectorStore;

/// 应用状态
pub struct AppState {
    pub config: Config,
    /// 共享数据库适配器（异步）
    pub db: Arc<SharedDbAdapter>,
    pub collector: Collector,
    pub backup: BackupService,
    pub ollama: Option<Arc<OllamaClient>>,
    pub vector: Option<Arc<RwLock<VectorStore>>>,
    pub indexer: Option<VectorIndexer>,
    pub hybrid_search: HybridSearchService,
    pub rag_service: RagService,
}

/// 创建路由
pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        // 基础路由
        .route("/health", get(health))
        .route("/api/stats", get(get_stats))
        // MCP 协议
        .route("/api/mcp", post(crate::mcp::handle_mcp))
        .route("/api/mcp", get(crate::mcp::handle_mcp_get))
        .route("/api/mcp/info", get(crate::mcp::get_mcp_info))
        // 项目
        .route("/api/projects", get(get_projects))
        .route("/api/projects/{id}", get(get_project))
        .route("/api/projects/{id}/sessions", get(get_project_sessions))
        // 会话
        .route("/api/sessions", get(get_sessions))
        .route("/api/sessions/search", get(search_sessions))
        .route("/api/sessions/{id}", get(get_session))
        .route("/api/sessions/{id}/messages", get(get_session_messages))
        // 搜索
        .route("/api/search", get(search))
        .route("/api/search/semantic", get(semantic_search))
        .route("/api/search/semantic/status", get(semantic_status))
        .route("/api/search/hybrid", get(hybrid_search))
        // RAG 问答
        .route("/api/ask", post(ask))
        .route("/api/ask", get(ask_get))
        .route("/api/ask/status", get(ask_status))
        // 采集
        .route("/api/collect", post(collect))
        // 索引
        .route("/api/index", post(index_session_by_path))  // 精确索引（按路径）
        .route("/api/index/all", post(index_messages))     // 全量索引
        .route("/api/index/batch", post(index_batch))
        // 备份
        .route("/api/backup", post(create_backup))
        .route("/api/backup/list", get(list_backups))
        // Embedding 状态
        .route("/api/embedding/status", get(embedding_status))
        .route("/api/embedding/stats", get(embedding_stats))
        .route("/api/embedding/trigger", post(embedding_trigger))
        .route("/api/embedding/trigger-all", post(embedding_trigger_all))
        .route("/api/embedding/failed", get(embedding_failed))
        .route("/api/embedding/reset-failed", post(embedding_reset_failed))
        .route("/api/embedding/compact", post(embedding_compact))
        // Admin
        .route("/api/admin/stats", get(get_stats))
        .route("/api/admin/fix-metadata", post(fix_metadata))
        .route("/api/admin/merge-projects", post(merge_projects))
        .route("/api/admin/deduplicate-projects", post(deduplicate_projects))
        .with_state(state)
}

/// 健康检查
async fn health() -> &'static str {
    "OK"
}

// ==================== 统计 ====================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsResponse {
    project_count: i64,
    session_count: i64,
    message_count: i64,
    /// 语义搜索是否可用 (Ollama Embedding)
    semantic_search_enabled: bool,
    /// AI 问答是否启用 (Ollama Chat)
    ai_chat_enabled: bool,
}

async fn get_stats(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    let stats = state.db.get_stats().await?;
    Ok(Json(StatsResponse {
        project_count: stats.project_count,
        session_count: stats.session_count,
        message_count: stats.message_count,
        semantic_search_enabled: state.ollama.is_some(),
        ai_chat_enabled: state.config.enable_ai_chat && state.ollama.is_some(),
    }))
}

// ==================== 项目 ====================

async fn get_projects(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    use crate::domain::ProjectDto;
    let projects: Vec<ProjectDto> = state
        .db
        .list_projects()
        .await?
        .into_iter()
        .map(Into::into)
        .collect();

    let response = ProjectListDto {
        total: projects.len(),
        projects,
    };
    Ok(Json(response))
}

async fn get_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    use crate::domain::ProjectDetailDto;
    match state.db.get_project(id).await? {
        Some(project) => {
            let dto: ProjectDetailDto = project.into();
            Ok(Json(dto).into_response())
        }
        None => Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "项目不存在"})),
        )
            .into_response()),
    }
}

async fn get_project_sessions(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    use crate::domain::SessionDto;
    // 先检查项目是否存在
    if state.db.get_project(id).await?.is_none() {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "项目不存在"})),
        )
            .into_response());
    }

    let sessions: Vec<SessionDto> = state
        .db
        .get_sessions(Some(id), 1000)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();

    let response = SessionListDto {
        total: sessions.len(),
        sessions,
    };
    Ok(Json(response).into_response())
}

// ==================== 会话 ====================

#[derive(Debug, Deserialize)]
pub struct SessionsQuery {
    #[serde(rename = "projectId")]
    project_id: Option<i64>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    50
}

#[derive(Debug, Deserialize)]
pub struct SessionSearchQuery {
    #[serde(rename = "idPrefix")]
    id_prefix: Option<String>,
    #[serde(default = "default_search_session_limit")]
    limit: usize,
}

fn default_search_session_limit() -> usize {
    20
}

async fn get_sessions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SessionsQuery>,
) -> Result<impl IntoResponse, AppError> {
    use crate::domain::SessionDto;
    let sessions: Vec<SessionDto> = state
        .db
        .get_sessions(query.project_id, query.limit)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();

    let response = SessionListDto {
        total: sessions.len(),
        sessions,
    };
    Ok(Json(response))
}

/// 会话 ID 前缀搜索
async fn search_sessions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SessionSearchQuery>,
) -> Result<impl IntoResponse, AppError> {
    use crate::domain::SessionDto;
    let id_prefix = query.id_prefix.unwrap_or_default();

    if id_prefix.trim().is_empty() {
        return Ok(Json(SessionSearchDto {
            query: String::new(),
            total: 0,
            sessions: vec![],
        }));
    }

    let sessions: Vec<SessionDto> = state
        .db
        .search_sessions_by_prefix(&id_prefix, query.limit)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();

    let response = SessionSearchDto {
        query: id_prefix,
        total: sessions.len(),
        sessions,
    };
    Ok(Json(response))
}

async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    use crate::domain::SessionDto;
    match state.db.get_session(&id).await? {
        Some(session) => {
            let dto: SessionDto = session.into();
            Ok(Json(dto).into_response())
        }
        None => Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "会话不存在"})),
        )
            .into_response()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MessagesQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    order: Option<String>,
}

async fn get_session_messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<MessagesQuery>,
) -> Result<impl IntoResponse, AppError> {
    use crate::domain::MessageDto;
    // 先获取总消息数（分页前）
    let total = state.db.get_session_message_count(&id).await? as usize;

    let desc = query.order.as_deref() == Some("desc");
    let messages: Vec<MessageDto> = state
        .db
        .get_messages_with_options(&id, query.limit, desc)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();

    let response = MessageListDto { total, messages };
    Ok(Json(response))
}

// ==================== 搜索 ====================

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    q: String,
    #[serde(default = "default_search_limit")]
    limit: usize,
    #[serde(rename = "projectId")]
    project_id: Option<i64>,
}

fn default_search_limit() -> usize {
    20
}

#[derive(Serialize)]
struct SearchResponse {
    results: Vec<crate::domain::SearchResult>,
    total: usize,
}

async fn search(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<impl IntoResponse, AppError> {
    if query.q.trim().is_empty() {
        return Ok(Json(SearchResponse {
            results: vec![],
            total: 0,
        }));
    }

    // 使用 SharedDbAdapter 的 FTS 搜索
    let results = if let Some(project_id) = query.project_id {
        state.db.search_fts_with_project(&query.q, query.limit, Some(project_id)).await?
    } else {
        state.db.search_fts(&query.q, query.limit).await?
    };

    // 转换为 domain::SearchResult
    let results: Vec<crate::domain::SearchResult> = results
        .into_iter()
        .map(|r| crate::domain::SearchResult {
            message_id: r.message_id,
            session_id: r.session_id,
            project_id: r.project_id,
            project_name: r.project_name,
            r#type: r.r#type,
            content: r.content_full.clone(),
            snippet: r.snippet,
            score: r.score,
            timestamp: r.timestamp.map(crate::domain::ms_to_local_iso),
        })
        .collect();
    let total = results.len();

    Ok(Json(SearchResponse { results, total }))
}

/// 语义搜索查询参数（与 hybrid_search 一致）
#[derive(Debug, Deserialize)]
pub struct SemanticSearchQuery {
    q: String,
    #[serde(default = "default_search_limit")]
    limit: usize,
    #[serde(rename = "projectId")]
    project_id: Option<i64>,
    #[serde(default)]
    mode: Option<String>,
    /// 排序方式: score / time_desc / time_asc
    #[serde(rename = "orderBy", default)]
    order_by: Option<String>,
    #[serde(rename = "startDate")]
    start_date: Option<String>,
    #[serde(rename = "endDate")]
    end_date: Option<String>,
}

/// 语义搜索 - 实际调用 hybrid_search 服务，统一返回格式
async fn semantic_search(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SemanticSearchQuery>,
) -> Result<impl IntoResponse, AppError> {
    if query.q.trim().is_empty() {
        return Ok(Json(HybridSearchResponse {
            results: vec![],
            total: 0,
        })
        .into_response());
    }

    // 解析搜索模式（默认 hybrid）
    let mode = match query.mode.as_deref() {
        Some("fts") => crate::search::SearchMode::Fts,
        Some("vector") => crate::search::SearchMode::Vector,
        _ => crate::search::SearchMode::Hybrid,
    };

    let order_by = match query.order_by.as_deref() {
        Some("time_desc") => crate::search::SearchOrderBy::TimeDesc,
        Some("time_asc") => crate::search::SearchOrderBy::TimeAsc,
        _ => crate::search::SearchOrderBy::Score,
    };

    let options = HybridSearchOptions {
        query: query.q,
        limit: query.limit,
        project_id: query.project_id,
        mode,
        order_by,
        start_date: query.start_date,
        end_date: query.end_date,
    };

    let results = state.hybrid_search.search(options).await?;
    let total = results.len();

    Ok(Json(HybridSearchResponse { results, total }).into_response())
}

/// 语义搜索状态
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SemanticStatusResponse {
    available: bool,
    ollama_connected: bool,
    vector_count: usize,
    embedding_model: String,
}

async fn semantic_status(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let ollama_connected = if let Some(ollama) = &state.ollama {
        ollama.is_available().await
    } else {
        false
    };

    let vector_count = if let Some(vector) = &state.vector {
        vector.read().await.count().await.unwrap_or(0)
    } else {
        0
    };

    let available = ollama_connected && vector_count > 0;

    Ok(Json(SemanticStatusResponse {
        available,
        ollama_connected,
        vector_count,
        embedding_model: state.config.embedding_model.clone(),
    }))
}

// ==================== 混合搜索 ====================

#[derive(Debug, Deserialize)]
pub struct HybridSearchQuery {
    q: String,
    #[serde(default = "default_search_limit")]
    limit: usize,
    #[serde(rename = "projectId")]
    project_id: Option<i64>,
    #[serde(default)]
    mode: Option<String>,
    /// 排序方式: score / time_desc / time_asc
    #[serde(rename = "orderBy", default)]
    order_by: Option<String>,
    #[serde(rename = "startDate")]
    start_date: Option<String>,
    #[serde(rename = "endDate")]
    end_date: Option<String>,
}

#[derive(Serialize)]
struct HybridSearchResponse {
    results: Vec<HybridSearchResult>,
    total: usize,
}

async fn hybrid_search(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HybridSearchQuery>,
) -> Result<impl IntoResponse, AppError> {
    if query.q.trim().is_empty() {
        return Ok(Json(HybridSearchResponse {
            results: vec![],
            total: 0,
        })
        .into_response());
    }

    // 解析搜索模式
    let mode = match query.mode.as_deref() {
        Some("fts") => crate::search::SearchMode::Fts,
        Some("vector") => crate::search::SearchMode::Vector,
        _ => crate::search::SearchMode::Hybrid,
    };

    let order_by = match query.order_by.as_deref() {
        Some("time_desc") => crate::search::SearchOrderBy::TimeDesc,
        Some("time_asc") => crate::search::SearchOrderBy::TimeAsc,
        _ => crate::search::SearchOrderBy::Score,
    };

    let options = HybridSearchOptions {
        query: query.q,
        limit: query.limit,
        project_id: query.project_id,
        mode,
        order_by,
        start_date: query.start_date,
        end_date: query.end_date,
    };

    let results = state.hybrid_search.search(options).await?;
    let total = results.len();

    Ok(Json(HybridSearchResponse { results, total }).into_response())
}

// ==================== RAG 问答 ====================

async fn ask(
    State(state): State<Arc<AppState>>,
    Json(options): Json<RagOptions>,
) -> Result<impl IntoResponse, AppError> {
    // 检查 AI 问答是否启用
    if !state.config.enable_ai_chat {
        return Ok((
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({
                "error": "AI 问答功能未启用。请设置环境变量 ENABLE_AI_CHAT=true 并确保 Ollama 已安装 chat 模型。"
            })),
        )
            .into_response());
    }

    let response = state.rag_service.ask(options).await?;
    Ok(Json(response).into_response())
}

/// GET /api/ask?q=xxx - 快捷查询（适合浏览器测试）
#[derive(Debug, Deserialize)]
pub struct AskQuery {
    q: String,
    #[serde(default, rename = "contextWindow")]
    context_window: Option<usize>,
    #[serde(default, rename = "maxSources")]
    max_sources: Option<usize>,
    #[serde(default, rename = "projectId")]
    project_id: Option<i64>,
}

async fn ask_get(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AskQuery>,
) -> Result<impl IntoResponse, AppError> {
    if query.q.trim().is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "参数 q (question) 是必需的"})),
        )
            .into_response());
    }

    // 检查 AI 问答是否启用
    if !state.config.enable_ai_chat {
        return Ok((
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({
                "error": "AI 问答功能未启用。请设置环境变量 ENABLE_AI_CHAT=true 并确保 Ollama 已安装 chat 模型。"
            })),
        )
            .into_response());
    }

    let options = RagOptions {
        question: query.q,
        context_window: query.context_window.unwrap_or(3),
        max_sources: query.max_sources.unwrap_or(5),
        project_id: query.project_id,
    };

    let response = state.rag_service.ask(options).await?;
    Ok(Json(response).into_response())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AskStatusResponse {
    /// AI 问答功能是否启用
    enabled: bool,
    /// Chat 模型名称
    chat_model: String,
    /// Ollama 是否已连接
    ollama_connected: bool,
}

async fn ask_status(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    let ollama_connected = if let Some(ollama) = &state.ollama {
        ollama.is_available().await
    } else {
        false
    };

    Ok(Json(AskStatusResponse {
        enabled: state.config.enable_ai_chat,
        chat_model: state.config.chat_model.clone(),
        ollama_connected,
    }))
}

// ==================== 采集 ====================

#[derive(Serialize)]
struct CollectResponse {
    projects_scanned: usize,
    sessions_scanned: usize,
    messages_inserted: usize,
    errors: Vec<String>,
}

async fn collect(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    // TODO: Collector 迁移后改为 async
    let result = state.collector.collect_all()?;

    Ok(Json(CollectResponse {
        projects_scanned: result.projects_scanned,
        sessions_scanned: result.sessions_scanned,
        messages_inserted: result.messages_inserted,
        errors: result.errors,
    }))
}

// ==================== 备份 ====================

#[derive(Serialize)]
struct BackupResponse {
    path: String,
    size: u64,
    timestamp: String,
}

async fn create_backup(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    let result = state.backup.backup()?;

    Ok(Json(BackupResponse {
        path: result.path.to_string_lossy().to_string(),
        size: result.size,
        timestamp: result.timestamp,
    }))
}

#[derive(Serialize)]
struct BackupListResponse {
    backups: Vec<BackupItem>,
}

#[derive(Serialize)]
struct BackupItem {
    name: String,
    size: u64,
    date: String,
}

async fn list_backups(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    let backups = state.backup.list_backups()?;

    Ok(Json(BackupListResponse {
        backups: backups
            .into_iter()
            .map(|b| BackupItem {
                name: b.name,
                size: b.size,
                date: b.date,
            })
            .collect(),
    }))
}

// ==================== Embedding 状态 ====================

#[derive(Serialize)]
struct EmbeddingStatusResponse {
    available: bool,
    model: String,
    ollama_connected: bool,
    indexed_count: usize,
}

async fn embedding_status(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let ollama_connected = if let Some(ollama) = &state.ollama {
        ollama.is_available().await
    } else {
        false
    };

    let indexed_count = if let Some(vector) = &state.vector {
        vector.read().await.count().await.unwrap_or(0)
    } else {
        0
    };

    Ok(Json(EmbeddingStatusResponse {
        available: state.ollama.is_some(),
        model: state.config.embedding_model.clone(),
        ollama_connected,
        indexed_count,
    }))
}

/// 增量索引触发
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingTriggerResponse {
    triggered: bool,
    indexed_messages: usize,
    indexed_chunks: usize,
    message: String,
}

async fn embedding_trigger(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let indexer = match &state.indexer {
        Some(i) => i,
        None => {
            return Ok(Json(EmbeddingTriggerResponse {
                triggered: false,
                indexed_messages: 0,
                indexed_chunks: 0,
                message: "Index service unavailable, RAG needs to be enabled".to_string(),
            })
            .into_response());
        }
    };

    // 执行增量索引 (batch 100)
    let result = indexer.index_batch(100).await?;

    Ok(Json(EmbeddingTriggerResponse {
        triggered: true,
        indexed_messages: result.indexed_messages,
        indexed_chunks: result.indexed_chunks,
        message: format!(
            "已索引 {} 条消息, {} 个 chunks",
            result.indexed_messages, result.indexed_chunks
        ),
    })
    .into_response())
}

/// 失败的索引列表
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingFailedResponse {
    failed_count: usize,
    failed_messages: Vec<FailedMessage>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FailedMessage {
    message_id: i64,
    session_id: String,
    content_preview: String,
}

/// 索引状态统计
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingStatsResponse {
    /// 待索引数量
    pending: usize,
    /// 索引失败数量
    failed: usize,
    /// 已索引数量（向量库中的 chunks）
    indexed: usize,
    /// Ollama 是否可用
    ollama_available: bool,
    /// Embedding 模型
    embedding_model: String,
    /// 后台索引任务是否正在运行
    is_running: bool,
}

async fn embedding_stats(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let indexer = match &state.indexer {
        Some(i) => i,
        None => {
            return Ok(Json(EmbeddingStatsResponse {
                pending: 0,
                failed: 0,
                indexed: 0,
                ollama_available: false,
                embedding_model: state.config.embedding_model.clone(),
                is_running: false,
            })
            .into_response());
        }
    };

    let stats = indexer.get_index_stats().await?;
    let ollama_available = if let Some(ollama) = &state.ollama {
        ollama.is_available().await
    } else {
        false
    };

    Ok(Json(EmbeddingStatsResponse {
        pending: stats.pending,
        failed: stats.failed,
        indexed: stats.indexed,
        ollama_available,
        embedding_model: state.config.embedding_model.clone(),
        is_running: indexer.is_running(),
    })
    .into_response())
}

async fn embedding_failed(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    // 从数据库获取失败的消息列表
    let failed_messages = state.db.get_failed_indexed_messages(100).await?;
    let failed_count = state.db.count_failed_indexed_messages().await? as usize;

    let messages: Vec<FailedMessage> = failed_messages
        .into_iter()
        .map(|m| {
            // 截取内容预览（前 100 字符）
            let preview = if m.content_text.len() > 100 {
                format!("{}...", &m.content_text[..100])
            } else {
                m.content_text.clone()
            };
            FailedMessage {
                message_id: m.id,
                session_id: m.session_id,
                content_preview: preview,
            }
        })
        .collect();

    Ok(Json(EmbeddingFailedResponse {
        failed_count,
        failed_messages: messages,
    }))
}

/// 全量索引触发（后台持续索引直到完成）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingTriggerAllResponse {
    triggered: bool,
    message: String,
}

async fn embedding_trigger_all(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let indexer = match &state.indexer {
        Some(i) => i.clone(),
        None => {
            return Ok(Json(EmbeddingTriggerAllResponse {
                triggered: false,
                message: "Index service unavailable, RAG needs to be enabled".to_string(),
            })
            .into_response());
        }
    };

    // Check if already running
    if indexer.is_running() {
        return Ok(Json(EmbeddingTriggerAllResponse {
            triggered: false,
            message: "Index task already running".to_string(),
        })
        .into_response());
    }

    // Mark as running
    indexer.set_running(true);

    // Start background task
    let db = state.db.clone();
    tokio::spawn(async move {
        tracing::info!("🚀 Starting background full indexing task");
        let mut total_indexed = 0usize;
        let mut total_failed = 0usize;
        let batch_size = 500; // Process 500 per batch

        loop {
            // Check if there are pending messages
            let pending = match db.count_unindexed_messages().await {
                Ok(n) => n as usize,
                Err(e) => {
                    tracing::error!("Failed to get pending count: {}", e);
                    break;
                }
            };

            if pending == 0 {
                tracing::info!("✅ Background indexing done: {} indexed, {} failed", total_indexed, total_failed);
                break;
            }

            // Process one batch
            match indexer.index_pending(batch_size).await {
                Ok(result) => {
                    total_indexed += result.indexed_messages;
                    total_failed += result.failed;
                    tracing::info!(
                        "📦 Index progress: batch {}, total {}, failed {}, remaining {}",
                        result.indexed_messages,
                        total_indexed,
                        total_failed,
                        pending.saturating_sub(result.indexed_messages)
                    );
                }
                Err(e) => {
                    tracing::error!("Index batch failed: {}", e);
                    // Continue with next batch
                }
            }

            // Brief pause to avoid overload
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        // Mark as finished
        indexer.set_running(false);
    });

    Ok(Json(EmbeddingTriggerAllResponse {
        triggered: true,
        message: "Background indexing started, check /api/embedding/stats for progress".to_string(),
    })
    .into_response())
}

/// 重置失败状态
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingResetFailedResponse {
    reset_count: usize,
    message: String,
}

async fn embedding_reset_failed(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let reset_count = state.db.reset_failed_indexed_messages().await?;

    Ok(Json(EmbeddingResetFailedResponse {
        reset_count,
        message: format!("Reset {} failed messages, ready for re-indexing", reset_count),
    }))
}

/// LanceDB 压缩（合并文件片段、清理旧版本）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingCompactResponse {
    success: bool,
    message: String,
}

async fn embedding_compact(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let vector = match &state.vector {
        Some(v) => v,
        None => {
            return Ok(Json(EmbeddingCompactResponse {
                success: false,
                message: "Vector store unavailable".to_string(),
            })
            .into_response());
        }
    };

    // 执行压缩
    let store = vector.read().await;
    match store.compact().await {
        Ok(()) => Ok(Json(EmbeddingCompactResponse {
            success: true,
            message: "LanceDB compaction completed (merged file fragments, cleaned up versions older than 7 days)".to_string(),
        })
        .into_response()),
        Err(e) => Ok(Json(EmbeddingCompactResponse {
            success: false,
            message: format!("Compaction failed: {}", e),
        })
        .into_response()),
    }
}

// ==================== 索引 ====================

/// 精确索引请求（按路径）
#[derive(Debug, Deserialize)]
struct IndexByPathRequest {
    path: String,
}

/// 精确索引响应
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexByPathResponse {
    success: bool,
    sessions_scanned: usize,
    messages_inserted: usize,
    new_message_ids: Vec<i64>,
    errors: Vec<String>,
}

/// 按路径精确索引会话（替代 file watcher 轮询）
async fn index_session_by_path(
    State(state): State<Arc<AppState>>,
    Json(req): Json<IndexByPathRequest>,
) -> Result<impl IntoResponse, AppError> {
    if req.path.trim().is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "path 参数不能为空"})),
        )
            .into_response());
    }

    // 调用采集服务解析并更新 FTS 索引
    let collect_result = match state.collector.collect_by_path(&req.path) {
        Ok(r) => r,
        Err(e) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response());
        }
    };

    // 可选：更新向量索引（如果有新消息且 indexer 可用）
    if !collect_result.new_message_ids.is_empty() {
        if let Some(indexer) = &state.indexer {
            if let Err(e) = indexer.index_by_ids(&collect_result.new_message_ids).await {
                tracing::warn!("Vector indexing failed: {}", e);
            }
        }
    }

    Ok(Json(IndexByPathResponse {
        success: collect_result.errors.is_empty(),
        sessions_scanned: collect_result.sessions_scanned,
        messages_inserted: collect_result.messages_inserted,
        new_message_ids: collect_result.new_message_ids,
        errors: collect_result.errors,
    })
    .into_response())
}

#[derive(Serialize)]
struct IndexResponse {
    total_messages: usize,
    indexed_messages: usize,
    indexed_chunks: usize,
    skipped: usize,
    errors: Vec<String>,
}

async fn index_messages(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let indexer = match &state.indexer {
        Some(i) => i,
        None => {
            return Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "索引服务不可用，需要启用 RAG"})),
            )
                .into_response());
        }
    };

    let result = indexer.index_all().await?;

    Ok(Json(IndexResponse {
        total_messages: result.total_messages,
        indexed_messages: result.indexed_messages,
        indexed_chunks: result.indexed_chunks,
        skipped: result.skipped,
        errors: result.errors,
    })
    .into_response())
}

#[derive(Debug, Deserialize)]
struct IndexBatchQuery {
    #[serde(default = "default_batch_limit")]
    limit: usize,
}

fn default_batch_limit() -> usize {
    100
}

async fn index_batch(
    State(state): State<Arc<AppState>>,
    Query(query): Query<IndexBatchQuery>,
) -> Result<impl IntoResponse, AppError> {
    let indexer = match &state.indexer {
        Some(i) => i,
        None => {
            return Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "索引服务不可用，需要启用 RAG"})),
            )
                .into_response());
        }
    };

    let result = indexer.index_batch(query.limit).await?;

    Ok(Json(IndexResponse {
        total_messages: result.total_messages,
        indexed_messages: result.indexed_messages,
        indexed_chunks: result.indexed_chunks,
        skipped: result.skipped,
        errors: result.errors,
    })
    .into_response())
}

// ==================== Admin ====================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FixMetadataResponse {
    sessions_without_cwd: i64,
    collect_result: CollectResponse,
}

async fn fix_metadata(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    // 统计没有 cwd 的会话数量
    let sessions_without_cwd = state.db.count_sessions_without_cwd().await?;

    // 触发采集 (TODO: Collector 迁移后改为 async)
    let collect_result = state.collector.collect_all()?;

    Ok(Json(FixMetadataResponse {
        sessions_without_cwd,
        collect_result: CollectResponse {
            projects_scanned: collect_result.projects_scanned,
            sessions_scanned: collect_result.sessions_scanned,
            messages_inserted: collect_result.messages_inserted,
            errors: collect_result.errors,
        },
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MergeProjectsResponse {
    merged_count: usize,
    deleted_count: usize,
    details: Vec<MergeDetail>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MergeDetail {
    path: String,
    source_project_id: i64,
    merged_from: Vec<i64>,
    sessions_moved: usize,
}

async fn merge_projects(State(state): State<Arc<AppState>>) -> Result<impl IntoResponse, AppError> {
    let mut merged_count = 0;
    let mut deleted_count = 0;
    let mut details = Vec::new();

    // 获取所有项目及其来源
    let projects = state.db.get_all_projects_with_source().await?;

    // 按 path 分组
    let mut path_groups: std::collections::HashMap<String, Vec<claude_session_db::ProjectWithSource>> =
        std::collections::HashMap::new();

    for project in projects {
        path_groups
            .entry(project.path.clone())
            .or_default()
            .push(project);
    }

    // 处理每个 path 组
    for (path, mut group) in path_groups {
        if group.len() <= 1 {
            continue; // 没有重复
        }

        // 优先选择 claude 源的项目作为目标
        group.sort_by(|a, b| {
            let a_is_claude = a.source == "claude";
            let b_is_claude = b.source == "claude";
            b_is_claude.cmp(&a_is_claude) // claude 源排在前面
        });

        let target = &group[0];
        let duplicates = &group[1..];

        let mut merged_from = Vec::new();
        let mut total_sessions_moved = 0;

        for dup in duplicates {
            // 移动会话到目标项目
            let moved = state.db.update_sessions_project_id(dup.id, target.id).await?;
            total_sessions_moved += moved;

            // 删除重复项目
            state.db.delete_project(dup.id).await?;

            merged_from.push(dup.id);
            deleted_count += 1;
        }

        if !merged_from.is_empty() {
            merged_count += 1;
            details.push(MergeDetail {
                path,
                source_project_id: target.id,
                merged_from,
                sessions_moved: total_sessions_moved,
            });
        }
    }

    Ok(Json(MergeProjectsResponse {
        merged_count,
        deleted_count,
        details,
    }))
}

/// 去重项目 - 按 path 合并，保留 session 数量最多的记录
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeduplicateProjectsResponse {
    merged_count: usize,
    deleted_ids: Vec<i64>,
}

async fn deduplicate_projects(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let (merged_count, deleted_ids) = state.db.deduplicate_projects().await?;

    Ok(Json(DeduplicateProjectsResponse {
        merged_count,
        deleted_ids,
    }))
}

// ==================== 错误处理 ====================

pub struct AppError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        tracing::error!("Request error: {:?}", self.0);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": self.0.to_string()
            })),
        )
            .into_response()
    }
}
