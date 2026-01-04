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
use memex::config::Config;
use memex::embedding::OllamaClient;
use memex::indexer::{IndexQueue, VectorIndexer};
use memex::rag::RagService;
use memex::search::HybridSearchService;
use memex::vector::VectorStore;
use memex::watcher::FileWatcher;
use memex::shared_adapter::SharedDbAdapter;

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
                println!("memex {} - Claude Code 会话历史管理系统", VERSION);
                println!();
                println!("用法: memex [命令] [选项]");
                println!();
                println!("命令:");
                println!("  archive              归档管理");
                println!("    --status           查看归档状态");
                println!("    --check            检查并执行所有需要的归档（推荐）");
                println!("    --now              立即执行每日归档");
                println!("    --daily            执行每日归档");
                println!("    --weekly           执行周合并");
                println!("    --monthly          执行月合并");
                println!("    --yearly           执行年合并");
                println!();
                println!("选项:");
                println!("  -V, --version    显示版本号");
                println!("  -h, --help       显示帮助信息");
                println!();
                println!("环境变量:");
                println!("  PORT                 服务端口 (默认: 10013)");
                println!("  MEMEX_DATA_DIR       数据目录 (默认: ~/.vimo/memex)");
                println!("  OLLAMA_API           Ollama API 地址 (默认: http://localhost:11434)");
                println!("  EMBEDDING_MODEL      Embedding 模型 (默认: bge-m3)");
                println!("  CHAT_MODEL           Chat 模型 (默认: qwen3:8b)");
                println!("  ENABLE_AI_CHAT       启用 AI 问答 (默认: false)");
                return Ok(());
            }
            "archive" => {
                return handle_archive_command(&args[2..]).await;
            }
            _ => {
                eprintln!("未知参数: {}", args[1]);
                eprintln!("使用 --help 查看帮助");
                std::process::exit(1);
            }
        }
    }

    // 初始化日志（使用东八区时间 UTC+8）
    let timer = tracing_subscriber::fmt::time::OffsetTime::new(
        time::UtcOffset::from_hms(8, 0, 0).unwrap(),
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]+08:00"),
    );
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "memex=info,tower_http=debug".into()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_timer(timer)
        )
        .init();

    tracing::info!("🚀 Memex Rust Backend 启动中...");

    // 初始化共享数据库（必须成功）
    let shared_db = {
        use claude_session_db::coordination::{Role, WriterHealth};

        let adapter = SharedDbAdapter::new(None)?;
        tracing::info!("[SharedDB] 连接共享数据库成功");
        let adapter = std::sync::Arc::new(adapter);

        // 注册 Writer
        match adapter.register().await {
            Ok(role) => {
                tracing::info!("[SharedDB] 注册为 {:?}", role);

                // 如果是 Reader，检查现有 Writer 是否超时
                if role == Role::Reader {
                    match adapter.check_writer_health().await {
                        Ok(health) => {
                            tracing::info!("[SharedDB] 当前 Writer 状态: {:?}", health);
                            if matches!(health, WriterHealth::Timeout | WriterHealth::Released) {
                                tracing::info!("[SharedDB] Writer 已超时/释放，尝试接管...");
                                match adapter.try_takeover().await {
                                    Ok(true) => {
                                        tracing::info!("[SharedDB] 接管成功，现在是 Writer");
                                    }
                                    Ok(false) => {
                                        tracing::info!("[SharedDB] 接管失败，其他组件已抢先");
                                    }
                                    Err(e) => {
                                        tracing::warn!("[SharedDB] 接管出错: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("[SharedDB] 检查 Writer 健康状态失败: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[SharedDB] 注册失败: {}", e);
            }
        }
        adapter
    };

    // 加载配置
    let config = Config::from_env();
    tracing::info!("📁 数据目录: {:?}", config.data_dir);
    tracing::info!("📁 Claude 项目: {:?}", config.claude_projects_path);

    // 获取统计信息
    let stats = shared_db.get_stats().await?;
    tracing::info!(
        "📊 数据库: {} 项目, {} 会话, {} 消息",
        stats.project_count,
        stats.session_count,
        stats.message_count
    );

    // 创建采集服务（统一使用 SharedDbAdapter）
    let collector = Collector::new(config.clone(), shared_db.clone());

    // 创建备份服务
    let backup = BackupService::new(config.db_path(), config.backup_dir());

    // 启动时执行归档补偿检查
    {
        let archive_dir = config.data_dir.join("archive");
        match ArchiveService::new(config.claude_projects_path.clone(), archive_dir) {
            Ok(mut service) => {
                if let Ok(Some(_lock)) = service.try_lock() {
                    tracing::info!("📦 启动时归档检查...");
                    match service.check_and_archive_all() {
                        Ok(result) => {
                            if result.has_work() {
                                tracing::info!(
                                    "✅ 补偿归档完成: {} 日归档, {} 周合并, {} 月合并, {} 年合并",
                                    result.daily_archived,
                                    result.weekly_merged,
                                    result.monthly_merged,
                                    result.yearly_merged
                                );
                            } else {
                                tracing::info!("📦 归档状态正常，无需补偿");
                            }
                            if result.has_errors() {
                                for err in &result.errors {
                                    tracing::warn!("归档警告: {}", err);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("⚠️ 启动归档检查失败: {}，将在定时任务中重试", e);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("⚠️ 创建归档服务失败: {}，将在定时任务中重试", e);
            }
        }
    }

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
                shared_db.clone(),
                o.clone(),
                v.clone(),
            ))
        }
        _ => None,
    };

    // 同步 LanceDB 索引状态到 SQLite（首次迁移用）
    if let Some(ref indexer) = indexer {
        let unindexed = shared_db.count_unindexed_messages().await.unwrap_or(0);
        if unindexed > 1000 {
            // 只有大量未索引时才执行同步（说明可能是迁移后首次启动）
            tracing::info!("🔄 检测到 {} 条未同步消息，执行 LanceDB 状态同步...", unindexed);
            match indexer.sync_indexed_status().await {
                Ok(n) => {
                    if n > 0 {
                        tracing::info!("✅ 同步完成: {} 条消息已标记为已索引", n);
                    }
                }
                Err(e) => {
                    tracing::warn!("⚠️ 同步失败: {}", e);
                }
            }
        }
    }

    // 创建索引队列（可选，用于实时索引）
    let index_queue = indexer.clone().map(IndexQueue::new);

    // 创建混合检索服务
    let hybrid_search = HybridSearchService::new(
        shared_db.clone(),
        ollama.clone(),
        vector.clone(),
    );

    // 创建 RAG 服务
    let rag_service = RagService::new(
        shared_db.clone(),
        ollama.clone(),
        vector.clone(),
        config.chat_model.clone(),
    );

    // 创建应用状态
    let state = Arc::new(AppState {
        config: config.clone(),
        db: shared_db,
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
    let config = memex::config::Config::from_env();

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

    // 每日 03:00 执行归档检查（统一入口，包含日/周/月/年的补偿逻辑）
    let archive_source = config.claude_projects_path.clone();
    let archive_dir = config.data_dir.join("archive");
    scheduler
        .add(Job::new_async("0 0 3 * * *", move |_uuid, _lock| {
            let source = archive_source.clone();
            let dir = archive_dir.clone();
            Box::pin(async move {
                tracing::info!("⏰ 定时任务: 开始归档检查...");
                match ArchiveService::new(source, dir) {
                    Ok(mut service) => {
                        match service.try_lock() {
                            Ok(Some(_lock)) => {
                                match service.check_and_archive_all() {
                                    Ok(result) => {
                                        if result.has_work() {
                                            tracing::info!(
                                                "✅ 归档完成: {} 日归档, {} 周合并, {} 月合并, {} 年合并",
                                                result.daily_archived,
                                                result.weekly_merged,
                                                result.monthly_merged,
                                                result.yearly_merged
                                            );
                                        }
                                        if result.has_errors() {
                                            for err in &result.errors {
                                                tracing::error!("归档错误: {}", err);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("❌ 归档检查失败: {}", e);
                                    }
                                }
                            }
                            Ok(None) => {
                                tracing::warn!("另一个归档任务正在运行，跳过");
                            }
                            Err(e) => {
                                tracing::error!("❌ 获取归档锁失败: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("❌ 创建归档服务失败: {}", e);
                    }
                }
            })
        })?)
        .await?;
    tracing::info!("📅 定时任务已注册: 归档检查 (每日 03:00)");

    // 启动调度器
    scheduler.start().await?;
    tracing::info!("🕐 定时任务调度器已启动");

    Ok(scheduler)
}

/// 处理归档命令
async fn handle_archive_command(args: &[String]) -> anyhow::Result<()> {
    // 初始化日志
    let timer = tracing_subscriber::fmt::time::OffsetTime::new(
        time::UtcOffset::from_hms(8, 0, 0).unwrap(),
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]+08:00"),
    );
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "memex=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_timer(timer))
        .init();

    let config = memex::config::Config::from_env();
    let archive_dir = config.data_dir.join("archive");

    let mut archive_service = ArchiveService::new(
        config.claude_projects_path.clone(),
        archive_dir,
    )?;

    // 尝试获取锁
    let _lock = match archive_service.try_lock()? {
        Some(lock) => lock,
        None => {
            eprintln!("另一个归档任务正在运行");
            std::process::exit(1);
        }
    };

    let action = args.first().map(|s| s.as_str()).unwrap_or("--status");

    match action {
        "--status" => {
            let state = archive_service.status();
            println!("归档状态:");
            println!("  最后成功: {:?}", state.last_success);
            println!("  待处理任务: {}", state.has_pending_task());
            println!("  失败记录: {} 条", state.failures.len());
            for failure in &state.failures {
                println!("    - {:?}: {} (重试 {} 次)",
                    failure.task_type, failure.error, failure.retry_count);
            }
        }
        "--check" => {
            println!("检查并执行所有需要的归档...");
            match archive_service.check_and_archive_all() {
                Ok(result) => {
                    if result.has_work() {
                        println!("归档完成:");
                        println!("  日归档: {} 个", result.daily_archived);
                        println!("  周合并: {} 个", result.weekly_merged);
                        println!("  月合并: {} 个", result.monthly_merged);
                        println!("  年合并: {} 个", result.yearly_merged);
                    } else {
                        println!("归档状态正常，无需操作");
                    }
                    if result.has_errors() {
                        println!("警告:");
                        for err in &result.errors {
                            println!("  - {}", err);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("归档检查失败: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "--now" | "--daily" => {
            println!("执行每日归档...");
            match archive_service.archive_daily() {
                Ok(Some(result)) => {
                    println!("归档完成:");
                    println!("  路径: {}", result.archive_path.display());
                    println!("  文件数: {}", result.files_count);
                    println!("  原始大小: {} bytes", result.original_size);
                    println!("  压缩后: {} bytes", result.compressed_size);
                    println!("  压缩率: {:.1}:1", result.compression_ratio);
                }
                Ok(None) => {
                    println!("没有需要归档的文件");
                }
                Err(e) => {
                    eprintln!("归档失败: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "--weekly" => {
            println!("执行周合并...");
            match archive_service.merge_weekly() {
                Ok(Some(result)) => {
                    println!("周合并完成:");
                    println!("  路径: {}", result.archive_path.display());
                    println!("  文件数: {}", result.files_count);
                    println!("  压缩率: {:.1}:1", result.compression_ratio);
                }
                Ok(None) => {
                    println!("没有需要合并的日包");
                }
                Err(e) => {
                    eprintln!("周合并失败: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "--monthly" => {
            println!("执行月合并...");
            match archive_service.merge_monthly() {
                Ok(Some(result)) => {
                    println!("月合并完成:");
                    println!("  路径: {}", result.archive_path.display());
                    println!("  文件数: {}", result.files_count);
                    println!("  压缩率: {:.1}:1", result.compression_ratio);
                }
                Ok(None) => {
                    println!("没有需要合并的周包");
                }
                Err(e) => {
                    eprintln!("月合并失败: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "--yearly" => {
            println!("执行年合并...");
            match archive_service.merge_yearly() {
                Ok(Some(result)) => {
                    println!("年合并完成:");
                    println!("  路径: {}", result.archive_path.display());
                    println!("  文件数: {}", result.files_count);
                    println!("  压缩率: {:.1}:1", result.compression_ratio);
                }
                Ok(None) => {
                    println!("没有需要合并的月包");
                }
                Err(e) => {
                    eprintln!("年合并失败: {}", e);
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("未知归档命令: {}", action);
            eprintln!("使用 --help 查看帮助");
            std::process::exit(1);
        }
    }

    Ok(())
}
