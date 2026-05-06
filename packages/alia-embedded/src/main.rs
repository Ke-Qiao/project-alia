#![allow(dead_code)]

mod binding;
mod config;
mod grpc;
mod module;
mod plugin;
mod process;

use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::info;

use crate::module::EmbeddedBuilder;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("alia-embedded starting");

    // Load configuration
    let config_path = PathBuf::from("config/embedded.toml");
    let config = config::load_config(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;

    info!(device_id = %config.identity.device_id, device_name = %config.identity.device_name, "identity loaded");

    // Build and run the embedded node using the module builder
    let node = EmbeddedBuilder::from_config(config)
        .await?
        .build()
        .await?;

    node.run().await?;

    Ok(())
}
