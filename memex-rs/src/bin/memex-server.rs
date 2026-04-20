use std::env;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::middleware;
use tokio::signal;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use memex::auth::{auth_layer, AuthState};
use memex::server::{create_sync_router, IngestState};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
struct ServerConfig {
    port: u16,
    db_path: String,
    api_keys: Vec<String>,
}

impl ServerConfig {
    fn load() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let vimo_root = env::var("VIMO_HOME")
            .map(std::path::PathBuf::from)
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

        let api_keys: Vec<String> = env::var("MEMEX_API_KEYS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Self {
            port,
            db_path,
            api_keys,
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
                println!("Environment:");
                println!("  PORT              Server port (default: 10013)");
                println!("  MEMEX_DB_PATH     Database path (default: ~/.vimo/db/server.db)");
                println!("  MEMEX_API_KEYS    Comma-separated API keys for auth");
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

    let config = ServerConfig::load();

    tracing::info!("memex-server {VERSION} starting...");
    tracing::info!("DB: {}", config.db_path);

    if let Some(parent) = std::path::Path::new(&config.db_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let ingest = Arc::new(IngestState::new(&config.db_path)?);
    let ingest_for_shutdown = ingest.clone();

    let sync_router = create_sync_router(ingest);

    let app = if config.api_keys.is_empty() {
        tracing::warn!("MEMEX_API_KEYS not set, running without auth");
        sync_router
    } else {
        tracing::info!("Auth enabled ({} keys)", config.api_keys.len());
        let auth_state = Arc::new(AuthState::new(&config.api_keys));
        sync_router
            .layer(middleware::from_fn(auth_layer))
            .layer(axum::Extension(auth_state))
    };

    let app = app.layer(
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any),
    );

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("Listening on http://0.0.0.0:{}", config.port);
    tracing::info!("Endpoints:");
    tracing::info!("  POST /api/sync/push    - Receive client push");
    tracing::info!("  GET  /api/sync/health  - Health check");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("Running WAL checkpoint...");
    if let Err(e) = ingest_for_shutdown.checkpoint() {
        tracing::warn!("Checkpoint failed: {e}");
    }

    tracing::info!("Shutdown complete");
    Ok(())
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
