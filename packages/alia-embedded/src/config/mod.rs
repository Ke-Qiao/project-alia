mod types;

pub use types::*;

use anyhow::{Context, Result};
use std::path::Path;

pub fn load_config(path: &Path) -> Result<EmbeddedConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading config: {}", path.display()))?;
    let mut config: EmbeddedConfig =
        toml::from_str(&content).with_context(|| format!("parsing config: {}", path.display()))?;
    if config.identity.keypair_path.is_relative() {
        if let Some(parent) = path.parent() {
            config.identity.keypair_path = parent.join(&config.identity.keypair_path);
        }
    }
    Ok(config)
}
