//! Memex Rust Backend
//!
//! Claude Code 会话历史管理系统 - Rust 实现
//! 支持多种 CLI 数据源 (Claude Code, Codex CLI)

use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_cron_scheduler::{Job, JobScheduler};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use memex::api::{create_router, AppState};
use memex::archive::ArchiveService;
use memex::backup::BackupService;
use memex::collector::Collector;
use memex::compact::{run_compact_worker, CompactDB, CompactQueue, CompactWorker};
use memex::config::Config;
use memex::indexer::{IndexQueue, VectorIndexer};
use memex::llm::{ChatProvider, EmbeddingProvider, OllamaProvider};
use memex::rag::RagService;
use memex::search::HybridSearchService;
use memex::shared_adapter::SharedDbAdapter;
use memex::vector::VectorStore;
use memex::watcher::FileWatcher;

/// 版本号（从 Cargo.toml 读取）
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 处理命令行参数
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "--version" | "-V" => {
                println!("memex {}", VERSION);
                return Ok(());
            }
            "--help" | "-h" => {
                println!("memex {} - Claude Code session history manager", VERSION);
                println!();
                println!("Usage: memex [command] [options]");
                println!();
                println!("Commands:");
                println!("  archive              Archive management");
                println!("    --status           Show archive status");
                println!("    --check            Check and run all needed archives (recommended)");
                println!("    --now              Run daily archive now");
                println!("    --daily            Run daily archive");
                println!("    --weekly           Run weekly merge");
                println!("    --monthly          Run monthly merge");
                println!("    --yearly           Run yearly merge");
                println!();
                println!("Options:");
                println!("  -V, --version    Show version");
                println!("  -h, --help       Show help");
                println!();
                println!("Environment:");
                println!("  PORT                 Server port (default: 10013)");
                println!("  MEMEX_DATA_DIR       Data directory (default: ~/.vimo/db)");
                println!("  MEMEX_WEB_DIR        Web static files (default: ~/.vimo/memex/web)");
                println!("  OLLAMA_API           Ollama API URL (default: http://localhost:11434)");
                println!("  EMBEDDING_MODEL      Embedding model (default: bge-m3)");
                println!("  CHAT_MODEL           Chat model (default: qwen3:0.6b)");
                println!("  ENABLE_AI_CHAT       Enable AI chat (default: false)");
                println!();
                println!("Config file: ~/.vimo/memex/config.json");
                println!("  Example: {{\"compact\": {{\"enabled\": true}}}}");
                return Ok(());
            }
            "archive" => {
                return handle_archive_command(&args[2..]).await;
            }
            _ => {
                eprintln!("Unknown argument: {}", args[1]);
                eprintln!("Use --help for usage");
                std::process::exit(1);
            }
        }
    }

    // 初始化日志（使用东八区时间 UTC+8）
    let timer = tracing_subscriber::fmt::time::OffsetTime::new(
        time::UtcOffset::from_hms(8, 0, 0).unwrap(),
        time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]+08:00"
        ),
    );
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "memex=info,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_timer(timer))
        .init();

    tracing::info!("🚀 Memex Rust Backend starting...");

    // Initialize shared database (must succeed)
    let shared_db = {
        use claude_session_db::coordination::{Role, WriterHealth};

        let adapter = SharedDbAdapter::new(None)?;
        tracing::info!("[SharedDB] Connected to shared database");
        let adapter = std::sync::Arc::new(adapter);

        // Register Writer
        match adapter.register().await {
            Ok(role) => {
                tracing::info!("[SharedDB] Registered as {:?}", role);

                // If Reader, check if existing Writer has timed out
                if role == Role::Reader {
                    match adapter.check_writer_health().await {
                        Ok(health) => {
                            tracing::info!("[SharedDB] Current Writer status: {:?}", health);
                            if matches!(health, WriterHealth::Timeout | WriterHealth::Released) {
                                tracing::info!(
                                    "[SharedDB] Writer timed out/released, trying takeover..."
                                );
                                match adapter.try_takeover().await {
                                    Ok(true) => {
                                        tracing::info!(
                                            "[SharedDB] Takeover successful, now Writer"
                                        );
                                    }
                                    Ok(false) => {
                                        tracing::info!(
                                            "[SharedDB] Takeover failed, another component took it"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!("[SharedDB] Takeover error: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("[SharedDB] Failed to check Writer health: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[SharedDB] Registration failed: {}", e);
            }
        }
        adapter
    };

    // Load config
    let config = Config::load();
    tracing::info!("📁 Data directory: {:?}", config.data_dir);
    tracing::info!("📁 Web directory: {:?}", config.web_dir);
    tracing::info!("📁 Claude projects: {:?}", config.claude_projects_path);

    // Get statistics
    let stats = shared_db.get_stats().await?;
    tracing::info!(
        "📊 Database: {} projects, {} sessions, {} messages",
        stats.project_count,
        stats.session_count,
        stats.message_count
    );

    // Create collector service (using SharedDbAdapter)
    let collector = Collector::new(config.clone(), shared_db.clone());

    // Create backup service
    let backup = BackupService::new(config.db_path(), config.backup_dir());

    // Run archive compensation check on startup (async, non-blocking)
    {
        let archive_dir = config.data_dir.join("archive");
        let claude_projects = config.claude_projects_path.clone();
        tokio::spawn(async move {
            // Delay 5 seconds to let HTTP start first
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

            match ArchiveService::new(claude_projects, archive_dir) {
                Ok(mut service) => {
                    if let Ok(Some(_lock)) = service.try_lock() {
                        tracing::info!("📦 Background archive check starting...");
                        match service.check_and_archive_all() {
                            Ok(result) => {
                                if result.has_work() {
                                    tracing::info!(
                                        "✅ Compensation archive done: {} daily, {} weekly, {} monthly, {} yearly",
                                        result.daily_archived,
                                        result.weekly_merged,
                                        result.monthly_merged,
                                        result.yearly_merged
                                    );
                                } else {
                                    tracing::info!("📦 Archive status OK, no compensation needed");
                                }
                                if result.has_errors() {
                                    for err in &result.errors {
                                        tracing::warn!("Archive warning: {}", err);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "⚠️ Archive check failed: {}, will retry in scheduled task",
                                    e
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "⚠️ Failed to create archive service: {}, will retry in scheduled task",
                        e
                    );
                }
            }
        });
    }

    // Initialize LLM providers (embedding + chat)
    // chat_for_compact: 只要模型可用就启用（Compact 是后台服务）
    // chat: 需要 ENABLE_AI_CHAT=true（RAG Q&A 是用户主动调用）
    #[allow(clippy::type_complexity)]
    let (embedding, chat, chat_for_compact): (
        Option<Arc<dyn EmbeddingProvider>>,
        Option<Arc<dyn ChatProvider>>,
        Option<Arc<dyn ChatProvider>>,
    ) = {
        let provider = OllamaProvider::new(
            &config.ollama_api,
            &config.embedding_model,
            &config.chat_model,
        );

        if provider.is_embedding_model_available().await || provider.is_chat_model_available().await
        {
            tracing::info!("🦙 Ollama connected: {}", config.ollama_api);

            // Embedding provider (required for semantic search)
            let embedding: Option<Arc<dyn EmbeddingProvider>> =
                if provider.is_embedding_model_available().await {
                    tracing::info!(
                        "✅ Embedding model available: {} (semantic search enabled)",
                        config.embedding_model
                    );
                    Some(Arc::new(provider.clone()))
                } else {
                    tracing::warn!(
                        "⚠️ Embedding model unavailable: {}, run: ollama pull {}",
                        config.embedding_model,
                        config.embedding_model
                    );
                    None
                };

            // Chat provider for Compact (always enabled if model available)
            let chat_for_compact: Option<Arc<dyn ChatProvider>> =
                if provider.is_chat_model_available().await {
                    tracing::info!(
                        "✅ Chat model available: {} (Compact enabled)",
                        config.chat_model
                    );
                    Some(Arc::new(provider.clone()))
                } else {
                    tracing::warn!(
                        "⚠️ Chat model unavailable: {}, Compact will not work",
                        config.chat_model
                    );
                    None
                };

            // Chat provider for RAG Q&A (requires explicit enable)
            let chat: Option<Arc<dyn ChatProvider>> =
                if config.enable_ai_chat && chat_for_compact.is_some() {
                    tracing::info!("✅ AI Q&A enabled");
                    chat_for_compact.clone()
                } else if !config.enable_ai_chat {
                    tracing::info!("ℹ️ AI Q&A disabled (ENABLE_AI_CHAT=false)");
                    None
                } else {
                    None
                };

            (embedding, chat, chat_for_compact)
        } else {
            tracing::warn!(
                "⚠️ Ollama unavailable ({}), semantic search will fallback to FTS",
                config.ollama_api
            );
            tracing::warn!("   Make sure Ollama is running: ollama serve");
            (None, None, None)
        }
    };

    // Initialize vector store (optional)
    let vector = if embedding.is_some() {
        match VectorStore::open(&config.lancedb_path()).await {
            Ok(store) => {
                tracing::info!("🗄️ LanceDB opened: {:?}", config.lancedb_path());
                Some(Arc::new(RwLock::new(store)))
            }
            Err(e) => {
                tracing::warn!("⚠️ Failed to open LanceDB: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Create indexer service (optional)
    let indexer = match (&embedding, &vector) {
        (Some(e), Some(v)) => Some(VectorIndexer::new(shared_db.clone(), e.clone(), v.clone())),
        _ => None,
    };

    // Sync LanceDB index status to SQLite (for first migration)
    if let Some(ref indexer) = indexer {
        let unindexed = shared_db.count_unindexed_messages().await.unwrap_or(0);
        if unindexed > 1000 {
            // Only sync when many unindexed (likely first run after migration)
            tracing::info!(
                "🔄 Found {} unsynced messages, syncing LanceDB status...",
                unindexed
            );
            match indexer.sync_indexed_status().await {
                Ok(n) => {
                    if n > 0 {
                        tracing::info!("✅ Sync complete: {} messages marked as indexed", n);
                    }
                }
                Err(e) => {
                    tracing::warn!("⚠️ Sync failed: {}", e);
                }
            }
        }
    }

    // Create index queue (optional, for real-time indexing)
    let index_queue = indexer.clone().map(IndexQueue::new);

    // Initialize Compact service (optional, requires enabled + chat provider)
    // 通过配置文件 ~/.vimo/memex/config.json 控制
    // 注意: compact_db 会同时传给 CompactWorker 和 AppState（用于 MCP 渐进式披露）
    let compact_config = config.compact.clone();
    let (compact_queue, compact_db): (Option<CompactQueue>, Option<Arc<CompactDB>>) =
        if compact_config.enabled {
            if let Some(ref chat_provider) = chat_for_compact {
                // 初始化 CompactDB（复用共享数据库文件）
                // 传递 fts_tokenizer 配置（默认 trigram 支持中文）
                match CompactDB::connect(config.db_path(), Some(&compact_config.fts_tokenizer)) {
                    Ok(compact_db) => {
                        let compact_db = Arc::new(compact_db);

                        // 启动时清理超时的锁（进程崩溃恢复）
                        if let Err(e) = compact_db.unlock_stale_locks().await {
                            tracing::warn!("⚠️ Failed to clean stale locks: {}", e);
                        }

                        // 创建队列和 worker
                        let (queue, receiver) = CompactQueue::new();
                        let worker = CompactWorker::new(
                            shared_db.clone(),
                            chat_provider.clone(),
                            compact_db.clone(), // Clone for worker
                            compact_config.clone(),
                            queue.tracker().clone(),
                        );

                        // 启动 worker（后台任务）
                        tokio::spawn(run_compact_worker(worker, receiver));

                        tracing::info!(
                            "🗜️ Compact service enabled (L1={}, L2={}, L3={})",
                            compact_config.l1_observations,
                            compact_config.l2_talk_summary,
                            compact_config.l3_session_summary
                        );

                        (Some(queue), Some(compact_db))
                    }
                    Err(e) => {
                        tracing::warn!("⚠️ Failed to initialize CompactDB: {}", e);
                        (None, None)
                    }
                }
            } else {
                tracing::warn!("⚠️ Compact enabled but chat model unavailable, worker disabled");
                // 连接 CompactDB（用于 MCP 搜索已有的摘要）
                let compact_db = CompactDB::connect(config.db_path(), Some(&compact_config.fts_tokenizer))
                    .ok()
                    .map(Arc::new);
                (None, compact_db)
            }
        } else {
            // Compact 未启用，只连接 CompactDB（用于 MCP 搜索已有的摘要）
            let compact_db = CompactDB::connect(config.db_path(), Some(&compact_config.fts_tokenizer))
                .ok()
                .map(Arc::new);
            if compact_db.is_some() {
                tracing::info!("ℹ️ CompactDB connected for MCP search (read-only, COMPACT_ENABLED=false)");
            }
            (None, compact_db)
        };

    // Create hybrid search service
    let hybrid_search =
        HybridSearchService::new(shared_db.clone(), embedding.clone(), vector.clone());

    // Create RAG service
    let rag_service =
        RagService::new(shared_db.clone(), chat.clone(), embedding.clone(), vector.clone());

    // Create app state
    // 注意：compact_queue 需要 clone，因为后面还要传给 file_watcher
    let state = Arc::new(AppState {
        config: config.clone(),
        db: shared_db.clone(),
        collector,
        backup,
        embedding,
        chat,
        vector,
        indexer,
        hybrid_search,
        rag_service,
        compact_db,
        compact_queue: compact_queue.clone(),
    });

    // Run initial collection in background (non-blocking HTTP startup)
    {
        let collector = state.collector.clone();
        tokio::spawn(async move {
            // Delay 2 seconds to let HTTP start first
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            tracing::info!("📥 Background initial collection starting...");
            match collector.collect_all() {
                Ok(result) => {
                    if result.messages_inserted > 0 {
                        tracing::info!(
                            "✅ Collection done: {} projects, {} sessions, {} new messages",
                            result.projects_scanned,
                            result.sessions_scanned,
                            result.messages_inserted
                        );
                    } else {
                        tracing::info!("📥 Initial collection done, no new messages");
                    }
                }
                Err(e) => {
                    tracing::warn!("⚠️ Initial collection failed: {}", e);
                }
            }
        });
    }

    // Start scheduled task scheduler
    let mut scheduler = setup_scheduler(
        state.collector.clone(),
        state.backup.clone(),
        state.indexer.clone(),
    )
    .await?;

    // Run compact once on startup (in case scheduled task was missed)
    if let Some(indexer) = &state.indexer {
        let indexer = indexer.clone();
        tokio::spawn(async move {
            // Delay 10 seconds to let service fully start
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
            tracing::info!("🔧 Startup compact: checking vector store...");
            match indexer.compact().await {
                Ok(()) => tracing::info!("✅ Startup compact done"),
                Err(e) => tracing::warn!("⚠️ Startup compact failed: {}", e),
            }
        });
    }

    // Start file watcher service (with optional real-time index queue and compact queue)
    let mut file_watcher = FileWatcher::new(config.clone(), state.collector.clone(), index_queue);
    if let Some(queue) = compact_queue {
        // 传入 compact_queue 和 shared_db（用于 Talk 边界检测）
        file_watcher = file_watcher.with_compact(queue, shared_db.clone());
    }
    let file_watcher = Arc::new(file_watcher);
    file_watcher.start().await?;

    // Web 静态文件目录 (独立于 data_dir)
    let web_dir = &config.web_dir;
    if !web_dir.exists() {
        std::fs::create_dir_all(web_dir)?;
        tracing::info!("📂 Created web directory: {:?}", web_dir);
    }

    // Create router
    let app = create_router(state)
        .fallback_service(ServeDir::new(web_dir))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );
    tracing::info!("📂 Web files: {:?}", web_dir);

    // Start server
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("🌐 Server listening: http://localhost:{}", config.port);
    tracing::info!("📝 API endpoints:");
    tracing::info!("   GET  /health              - Health check");
    tracing::info!("   GET  /api/stats           - Statistics");
    tracing::info!("   GET  /api/projects        - Project list");
    tracing::info!("   GET  /api/projects/:id    - Project details");
    tracing::info!("   GET  /api/projects/:id/sessions - Project sessions");
    tracing::info!("   GET  /api/sessions        - Session list");
    tracing::info!("   GET  /api/sessions/search - Session search");
    tracing::info!("   GET  /api/sessions/:id    - Session details");
    tracing::info!("   GET  /api/sessions/:id/messages - Session messages");
    tracing::info!("   GET  /api/search?q=       - FTS search");
    tracing::info!("   GET  /api/search/semantic?q= - Semantic search");
    tracing::info!("   GET  /api/search/semantic/status - Semantic search status");
    tracing::info!("   GET  /api/search/hybrid?q= - Hybrid search (FTS+vector+RRF)");
    tracing::info!("   POST /api/ask             - RAG Q&A");
    tracing::info!("   GET  /api/ask?q=          - RAG Q&A (GET)");
    tracing::info!("   GET  /api/ask/status      - RAG status");
    tracing::info!("   POST /api/collect         - Manual collection");
    tracing::info!("   POST /api/backup          - Create backup");
    tracing::info!("   GET  /api/backup/list     - Backup list");
    tracing::info!("   GET  /api/embedding/status - Embedding status");
    tracing::info!("   GET  /api/embedding/stats  - Index stats (pending/indexed/failed)");
    tracing::info!("   POST /api/embedding/trigger - Incremental index (100)");
    tracing::info!("   POST /api/embedding/trigger-all - Full index (max 3000)");
    tracing::info!("   GET  /api/embedding/failed - Failed index list");
    tracing::info!("   POST /api/embedding/reset-failed - Reset failed status");
    tracing::info!("   GET  /api/mcp             - MCP JSON-RPC");
    tracing::info!("   POST /api/mcp             - MCP JSON-RPC");
    tracing::info!("   GET  /api/mcp/info        - MCP service info");
    tracing::info!("   POST /api/admin/fix-metadata - Fix metadata");
    tracing::info!("   POST /api/admin/merge-projects - Merge projects");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    // Stop scheduler
    scheduler.shutdown().await?;

    Ok(())
}

/// Setup scheduled task scheduler
async fn setup_scheduler(
    collector: Collector,
    backup: BackupService,
    indexer: Option<VectorIndexer>,
) -> anyhow::Result<JobScheduler> {
    let scheduler = JobScheduler::new().await?;
    let config = memex::config::Config::load();

    // Daily backup at 02:00
    let backup_clone = backup.clone();
    scheduler
        .add(Job::new_async("0 0 2 * * *", move |_uuid, _lock| {
            let backup = backup_clone.clone();
            Box::pin(async move {
                tracing::info!("⏰ Scheduled task: starting daily backup...");
                match backup.backup() {
                    Ok(result) => {
                        tracing::info!(
                            "✅ Backup done: {} ({} bytes)",
                            result.path.display(),
                            result.size
                        );
                    }
                    Err(e) => {
                        tracing::error!("❌ Backup failed: {}", e);
                    }
                }
            })
        })?)
        .await?;
    tracing::info!("📅 Scheduled task registered: backup (daily 02:00)");

    // Daily collection at 02:30
    let collector_clone = collector.clone();
    scheduler
        .add(Job::new_async("0 30 2 * * *", move |_uuid, _lock| {
            let collector = collector_clone.clone();
            Box::pin(async move {
                tracing::info!("⏰ Scheduled task: starting daily collection...");
                match collector.collect_all() {
                    Ok(result) => {
                        tracing::info!(
                            "✅ Collection done: {} projects, {} sessions, {} new messages",
                            result.projects_scanned,
                            result.sessions_scanned,
                            result.messages_inserted
                        );
                    }
                    Err(e) => {
                        tracing::error!("❌ Collection failed: {}", e);
                    }
                }
            })
        })?)
        .await?;
    tracing::info!("📅 Scheduled task registered: collection (daily 02:30)");

    // Hourly vector indexing (if RAG enabled)
    // Use index_pending to clear increments, max 3000 (prevent Ollama overload)
    let indexer_for_compact = indexer.clone(); // For compact task later
    if let Some(indexer) = indexer {
        scheduler
            .add(Job::new_async("0 0 * * * *", move |_uuid, _lock| {
                let indexer = indexer.clone();
                Box::pin(async move {
                    tracing::info!("⏰ Scheduled task: starting incremental indexing...");
                    // Use index_pending(3000) instead of index_batch(100)
                    // - Clear all increments from this hour
                    // - Max 3000, excess will continue next hour
                    match indexer.index_pending(3000).await {
                        Ok(result) => {
                            if result.indexed_messages > 0 || result.failed > 0 {
                                tracing::info!(
                                    "✅ Indexing done: {} messages, {} chunks, {} failed",
                                    result.indexed_messages,
                                    result.indexed_chunks,
                                    result.failed
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!("❌ Indexing failed: {}", e);
                        }
                    }
                })
            })?)
            .await?;
        tracing::info!("📅 Scheduled task registered: vector indexing (hourly, max 3000)");
    }

    // Daily archive check at 03:00 (unified entry, includes daily/weekly/monthly/yearly compensation)
    let archive_source = config.claude_projects_path.clone();
    let archive_dir = config.data_dir.join("archive");
    scheduler
        .add(Job::new_async("0 0 3 * * *", move |_uuid, _lock| {
            let source = archive_source.clone();
            let dir = archive_dir.clone();
            Box::pin(async move {
                tracing::info!("⏰ Scheduled task: starting archive check...");
                match ArchiveService::new(source, dir) {
                    Ok(mut service) => {
                        match service.try_lock() {
                            Ok(Some(_lock)) => {
                                match service.check_and_archive_all() {
                                    Ok(result) => {
                                        if result.has_work() {
                                            tracing::info!(
                                                "✅ Archive done: {} daily, {} weekly, {} monthly, {} yearly",
                                                result.daily_archived,
                                                result.weekly_merged,
                                                result.monthly_merged,
                                                result.yearly_merged
                                            );
                                        }
                                        if result.has_errors() {
                                            for err in &result.errors {
                                                tracing::error!("Archive error: {}", err);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("❌ Archive check failed: {}", e);
                                    }
                                }
                            }
                            Ok(None) => {
                                tracing::warn!("Another archive task is running, skipping");
                            }
                            Err(e) => {
                                tracing::error!("❌ Failed to acquire archive lock: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("❌ Failed to create archive service: {}", e);
                    }
                }
            })
        })?)
        .await?;
    tracing::info!("📅 Scheduled task registered: archive check (daily 03:00)");

    // Daily vector store compaction at 03:30 (merge files, cleanup old versions)
    if let Some(indexer) = indexer_for_compact {
        scheduler
            .add(Job::new_async("0 30 3 * * *", move |_uuid, _lock| {
                let indexer = indexer.clone();
                Box::pin(async move {
                    tracing::info!("⏰ Scheduled task: starting vector store compaction...");
                    match indexer.compact().await {
                        Ok(()) => {
                            tracing::info!("✅ Vector store compaction done");
                        }
                        Err(e) => {
                            tracing::error!("❌ Vector store compaction failed: {}", e);
                        }
                    }
                })
            })?)
            .await?;
        tracing::info!("📅 Scheduled task registered: vector compaction (daily 03:30)");
    }

    // Start scheduler
    scheduler.start().await?;
    tracing::info!("🕐 Scheduled task scheduler started");

    Ok(scheduler)
}

/// Handle archive command
async fn handle_archive_command(args: &[String]) -> anyhow::Result<()> {
    // Initialize logging
    let timer = tracing_subscriber::fmt::time::OffsetTime::new(
        time::UtcOffset::from_hms(8, 0, 0).unwrap(),
        time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]+08:00"
        ),
    );
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "memex=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_timer(timer))
        .init();

    let config = memex::config::Config::load();
    let archive_dir = config.data_dir.join("archive");

    let mut archive_service =
        ArchiveService::new(config.claude_projects_path.clone(), archive_dir)?;

    // Try to acquire lock
    let _lock = match archive_service.try_lock()? {
        Some(lock) => lock,
        None => {
            eprintln!("Another archive task is running");
            std::process::exit(1);
        }
    };

    let action = args.first().map(|s| s.as_str()).unwrap_or("--status");

    match action {
        "--status" => {
            let state = archive_service.status();
            println!("Archive status:");
            println!("  Last success: {:?}", state.last_success);
            println!("  Pending tasks: {}", state.has_pending_task());
            println!("  Failures: {}", state.failures.len());
            for failure in &state.failures {
                println!(
                    "    - {:?}: {} (retried {} times)",
                    failure.task_type, failure.error, failure.retry_count
                );
            }
        }
        "--check" => {
            println!("Checking and running all needed archives...");
            match archive_service.check_and_archive_all() {
                Ok(result) => {
                    if result.has_work() {
                        println!("Archive done:");
                        println!("  Daily: {}", result.daily_archived);
                        println!("  Weekly: {}", result.weekly_merged);
                        println!("  Monthly: {}", result.monthly_merged);
                        println!("  Yearly: {}", result.yearly_merged);
                    } else {
                        println!("Archive status OK, no action needed");
                    }
                    if result.has_errors() {
                        println!("Warnings:");
                        for err in &result.errors {
                            println!("  - {}", err);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Archive check failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "--now" | "--daily" => {
            println!("Running daily archive...");
            match archive_service.archive_daily() {
                Ok(Some(result)) => {
                    println!("Archive done:");
                    println!("  Path: {}", result.archive_path.display());
                    println!("  Files: {}", result.files_count);
                    println!("  Original size: {} bytes", result.original_size);
                    println!("  Compressed: {} bytes", result.compressed_size);
                    println!("  Ratio: {:.1}:1", result.compression_ratio);
                }
                Ok(None) => {
                    println!("No files to archive");
                }
                Err(e) => {
                    eprintln!("Archive failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "--weekly" => {
            println!("Running weekly merge...");
            match archive_service.merge_weekly() {
                Ok(Some(result)) => {
                    println!("Weekly merge done:");
                    println!("  Path: {}", result.archive_path.display());
                    println!("  Files: {}", result.files_count);
                    println!("  Ratio: {:.1}:1", result.compression_ratio);
                }
                Ok(None) => {
                    println!("No daily archives to merge");
                }
                Err(e) => {
                    eprintln!("Weekly merge failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "--monthly" => {
            println!("Running monthly merge...");
            match archive_service.merge_monthly() {
                Ok(Some(result)) => {
                    println!("Monthly merge done:");
                    println!("  Path: {}", result.archive_path.display());
                    println!("  Files: {}", result.files_count);
                    println!("  Ratio: {:.1}:1", result.compression_ratio);
                }
                Ok(None) => {
                    println!("No weekly archives to merge");
                }
                Err(e) => {
                    eprintln!("Monthly merge failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "--yearly" => {
            println!("Running yearly merge...");
            match archive_service.merge_yearly() {
                Ok(Some(result)) => {
                    println!("Yearly merge done:");
                    println!("  Path: {}", result.archive_path.display());
                    println!("  Files: {}", result.files_count);
                    println!("  Ratio: {:.1}:1", result.compression_ratio);
                }
                Ok(None) => {
                    println!("No monthly archives to merge");
                }
                Err(e) => {
                    eprintln!("Yearly merge failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("Unknown archive command: {}", action);
            eprintln!("Use --help for usage");
            std::process::exit(1);
        }
    }

    Ok(())
}
