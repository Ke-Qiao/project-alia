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
    let Some(parent) = path.parent() else {
        return;
    };

    resolve_relative(parent, &mut config.identity.keypair_path);
    resolve_relative(parent, &mut config.wakeword.model_path);
    resolve_relative(parent, &mut config.wakeword.encoder_path);
    resolve_relative(parent, &mut config.wakeword.decoder_path);
    resolve_relative(parent, &mut config.wakeword.joiner_path);
    resolve_relative(parent, &mut config.wakeword.tokens_path);
    resolve_relative(parent, &mut config.wakeword.keywords_file);
    resolve_relative(parent, &mut config.edge_stt.model_path);
    resolve_relative(parent, &mut config.edge_stt.tokens_path);

    for response in &mut config.direct_response {
        resolve_relative(parent, &mut response.audio);
    }
}

fn resolve_relative(base: &Path, path: &mut std::path::PathBuf) {
    if path.is_relative() {
        *path = base.join(&path);
    }
}
