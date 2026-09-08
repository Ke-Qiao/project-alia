use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::filter::ParseError;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use alia_server::bootstrap;
use alia_server::config::ServerConfig;
use alia_server::oobe;

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("creating server runtime")?;
    let result = runtime.block_on(async_main());
    // The library has its own overall cleanup deadline and retains unfinished
    // ownership. Bound process teardown separately; an incomplete result remains
    // an error, not a claim that a stuck native worker stopped safely.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    result
}

async fn async_main() -> anyhow::Result<()> {
    let needs_setup = oobe::bootstrap::needs_setup();
    let config_path = ServerConfig::discover_config_path();
    let preloaded_config = if needs_setup {
        None
    } else {
        Some(
            ServerConfig::load(Path::new(&config_path))
                .with_context(|| format!("failed to load config from '{config_path}'"))?,
        )
    };

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(default_env_filter(
            preloaded_config
                .as_ref()
                .map(|config| config.server.log_level.as_str()),
        )?)
        .init();

    let shutdown = CancellationToken::new();
    let server_shutdown = shutdown.clone();
    let server = async move {
        let config = match preloaded_config {
            Some(config) => config,
            None => {
                let Some(path) = bootstrap::run_setup(server_shutdown.clone()).await? else {
                    return Ok(());
                };
                ServerConfig::load(&path)
                    .with_context(|| format!("failed to load config from '{}'", path.display()))?
            }
        };
        tracing::info!(config_dir = %config.config_dir.display(), "configuration loaded");
        bootstrap::run(Arc::new(config), server_shutdown).await
    };
    tokio::pin!(server);

    tokio::select! {
        result = &mut server => result,
        signal = shutdown_signal() => {
            tracing::info!("shutdown requested");
            shutdown.cancel();
            let result = server.await;
            signal.and(result)
        }
    }
}

fn default_env_filter(config_log_level: Option<&str>) -> Result<EnvFilter, ParseError> {
    let base_filter =
        std::env::var("RUST_LOG").unwrap_or_else(|_| config_log_level.unwrap_or("info").to_owned());
    Ok(EnvFilter::new(base_filter)
        .add_directive("sqlx::query=warn".parse()?)
        .add_directive("sqlx::postgres::notice=warn".parse()?))
}

async fn shutdown_signal() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("failed to install SIGTERM handler")?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.context("Ctrl+C signal listener failed"),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .context("Ctrl+C signal listener failed")
}
