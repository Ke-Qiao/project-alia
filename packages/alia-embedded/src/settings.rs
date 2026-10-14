use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use tokio::sync::RwLock;

use crate::config::{
    EmbeddedConfig, EmbeddedRuntimeSettings, runtime_settings_for_path, save_runtime_settings,
};

#[derive(Debug, Clone, Serialize)]
pub struct SettingsSnapshot {
    pub settings: EmbeddedRuntimeSettings,
    pub provisioning_mode: bool,
    pub requires_restart: bool,
    pub settings_path: String,
}

pub struct EmbeddedSettingsManager {
    path: PathBuf,
    direct_response_audio_dir: PathBuf,
    settings: RwLock<EmbeddedRuntimeSettings>,
}

impl EmbeddedSettingsManager {
    pub fn new(config: &EmbeddedConfig) -> Self {
        Self {
            path: config.config_path.clone(),
            direct_response_audio_dir: config.direct_response_defaults.audio_dir.clone(),
            settings: RwLock::new(runtime_settings_for_path(config)),
        }
    }

    /// The startup directory resolved by config loading or the runtime builder.
    /// Mutable settings describe the next boot and never redirect live uploads.
    pub fn direct_response_audio_dir(&self) -> &Path {
        &self.direct_response_audio_dir
    }

    pub async fn snapshot(&self, requires_restart: bool) -> SettingsSnapshot {
        let settings = self.settings.read().await.clone();
        SettingsSnapshot {
            provisioning_mode: !settings.runtime.provisioned,
            settings,
            requires_restart,
            settings_path: self.path.display().to_string(),
        }
    }

    pub async fn snapshot_json(&self, requires_restart: bool) -> Result<String> {
        let snapshot = self.snapshot(requires_restart).await;
        serde_json::to_string_pretty(&snapshot.settings).context("serializing settings snapshot")
    }

    pub async fn provisioning_mode(&self) -> bool {
        !self.settings.read().await.runtime.provisioned
    }

    pub async fn update_from_json(&self, settings_json: &str) -> Result<SettingsSnapshot> {
        let patch = if settings_json.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(settings_json).context("parsing embedded settings patch")?
        };
        self.update_from_value(patch).await
    }

    pub async fn update_from_value(&self, patch: serde_json::Value) -> Result<SettingsSnapshot> {
        let before = self.settings.read().await.clone();
        let mut current =
            serde_json::to_value(&before).context("serializing current embedded settings")?;
        merge_values(&mut current, patch)?;

        let merged: EmbeddedRuntimeSettings =
            serde_json::from_value(current).context("validating embedded settings patch")?;
        ensure!(
            merged.motion.approved_animations == before.motion.approved_animations,
            "approved animation catalog is local-only; edit local configuration and restart"
        );
        ensure!(
            merged.direct_response_defaults.audio_dir.as_os_str()
                == before.direct_response_defaults.audio_dir.as_os_str(),
            "direct response audio directory is local-only; edit local configuration and restart"
        );
        save_runtime_settings(&self.path, &merged)?;
        *self.settings.write().await = merged;

        Ok(self.snapshot(true).await)
    }

    pub async fn ensure_device_id(&self, device_id: &str) -> Result<bool> {
        let mut settings = self.settings.write().await;
        if !settings.identity.device_id.is_empty() {
            return Ok(false);
        }

        settings.identity.device_id = device_id.to_string();
        let persisted = settings.clone();
        drop(settings);

        save_runtime_settings(&self.path, &persisted)?;
        Ok(true)
    }
}

fn merge_values(target: &mut serde_json::Value, patch: serde_json::Value) -> Result<()> {
    match (target, patch) {
        (serde_json::Value::Object(target), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                match target.get_mut(&key) {
                    Some(existing) => merge_values(existing, value)?,
                    None => {
                        target.insert(key, value);
                    }
                }
            }
            Ok(())
        }
        (slot, value @ serde_json::Value::Null) => {
            *slot = value;
            Ok(())
        }
        (slot @ serde_json::Value::Object(_), value)
        | (slot @ serde_json::Value::Array(_), value)
        | (slot, value) => {
            if matches!(slot, serde_json::Value::Null) && matches!(value, serde_json::Value::Null) {
                bail!("settings patch cannot be null");
            }
            *slot = value;
            Ok(())
        }
    }
}
