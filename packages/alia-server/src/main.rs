#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::routing::get;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

use alia_server::auth::ensure_server_keypair;
use alia_server::config::ServerConfig;
use alia_server::event::EventBus;
use alia_server::module::ServerBuilder;
use alia_server::oobe;
use alia_server::{api, openapi, ws};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Initialize tracing
    let default_filter = "info,sqlx::query=warn,sqlx::postgres::notice=warn";
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)))
        .init();

    // 2. Check if we need OOBE setup
    if oobe::bootstrap::needs_setup() {
        run_oobe().await?;
        // After OOBE completes we restart into normal mode.
        // In production this would re-exec; for now just fall through.
    }

    // 3. Load configuration (now guaranteed to exist)
    let config_path = ServerConfig::discover_config_path();
    let config = ServerConfig::load(Path::new(&config_path))
        .context(format!("failed to load config from '{config_path}'"))?;
    let config = Arc::new(config);
    tracing::info!(path = %config_path, config_dir = %config.config_dir.display(), "configuration loaded");

    let server_keypair =
        ensure_server_keypair(&config.auth.server_keypair_path).with_context(|| {
            format!(
                "failed to ensure server keypair at {}",
                config.auth.server_keypair_path.display()
            )
        })?;
    tracing::info!(
        keypair_path = %config.auth.server_keypair_path.display(),
        public_key_len = server_keypair.public_key_bytes().len(),
        "server keypair ready"
    );

    run_server(config).await
}

/// OOBE flow: start a minimal setup server and CLI listener.
async fn run_oobe() -> anyhow::Result<()> {
    use oobe::bootstrap::{SetupResult, print_banner, spawn_cli_listener};
    use oobe::routes::{OobeState, routes};
    use tokio::sync::watch;

    const OOBE_PORT: u16 = 3000;

    // Channel to signal setup completion from either CLI or HTTP
    let (done_tx, mut done_rx) = watch::channel::<Option<SetupResult>>(None);
    let done_tx = Arc::new(done_tx);

    // Print CLI banner
    print_banner(OOBE_PORT);

    // Spawn CLI key listener (blocking thread)
    spawn_cli_listener((*done_tx).clone());

    // Build minimal Axum app with setup routes
    let oobe_state = OobeState {
        done_tx: done_tx.clone(),
    };
    let app = Router::new()
        .merge(routes())
        .layer(CorsLayer::permissive())
        .with_state(oobe_state);

    // Bind setup server
    let addr = format!("0.0.0.0:{OOBE_PORT}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context(format!("failed to bind OOBE server to {addr}"))?;
    tracing::info!("OOBE setup server listening on {addr}");

    // Run server until setup completes
    let server = axum::serve(listener, app);
    tokio::select! {
        result = server => {
            result?;
        }
        _ = done_rx.wait_for(|v| v.is_some()) => {}
    }

    let result = done_rx.borrow().clone();
    match result {
        Some(SetupResult::ConfigReady(path)) => {
            tracing::info!(path = %path.display(), "config ready — starting server");
        }
        Some(SetupResult::Cancelled) => {
            eprintln!("  Setup cancelled.");
            std::process::exit(0);
        }
        None => {
            // Server exited without setup completing
            anyhow::bail!("OOBE server exited unexpectedly");
        }
    }

    Ok(())
}

/// Normal server startup with a loaded config.
async fn run_server(config: Arc<ServerConfig>) -> anyhow::Result<()> {
    // Connect to PostgreSQL
    let mut db_opts = ConnectOptions::new(&config.database.url);
    db_opts
        .max_connections(config.database.max_connections)
        .min_connections(config.database.min_connections);
    let db = Database::connect(db_opts).await.context(format!(
        "failed to connect to database at '{}'",
        config.database.url
    ))?;
    tracing::info!("connected to database");

    run_sql_migrations(&db).await?;

    // Create EventBus
    let event_bus = EventBus::new(1024);

    // Create CancellationToken for graceful shutdown
    let cancel_token = CancellationToken::new();

    // Build server via ServerBuilder (init + start all modules and channels)
    let state = ServerBuilder::new(config.clone(), db, event_bus.clone(), cancel_token.clone())
        .build()
        .await
        .context("failed to initialize server modules")?;
    tracing::info!("all modules and channels initialized and started");

    // Build Axum router
    let app = Router::new()
        .merge(api::routes())
        .route("/api/v1/ws", get(ws::ws_handler))
        .merge(Scalar::with_url("/scalar", openapi::ApiDoc::openapi()))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    #[cfg(feature = "channel-embedded")]
    {
        let grpc_addr = format!("{}:{}", config.server.host, config.server.port + 1)
            .parse()
            .context("failed to parse embedded audio ingress gRPC address")?;
        let grpc_cancel = cancel_token.clone();
        let grpc_event_bus = event_bus.clone();
        tokio::spawn(async move {
            use alia_proto::audio::server_audio_ingress_server::ServerAudioIngressServer;
            use alia_server::grpc::audio_ingress::ServerAudioIngressService;
            use tonic::transport::Server;

            tracing::info!(addr = %grpc_addr, "embedded audio ingress gRPC listening");
            if let Err(error) = Server::builder()
                .add_service(ServerAudioIngressServer::new(
                    ServerAudioIngressService::new(grpc_event_bus, "embedded"),
                ))
                .serve_with_shutdown(grpc_addr, async move {
                    grpc_cancel.cancelled().await;
                })
                .await
            {
                tracing::error!(error = %error, "embedded audio ingress gRPC failed");
            }
        });
    }

    // Bind and serve
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context(format!("failed to bind to {addr}"))?;
    tracing::info!("alia-server listening on {addr}");

    // Graceful shutdown
    let shutdown_signal = async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to install signal handler")
                .recv()
                .await;
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {},
            _ = terminate => {},
        }
        tracing::info!("shutdown signal received, starting graceful shutdown...");
        cancel_token.cancel();
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await?;

    // Stop channels and modules in reverse order
    state.channels.shutdown().await;
    state.modules.shutdown().await;
    tracing::info!("alia-server shutdown complete");

    Ok(())
}

async fn run_sql_migrations(db: &DatabaseConnection) -> anyhow::Result<()> {
    // Keep migration tracking simple and local to this server crate.
    db.execute(Statement::from_string(
        DatabaseBackend::Postgres,
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version TEXT PRIMARY KEY,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
    ))
    .await
    .context("failed to ensure schema_migrations table")?;

    let mut files = std::fs::read_dir("packages/alia-server/migrations")
        .or_else(|_| std::fs::read_dir("migrations"))
        .context("failed to read migrations directory")?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect::<Vec<_>>();
    files.sort();

    for path in files {
        let version = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid migration filename: {}", path.display()))?
            .to_string();

        let check_sql = format!(
            "SELECT 1 FROM schema_migrations WHERE version = '{}' LIMIT 1",
            version.replace('\'', "''")
        );
        if db
            .query_one(Statement::from_string(DatabaseBackend::Postgres, check_sql))
            .await?
            .is_some()
        {
            continue;
        }

        let sql = std::fs::read_to_string(&path)
            .with_context(|| format!("failed reading migration {}", path.display()))?;

        let statements = split_sql_statements(&sql);
        if statements.is_empty() {
            tracing::warn!(migration = %version, "migration file has no executable SQL statements");
        }

        for (idx, stmt) in statements.iter().enumerate() {
            if let Err(e) = db
                .execute(Statement::from_string(
                    DatabaseBackend::Postgres,
                    stmt.clone(),
                ))
                .await
            {
                let err_text = e.to_string();
                let stmt_lower = stmt.to_ascii_lowercase();
                if stmt_lower.contains("create extension")
                    && stmt_lower.contains("vector")
                    && err_text.contains("extension \"vector\" is not available")
                {
                    anyhow::bail!(
                        "pgvector extension is required but not installed on this PostgreSQL instance. \
Install pgvector first (e.g. package 'postgresql-<version>-pgvector' or enable via your managed DB), \
then run the server again."
                    );
                }

                return Err(anyhow::Error::new(e).context(format!(
                    "failed applying migration {} (statement #{})",
                    path.display(),
                    idx + 1
                )));
            }
        }

        let record_sql = format!(
            "INSERT INTO schema_migrations (version) VALUES ('{}')",
            version.replace('\'', "''")
        );
        db.execute(Statement::from_string(
            DatabaseBackend::Postgres,
            record_sql,
        ))
        .await
        .with_context(|| format!("failed recording migration {}", version))?;

        tracing::info!(migration = %version, "applied SQL migration");
    }

    Ok(())
}

fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();

    for line in sql.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("--") {
            continue;
        }

        current.push_str(line);
        current.push('\n');

        if line.trim_end().ends_with(';') {
            let stmt = current.trim();
            if !stmt.is_empty() {
                statements.push(stmt.to_string());
            }
            current.clear();
        }
    }

    let tail = current.trim();
    if !tail.is_empty() {
        statements.push(tail.to_string());
    }

    statements
}
