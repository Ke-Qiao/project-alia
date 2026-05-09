#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::routing::get;
use sea_orm::{ConnectOptions, Database};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

use alia_server::config::ServerConfig;
use alia_server::event::EventBus;
use alia_server::module::ServerBuilder;
use alia_server::{api, openapi, ws};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    // 2. Load configuration
    let config_path =
        std::env::var("ALIA_CONFIG_PATH").unwrap_or_else(|_| "config/server.toml".into());
    let config = ServerConfig::load(Path::new(&config_path))
        .context(format!("failed to load config from '{config_path}'"))?;
    let config = Arc::new(config);
    tracing::info!(path = %config_path, "configuration loaded");

    // 3. Connect to PostgreSQL
    let mut db_opts = ConnectOptions::new(&config.database.url);
    db_opts
        .max_connections(config.database.max_connections)
        .min_connections(config.database.min_connections);
    let db = Database::connect(db_opts).await.context(format!(
        "failed to connect to database at '{}'",
        config.database.url
    ))?;
    tracing::info!("connected to database");

    // 4. Create EventBus
    let event_bus = EventBus::new(1024);

    // 5. Create CancellationToken for graceful shutdown
    let cancel_token = CancellationToken::new();

    // 6. Build server via ServerBuilder (init + start all modules and channels)
    let state = ServerBuilder::new(config.clone(), db, event_bus, cancel_token.clone())
        .build()
        .await
        .context("failed to initialize server modules")?;
    tracing::info!("all modules and channels initialized and started");

    // 8. Build Axum router
    let app = Router::new()
        .merge(api::routes())
        .route("/api/v1/ws", get(ws::ws_handler))
        .merge(Scalar::with_url("/scalar", openapi::ApiDoc::openapi()))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    // 9. Bind and serve
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context(format!("failed to bind to {addr}"))?;
    tracing::info!("alia-server listening on {addr}");

    // 10. Graceful shutdown
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

    // 11. Stop channels and modules in reverse order
    state.channels.shutdown().await;
    state.modules.shutdown().await;
    tracing::info!("alia-server shutdown complete");

    Ok(())
}
