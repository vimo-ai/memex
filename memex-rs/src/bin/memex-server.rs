use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::middleware;
use serde::Deserialize;
use tokio::signal;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use memex::auth::{auth_layer, AuthState};
use memex::db_reader::DbReader;
use memex::search::HybridSearchService;
use memex::server::{
    create_register_router, create_search_router, create_sync_router, IngestState,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize, Clone)]
struct UserConfig {
    name: String,
    api_key: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FileConfig {
    users: Vec<UserConfig>,
    master_key: Option<String>,
}

#[derive(Debug)]
struct ServerConfig {
    port: u16,
    db_path: String,
    users: Vec<(String, String)>, // (name, api_key)
    master_key: Option<String>,
    tls_cert: Option<String>,
    tls_key: Option<String>,
}

impl ServerConfig {
    fn load() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let vimo_root = env::var("VIMO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".vimo"));

        let db_path = env::var("MEMEX_DB_PATH").unwrap_or_else(|_| {
            vimo_root
                .join("db/server.db")
                .to_string_lossy()
                .into_owned()
        });

        let port = env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(10013);

        let tls_cert = env::var("TLS_CERT").ok();
        let tls_key = env::var("TLS_KEY").ok();

        let file_config = Self::load_config_file(&vimo_root);

        let master_key = file_config
            .master_key
            .or_else(|| env::var("MEMEX_MASTER_KEY").ok())
            .filter(|s| !s.is_empty());

        let users = if !file_config.users.is_empty() {
            file_config
                .users
                .into_iter()
                .map(|u| (u.name, u.api_key))
                .collect()
        } else if let Ok(keys) = env::var("MEMEX_API_KEYS") {
            keys.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .enumerate()
                .map(|(i, key)| (format!("user_{i}"), key))
                .collect()
        } else {
            vec![]
        };

        Self {
            port,
            db_path,
            users,
            master_key,
            tls_cert,
            tls_key,
        }
    }

    fn load_config_file(vimo_root: &Path) -> FileConfig {
        let path = vimo_root.join("memex/config.json");
        if !path.exists() {
            return FileConfig::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("config.json 解析失败: {e}");
                FileConfig::default()
            }),
            Err(_) => FileConfig::default(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "--version" | "-V" => {
                println!("memex-server {VERSION}");
                return Ok(());
            }
            "--help" | "-h" => {
                println!("memex-server {VERSION} - Memex sync server");
                println!();
                println!("Receives push data from clients, provides search API.");
                println!("No local collection, no LLM, no embedding.");
                println!();
                println!("Config file: $VIMO_HOME/memex/config.json");
                println!("  {{\"master_key\": \"mmk_xxx\", \"users\": [...]}}");
                println!();
                println!("Environment:");
                println!("  PORT              Server port (default: 10013)");
                println!("  VIMO_HOME         Data root (default: ~/.vimo)");
                println!("  MEMEX_DB_PATH     Database path (default: $VIMO_HOME/db/server.db)");
                println!("  MEMEX_MASTER_KEY  Master key for self-registration");
                println!("  MEMEX_API_KEYS    Fallback: comma-separated keys (if no config file)");
                println!("  TLS_CERT          Path to TLS certificate (PEM)");
                println!("  TLS_KEY           Path to TLS private key (PEM)");
                println!("  RUST_LOG          Log level (default: memex=info)");
                return Ok(());
            }
            _ => {
                eprintln!("Unknown argument: {}", args[1]);
                eprintln!("Use --help for usage");
                std::process::exit(1);
            }
        }
    }

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

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls crypto provider");

    let config = ServerConfig::load();

    tracing::info!("memex-server {VERSION} starting...");
    tracing::info!("DB: {}", config.db_path);

    if let Some(parent) = std::path::Path::new(&config.db_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let ingest = Arc::new(IngestState::new(&config.db_path)?);
    let ingest_for_shutdown = ingest.clone();

    let db = Arc::new(DbReader::new(Some(config.db_path.clone().into()))?);
    let search_service = Arc::new(HybridSearchService::new(db, None, None));
    tracing::info!("Search service ready (FTS mode)");

    let sync_router = create_sync_router(ingest.clone());
    let search_router = create_search_router(search_service);

    let register_router = config
        .master_key
        .as_ref()
        .map(|mk| create_register_router(ingest.clone(), mk.clone()));

    let health_route = axum::Router::new().route(
        "/api/sync/health",
        axum::routing::get(|| async {
            axum::Json(serde_json::json!({"status": "ok", "mode": "server"}))
        }),
    );

    let has_auth = !config.users.is_empty() || config.master_key.is_some();

    let app = if !has_auth {
        tracing::warn!("No users and no master_key configured, running without auth");
        let mut app = health_route.merge(sync_router).merge(search_router);
        if let Some(reg) = register_router {
            app = app.merge(reg);
        }
        app
    } else {
        let user_names: Vec<&str> = config.users.iter().map(|(n, _)| n.as_str()).collect();
        if !user_names.is_empty() {
            tracing::info!("Static users: {:?}", user_names);
        }
        if config.master_key.is_some() {
            tracing::info!("Self-registration enabled (master_key configured)");
        }

        let ingest_for_lookup = ingest.clone();
        let dynamic_lookup: memex::auth::UserLookupFn =
            Arc::new(move |key: &str| ingest_for_lookup.lookup_registered_user_by_key(key));

        let auth_state = Arc::new(AuthState::with_dynamic_lookup(
            &config.users,
            dynamic_lookup,
        ));
        let authed = sync_router
            .merge(search_router)
            .layer(middleware::from_fn(auth_layer))
            .layer(axum::Extension(auth_state));
        let mut app = health_route.merge(authed);
        if let Some(reg) = register_router {
            app = app.merge(reg);
        }
        app
    };

    let app = app.layer(
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any),
    );

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));

    match (&config.tls_cert, &config.tls_key) {
        (Some(cert_path), Some(key_path)) => {
            tracing::info!("TLS enabled: cert={cert_path}");
            tracing::info!("Listening on https://0.0.0.0:{}", config.port);
            log_endpoints(config.master_key.is_some());

            let tls_config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(cert_path, key_path).await?;

            axum_server::bind_rustls(addr, tls_config)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            tracing::info!("Listening on http://0.0.0.0:{}", config.port);
            log_endpoints(config.master_key.is_some());

            let listener = tokio::net::TcpListener::bind(addr).await?;
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await?;
        }
    }

    tracing::info!("Running WAL checkpoint...");
    if let Err(e) = ingest_for_shutdown.checkpoint() {
        tracing::warn!("Checkpoint failed: {e}");
    }

    tracing::info!("Shutdown complete");
    Ok(())
}

fn log_endpoints(has_register: bool) {
    tracing::info!("Endpoints:");
    tracing::info!("  POST /api/sync/push                 - Receive client push");
    tracing::info!("  GET  /api/sync/health               - Health check");
    tracing::info!("  GET  /api/sync/peers                 - List peers");
    tracing::info!("  GET  /api/sync/peers/:name/sessions  - Peer sessions");
    tracing::info!("  GET  /api/sync/peers/:name/timeline  - Peer timeline");
    tracing::info!("  GET  /api/search                    - Search (FTS/vector/hybrid)");
    if has_register {
        tracing::info!("  POST /api/auth/register             - Self-registration");
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("Received Ctrl+C"),
        _ = terminate => tracing::info!("Received SIGTERM"),
    }
}
