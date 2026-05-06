mod types;

pub use types::*;

use anyhow::{Context, Result};
use std::path::Path;

pub fn load_config(path: &Path) -> Result<EmbeddedConfig> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading config: {}", path.display()))?;
    let config: EmbeddedConfig =
        toml::from_str(&content).with_context(|| format!("parsing config: {}", path.display()))?;
    Ok(config)
}
