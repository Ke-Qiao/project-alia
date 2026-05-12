mod types;

pub use types::*;

use anyhow::{Context, Result};
use std::path::Path;

pub fn load_config(path: &Path) -> Result<EmbeddedConfig> {
    let mut config = EmbeddedConfig::default();
    config.config_path = path.to_path_buf();

    if path.exists() {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading config: {}", path.display()))?;
        let runtime: EmbeddedRuntimeSettings = toml::from_str(&content)
            .with_context(|| format!("parsing config: {}", path.display()))?;
        config.apply_runtime_settings(runtime);
    }

    resolve_paths(&mut config, path);
    Ok(config)
}

pub fn save_runtime_settings(path: &Path, settings: &EmbeddedRuntimeSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory: {}", parent.display()))?;
    }

    let content =
        toml::to_string_pretty(settings).context("serializing embedded runtime settings")?;
    std::fs::write(path, content).with_context(|| format!("writing config: {}", path.display()))?;
    Ok(())
}

pub fn save_runtime_config(config: &EmbeddedConfig) -> Result<()> {
    save_runtime_settings(&config.config_path, &config.runtime_settings())
}

fn resolve_paths(config: &mut EmbeddedConfig, path: &Path) {
    if config.identity.keypair_path.is_relative() {
        if let Some(parent) = path.parent() {
            config.identity.keypair_path = parent.join(&config.identity.keypair_path);
        }
    }
}
