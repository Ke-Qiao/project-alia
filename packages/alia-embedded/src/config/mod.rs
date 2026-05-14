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
    save_runtime_settings(&config.config_path, &runtime_settings_for_path(config))
}

pub fn runtime_settings_for_path(config: &EmbeddedConfig) -> EmbeddedRuntimeSettings {
    let mut settings = config.runtime_settings();
    relativize_runtime_paths(&config.config_path, &mut settings);
    settings
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
    resolve_runtime_relative(&mut config.direct_response_defaults.audio_dir);

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

fn resolve_runtime_relative(path: &mut PathBuf) {
    if path.is_relative() {
        *path = normalize_relative_components(path);
    }
}

fn normalize_relative_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn normalize_config_path_for_cwd(path: PathBuf, cwd: &Path) -> PathBuf {
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

fn relativize_runtime_paths(config_path: &Path, settings: &mut EmbeddedRuntimeSettings) {
    let Some(base) = config_path.parent() else {
        return;
    };
    relativize_path(base, &mut settings.identity.keypair_path);
    normalize_runtime_path_for_settings(&mut settings.direct_response_defaults.audio_dir);
}

fn normalize_runtime_path_for_settings(path: &mut PathBuf) {
    if path.is_absolute() {
        return;
    }
    *path = normalize_relative_components(path);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_duplicate_config_prefix_when_cwd_is_config_dir() {
        let normalized = normalize_config_path_for_cwd(
            PathBuf::from("config/embedded.toml"),
            Path::new("config"),
        );

        assert_eq!(normalized, PathBuf::from("embedded.toml"));
    }

    #[test]
    fn keeps_config_prefix_when_cwd_is_project_root() {
        let normalized = normalize_config_path_for_cwd(
            PathBuf::from("config/embedded.toml"),
            Path::new("project-alia"),
        );

        assert_eq!(normalized, PathBuf::from("config/embedded.toml"));
    }

    #[test]
    fn saves_keypair_path_relative_to_config_file() {
        let mut settings = EmbeddedRuntimeSettings::default();
        settings.identity.keypair_path = PathBuf::from("config/device.key");

        relativize_runtime_paths(Path::new("config/embedded.toml"), &mut settings);

        assert_eq!(settings.identity.keypair_path, PathBuf::from("device.key"));
    }

    #[test]
    fn keeps_runtime_asset_dirs_relative_to_runtime_root() {
        let mut settings = EmbeddedRuntimeSettings::default();
        settings.direct_response_defaults.audio_dir = PathBuf::from("run/direct_response");

        relativize_runtime_paths(Path::new("config/embedded.toml"), &mut settings);

        assert_eq!(
            settings.direct_response_defaults.audio_dir,
            PathBuf::from("run/direct_response")
        );
    }
}
