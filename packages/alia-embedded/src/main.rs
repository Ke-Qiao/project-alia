#![allow(dead_code)]

// TODO(sensor): Implement sensor reporting system:
//   - Add a `sensor` module that collects readings from hardware sensors (temperature, humidity, motion, etc.)
//   - Report sensor data to alia-server via gRPC `SensorData` channel events
//   - Support plugin-defined sensors (plugins can register custom sensor types)
//   - Periodic reporting interval configurable per sensor
//   - Batch multiple readings in a single gRPC call for efficiency

mod binding;
mod config;
mod grpc;
mod identity;
mod module;
mod plugin;
mod process;
mod settings;
mod storage;

use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::module::EmbeddedBuilder;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(default_env_filter())
        .init();

    info!("alia-embedded starting");

    // Load configuration
    let config_path = std::env::var("ALIA_EMBEDDED_CONFIG_PATH")
        .map(PathBuf::from)
        .map(config::normalize_config_path)
        .unwrap_or_else(|_| config::default_config_path());
    let mut config = config::load_config(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;

    let keypair =
        identity::ensure_device_keypair(&config.identity.keypair_path).with_context(|| {
            format!(
                "ensuring device keypair at {}",
                config.identity.keypair_path.display()
            )
        })?;

    let generated_device_id = if config.identity.device_id.is_empty() {
        config.identity.device_id = derived_device_id(&keypair.public_key_bytes());
        true
    } else {
        false
    };

    if generated_device_id || !config_path.exists() {
        config::save_runtime_config(&config).with_context(|| {
            format!(
                "persisting embedded runtime settings to {}",
                config.config_path.display()
            )
        })?;
    }

    info!(device_id = %config.identity.device_id, device_name = %config.identity.device_name, "identity loaded");
    info!(
        keypair_path = %config.identity.keypair_path.display(),
        public_key_len = keypair.public_key_bytes().len(),
        "device keypair ready"
    );

    // Build and run the embedded node using the module builder
    let node = EmbeddedBuilder::from_config(config).await?.build().await?;

    node.run().await?;

    Ok(())
}

fn default_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("sqlx::query=warn".parse().expect("valid sqlx query filter"))
}

fn derived_device_id(public_key: &[u8; 32]) -> String {
    let suffix = public_key[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("alia-embedded-{suffix}")
}
