//! Memex Rust Backend
//!
//! Claude Code 会话历史管理系统 - Rust 实现
//! 支持多种 CLI 数据源 (Claude Code, Codex CLI)

mod adapter;
mod api;
mod backup;
mod collector;
mod config;
mod db;
mod domain;
mod embedding;
mod indexer;
mod mcp;
mod parser;
mod rag;
mod search;
mod vector;
mod watcher;

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_cron_scheduler::{Job, JobScheduler};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use api::{create_router, AppState};
use backup::BackupService;
use collector::Collector;
use config::Config;
use db::Database;
use embedding::OllamaClient;
use indexer::{IndexQueue, VectorIndexer};
use rag::RagService;
use search::HybridSearchService;
use vector::VectorStore;
use watcher::FileWatcher;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "memex_rs=info,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("🚀 Memex Rust Backend 启动中...");

    // 加载配置
    let config = Config::from_env();
    tracing::info!("📁 数据目录: {:?}", config.data_dir);
    tracing::info!("📁 Claude 项目: {:?}", config.claude_projects_path);

    // 打开数据库
    let db = Database::open(&config.db_path())?;

    // 获取统计信息
    let stats = db.get_stats()?;
    tracing::info!(
        "📊 数据库: {} 项目, {} 会话, {} 消息",
        stats.project_count,
        stats.session_count,
        stats.message_count
    );

    // 创建采集服务
    let collector = Collector::new(config.clone(), db.clone());

    // 创建备份服务
    let backup = BackupService::new(config.db_path(), config.backup_dir());

    // 初始化 Ollama 客户端 (语义搜索核心功能)
    let ollama = {
        let client = OllamaClient::new(
            &config.ollama_api,
            &config.embedding_model,
            &config.chat_model,
        );

        if client.is_available().await {
            tracing::info!("🦙 Ollama 已连接: {}", config.ollama_api);

            // Embedding 模型检查 (语义搜索必须)
            if client.is_embedding_model_available().await {
                tracing::info!("✅ Embedding 模型可用: {} (语义搜索已启用)", config.embedding_model);
            } else {
                tracing::warn!("⚠️ Embedding 模型不可用: {}，请运行: ollama pull {}",
                    config.embedding_model, config.embedding_model);
            }

            // Chat 模型检查 (AI 问答可选)
            if config.enable_ai_chat {
                if client.is_chat_model_available().await {
                    tracing::info!("✅ Chat 模型可用: {} (AI 问答已启用)", config.chat_model);
                } else {
                    tracing::warn!("⚠️ Chat 模型不可用: {}，AI 问答功能将不可用", config.chat_model);
                }
            } else {
                tracing::info!("ℹ️ AI 问答功能已禁用 (ENABLE_AI_CHAT=false)");
            }

            Some(Arc::new(client))
        } else {
            tracing::warn!("⚠️ Ollama 不可用 ({})，语义搜索功能将降级为 FTS", config.ollama_api);
            tracing::warn!("   请确保 Ollama 正在运行: ollama serve");
            None
        }
    };

    // 初始化向量存储（可选）
    let vector = if ollama.is_some() {
        match VectorStore::open(&config.lancedb_path()).await {
            Ok(store) => {
                tracing::info!("🗄️ LanceDB 已打开: {:?}", config.lancedb_path());
                Some(Arc::new(RwLock::new(store)))
            }
            Err(e) => {
                tracing::warn!("⚠️ LanceDB 打开失败: {}", e);
                None
            }
        }
    } else {
        None
    };

    // 创建索引服务（可选）
    let indexer = match (&ollama, &vector) {
        (Some(o), Some(v)) => {
            Some(VectorIndexer::new(
                db.clone(),
                o.clone(),
                v.clone(),
            ))
        }
        _ => None,
    };

    // 创建索引队列（可选，用于实时索引）
    let index_queue = indexer.clone().map(IndexQueue::new);

    // 创建混合检索服务
    let hybrid_search = HybridSearchService::new(
        db.clone(),
        ollama.clone(),
        vector.clone(),
    );

    // 创建 RAG 服务
    let rag_service = RagService::new(
        db.clone(),
        ollama.clone(),
        vector.clone(),
        config.chat_model.clone(),
    );

    // 创建应用状态
    let state = Arc::new(AppState {
        config: config.clone(),
        db,
        collector,
        backup,
        ollama,
        vector,
        indexer,
        hybrid_search,
        rag_service,
    });

    // 执行一次采集
    tracing::info!("📥 执行初始采集...");
    match state.collector.collect_all() {
        Ok(result) => {
            if result.messages_inserted > 0 {
                tracing::info!(
                    "✅ 采集完成: {} 项目, {} 会话, {} 新消息",
                    result.projects_scanned,
                    result.sessions_scanned,
                    result.messages_inserted
                );
            }
        }
        Err(e) => {
            tracing::warn!("⚠️ 初始采集失败: {}", e);
        }
    }

    // 启动定时任务调度器
    let mut scheduler = setup_scheduler(
        state.collector.clone(),
        state.backup.clone(),
        state.indexer.clone(),
    )
    .await?;

    // 启动文件监听服务（带可选的实时索引队列）
    let file_watcher = Arc::new(FileWatcher::new(
        config.clone(),
        state.collector.clone(),
        index_queue,
    ));
    file_watcher.start().await?;

    // 静态文件目录
    let public_dir = config.data_dir.join("public");
    if !public_dir.exists() {
        std::fs::create_dir_all(&public_dir)?;
        tracing::info!("📂 创建 public 目录: {:?}", public_dir);
    }

    // 创建路由
    let app = create_router(state)
        .fallback_service(ServeDir::new(&public_dir))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );
    tracing::info!("📂 静态文件目录: {:?}", public_dir);

    // 启动服务
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("🌐 服务监听: http://localhost:{}", config.port);
    tracing::info!("📝 API 端点:");
    tracing::info!("   GET  /health              - 健康检查");
    tracing::info!("   GET  /api/stats           - 统计信息");
    tracing::info!("   GET  /api/projects        - 项目列表");
    tracing::info!("   GET  /api/projects/:id    - 项目详情");
    tracing::info!("   GET  /api/projects/:id/sessions - 项目会话");
    tracing::info!("   GET  /api/sessions        - 会话列表");
    tracing::info!("   GET  /api/sessions/search - 会话搜索");
    tracing::info!("   GET  /api/sessions/:id    - 会话详情");
    tracing::info!("   GET  /api/sessions/:id/messages - 会话消息");
    tracing::info!("   GET  /api/search?q=       - FTS 搜索");
    tracing::info!("   GET  /api/search/semantic?q= - 语义搜索");
    tracing::info!("   GET  /api/search/semantic/status - 语义搜索状态");
    tracing::info!("   GET  /api/search/hybrid?q= - 混合搜索 (FTS+向量+RRF)");
    tracing::info!("   POST /api/ask             - RAG 问答");
    tracing::info!("   GET  /api/ask?q=          - RAG 问答 (GET)");
    tracing::info!("   GET  /api/ask/status      - RAG 状态");
    tracing::info!("   POST /api/collect         - 手动采集");
    tracing::info!("   POST /api/backup          - 创建备份");
    tracing::info!("   GET  /api/backup/list     - 备份列表");
    tracing::info!("   GET  /api/embedding/status - Embedding 状态");
    tracing::info!("   POST /api/embedding/trigger - 增量索引触发");
    tracing::info!("   GET  /api/embedding/failed - 失败索引列表");
    tracing::info!("   GET  /api/mcp             - MCP JSON-RPC");
    tracing::info!("   POST /api/mcp             - MCP JSON-RPC");
    tracing::info!("   GET  /api/mcp/info        - MCP 服务信息");
    tracing::info!("   POST /api/admin/fix-metadata - 修复元数据");
    tracing::info!("   POST /api/admin/merge-projects - 合并项目");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    // 停止调度器
    scheduler.shutdown().await?;

    Ok(())
}

/// 设置定时任务调度器
async fn setup_scheduler(
    collector: Collector,
    backup: BackupService,
    indexer: Option<VectorIndexer>,
) -> anyhow::Result<JobScheduler> {
    let scheduler = JobScheduler::new().await?;

    // 每日 02:00 执行备份
    let backup_clone = backup.clone();
    scheduler
        .add(Job::new_async("0 0 2 * * *", move |_uuid, _lock| {
            let backup = backup_clone.clone();
            Box::pin(async move {
                tracing::info!("⏰ 定时任务: 开始每日备份...");
                match backup.backup() {
                    Ok(result) => {
                        tracing::info!(
                            "✅ 备份完成: {} ({} bytes)",
                            result.path.display(),
                            result.size
                        );
                    }
                    Err(e) => {
                        tracing::error!("❌ 备份失败: {}", e);
                    }
                }
            })
        })?)
        .await?;
    tracing::info!("📅 定时任务已注册: 备份 (每日 02:00)");

    // 每日 02:30 执行采集
    let collector_clone = collector.clone();
    scheduler
        .add(Job::new_async("0 30 2 * * *", move |_uuid, _lock| {
            let collector = collector_clone.clone();
            Box::pin(async move {
                tracing::info!("⏰ 定时任务: 开始每日采集...");
                match collector.collect_all() {
                    Ok(result) => {
                        tracing::info!(
                            "✅ 采集完成: {} 项目, {} 会话, {} 新消息",
                            result.projects_scanned,
                            result.sessions_scanned,
                            result.messages_inserted
                        );
                    }
                    Err(e) => {
                        tracing::error!("❌ 采集失败: {}", e);
                    }
                }
            })
        })?)
        .await?;
    tracing::info!("📅 定时任务已注册: 采集 (每日 02:30)");

    // 每小时执行向量索引（如果启用 RAG）
    if let Some(indexer) = indexer {
        scheduler
            .add(Job::new_async("0 0 * * * *", move |_uuid, _lock| {
                let indexer = indexer.clone();
                Box::pin(async move {
                    tracing::info!("⏰ 定时任务: 开始增量索引...");
                    match indexer.index_batch(100).await {
                        Ok(result) => {
                            if result.indexed_messages > 0 {
                                tracing::info!(
                                    "✅ 索引完成: {} 消息, {} chunks",
                                    result.indexed_messages,
                                    result.indexed_chunks
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!("❌ 索引失败: {}", e);
                        }
                    }
                })
            })?)
            .await?;
        tracing::info!("📅 定时任务已注册: 向量索引 (每小时)");
    }

    // 启动调度器
    scheduler.start().await?;
    tracing::info!("🕐 定时任务调度器已启动");

    Ok(scheduler)
}
