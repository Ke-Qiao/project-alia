mod types;

pub use types::*;

use anyhow::{Context, Result};
use std::path::{Component, Path, PathBuf};

pub fn default_config_path() -> PathBuf {
    let default = PathBuf::from("config/embedded.toml");
    normalize_config_path(default)
}

pub fn normalize_config_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }

    let Ok(cwd) = std::env::current_dir() else {
        return path;
    };
    normalize_config_path_for_cwd(path, &cwd)
}

pub fn load_config(path: &Path) -> Result<EmbeddedConfig> {
    let path = std::path::absolute(path).context("resolving config path")?;
    let path = path.as_path();
    let mut config = EmbeddedConfig::default();
    config.config_path = path.to_path_buf();

    if path.exists() {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading config: {}", path.display()))?;
        let runtime: EmbeddedRuntimeSettings = toml::from_str(&content)
            .with_context(|| format!("parsing config: {}", path.display()))?;
        config.apply_runtime_settings(runtime);
    }

    config.device_runtime.validate()?;
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
    save_runtime_settings(&config.config_path, &runtime_settings_for_path(config))
}

pub fn runtime_settings_for_path(config: &EmbeddedConfig) -> EmbeddedRuntimeSettings {
    let mut settings = config.runtime_settings();
    relativize_runtime_paths(&config.config_path, &mut settings);
    settings
}

/// Normalize a library-supplied configuration using the same rules as file loading.
pub fn resolve_config_paths(config: &mut EmbeddedConfig) -> Result<()> {
    config.device_runtime.validate()?;
    let path = if config.config_path.as_os_str().is_empty() {
        std::env::current_dir()?.join("embedded.toml")
    } else {
        std::path::absolute(&config.config_path)?
    };
    resolve_paths(config, &path);
    config.config_path = path;
    Ok(())
}

fn resolve_paths(config: &mut EmbeddedConfig, path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };

    resolve_relative(parent, &mut config.device_runtime.state_path);
    resolve_relative(parent, &mut config.device_runtime.plugins_dir);
    resolve_relative(parent, &mut config.identity.keypair_path);
    resolve_relative(parent, &mut config.wakeword.model_path);
    resolve_relative(parent, &mut config.wakeword.encoder_path);
    resolve_relative(parent, &mut config.wakeword.decoder_path);
    resolve_relative(parent, &mut config.wakeword.joiner_path);
    resolve_relative(parent, &mut config.wakeword.tokens_path);
    resolve_relative(parent, &mut config.wakeword.keywords_file);
    resolve_relative(parent, &mut config.edge_stt.model_path);
    resolve_relative(parent, &mut config.edge_stt.tokens_path);
    resolve_relative(parent, &mut config.vad.model_path);
    resolve_relative(parent, &mut config.hmmd_mmwave.serial_path);
    resolve_relative(parent, &mut config.motion.neck_config_file);
    resolve_relative(parent, &mut config.motion.animation_dir);
    resolve_relative(parent, &mut config.motion.serial_path);
    resolve_relative(parent, &mut config.motion.i2c_device);
    resolve_relative(parent, &mut config.vision.yunet_model_path);
    resolve_relative(parent, &mut config.direct_response_defaults.audio_dir);
    resolve_relative(parent, &mut config.audio_debug.dir);

    for response in &mut config.direct_response {
        resolve_relative(parent, &mut response.audio);
        for variant in &mut response.audio_variants {
            resolve_relative(parent, &mut variant.audio);
        }
    }
}

fn resolve_relative(base: &Path, path: &mut std::path::PathBuf) {
    if path.is_relative() {
        *path = base.join(&path);
    }
}

pub fn normalize_config_path_for_cwd(path: PathBuf, cwd: &Path) -> PathBuf {
    if cwd.file_name().is_some_and(|name| name == "config")
        && path.starts_with("config")
        && path.components().next() == Some(Component::Normal("config".as_ref()))
    {
        return path
            .strip_prefix("config")
            .map(Path::to_path_buf)
            .unwrap_or(path);
    }

    path
}

pub fn relativize_runtime_paths(config_path: &Path, settings: &mut EmbeddedRuntimeSettings) {
    let Some(base) = config_path.parent() else {
        return;
    };
    relativize_path(base, &mut settings.device_runtime.state_path);
    relativize_path(base, &mut settings.device_runtime.plugins_dir);
    relativize_path(base, &mut settings.identity.keypair_path);
    relativize_path(base, &mut settings.wakeword.model_path);
    relativize_path(base, &mut settings.wakeword.encoder_path);
    relativize_path(base, &mut settings.wakeword.decoder_path);
    relativize_path(base, &mut settings.wakeword.joiner_path);
    relativize_path(base, &mut settings.wakeword.tokens_path);
    relativize_path(base, &mut settings.wakeword.keywords_file);
    relativize_path(base, &mut settings.edge_stt.model_path);
    relativize_path(base, &mut settings.edge_stt.tokens_path);
    relativize_path(base, &mut settings.vad.model_path);
    relativize_path(base, &mut settings.hmmd_mmwave.serial_path);
    relativize_path(base, &mut settings.motion.neck_config_file);
    relativize_path(base, &mut settings.motion.animation_dir);
    relativize_path(base, &mut settings.motion.serial_path);
    relativize_path(base, &mut settings.motion.i2c_device);
    relativize_path(base, &mut settings.vision.yunet_model_path);
    relativize_path(base, &mut settings.direct_response_defaults.audio_dir);
    relativize_path(base, &mut settings.audio_debug.dir);
}

fn relativize_path(base: &Path, path: &mut PathBuf) {
    if base.as_os_str().is_empty() {
        return;
    }

    if let Ok(stripped) = path.strip_prefix(base) {
        *path = stripped.to_path_buf();
        return;
    }

    if path.is_absolute()
        && let Ok(base) = std::fs::canonicalize(base)
        && let Ok(stripped) = path.strip_prefix(base)
    {
        *path = stripped.to_path_buf();
    }
}
