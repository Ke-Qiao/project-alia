//! Core settings registry implementation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

// ── Setting value types ────────────────────────────────────────────────

/// A single setting value with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingEntry {
    pub value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<SettingSchema>,
}

/// Schema metadata for a setting, used by the frontend to render forms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingSchema {
    /// Human-readable label
    pub label: String,
    /// Longer description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Value type hint: "string", "number", "bool", "select", "secret"
    #[serde(rename = "type")]
    pub value_type: String,
    /// Default value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// Allowed values for "select" type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// Whether this setting requires a restart to take effect
    #[serde(default)]
    pub requires_restart: bool,
    /// Whether the value should be masked in UI (passwords, keys)
    #[serde(default)]
    pub secret: bool,
}

// ── Settings Registry ──────────────────────────────────────────────────

/// Thread-safe settings registry with namespace isolation.
///
/// Keys use dot-separated namespaces:
/// - `core.server.port`           — server settings
/// - `core.ai.openai_api_key`     — AI settings
/// - `plugin.zigbee.bridge_host`  — plugin settings
#[derive(Clone)]
pub struct SettingsRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

struct RegistryInner {
    /// All settings, keyed by dotted path
    entries: BTreeMap<String, SettingEntry>,
    /// Path to persist settings (e.g. config/settings.toml)
    persist_path: Option<PathBuf>,
}

impl SettingsRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RegistryInner {
                entries: BTreeMap::new(),
                persist_path: None,
            })),
        }
    }

    /// Set the file path for persisting runtime settings.
    pub async fn set_persist_path(&self, path: PathBuf) {
        self.inner.write().await.persist_path = Some(path);
    }

    // ── Read operations ────────────────────────────────────────────

    /// Get a single setting value by key.
    pub async fn get(&self, key: &str) -> Option<serde_json::Value> {
        let inner = self.inner.read().await;
        inner.entries.get(key).map(|e| e.value.clone())
    }

    /// Get a single setting entry (value + schema).
    pub async fn get_entry(&self, key: &str) -> Option<SettingEntry> {
        let inner = self.inner.read().await;
        inner.entries.get(key).cloned()
    }

    /// List all settings under a namespace prefix.
    /// e.g. `list("core.ai")` returns all AI settings.
    pub async fn list(&self, prefix: &str) -> BTreeMap<String, SettingEntry> {
        let inner = self.inner.read().await;
        let prefix_dot = if prefix.is_empty() {
            String::new()
        } else if prefix.ends_with('.') {
            prefix.to_string()
        } else {
            format!("{prefix}.")
        };

        inner
            .entries
            .iter()
            .filter(|(k, _)| prefix.is_empty() || k.starts_with(&prefix_dot) || *k == prefix)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Get all settings as a flat map (for serialization).
    pub async fn all(&self) -> BTreeMap<String, SettingEntry> {
        self.inner.read().await.entries.clone()
    }

    /// List registered namespaces (unique first-level prefixes).
    pub async fn namespaces(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        let mut ns: Vec<String> = inner
            .entries
            .keys()
            .filter_map(|k| k.split('.').next().map(String::from))
            .collect();
        ns.dedup();
        ns
    }

    // ── Write operations ───────────────────────────────────────────

    /// Set a value (upsert). Preserves existing schema if not provided.
    pub async fn set(&self, key: &str, value: serde_json::Value) {
        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.entries.get_mut(key) {
            entry.value = value;
        } else {
            inner.entries.insert(
                key.to_string(),
                SettingEntry {
                    value,
                    schema: None,
                },
            );
        }
    }

    /// Set a value with schema metadata (used during registration).
    pub async fn register(&self, key: &str, value: serde_json::Value, schema: SettingSchema) {
        let mut inner = self.inner.write().await;
        inner.entries.insert(
            key.to_string(),
            SettingEntry {
                value,
                schema: Some(schema),
            },
        );
    }

    /// Batch update: apply a map of key→value changes.
    /// Returns list of keys that were actually changed.
    pub async fn batch_update(&self, updates: &BTreeMap<String, serde_json::Value>) -> Vec<String> {
        let mut inner = self.inner.write().await;
        let mut changed = Vec::new();

        for (key, value) in updates {
            if let Some(entry) = inner.entries.get_mut(key) {
                if entry.value != *value {
                    entry.value = value.clone();
                    changed.push(key.clone());
                }
            } else {
                inner.entries.insert(
                    key.clone(),
                    SettingEntry {
                        value: value.clone(),
                        schema: None,
                    },
                );
                changed.push(key.clone());
            }
        }

        changed
    }

    /// Remove a setting by key.
    pub async fn remove(&self, key: &str) -> bool {
        self.inner.write().await.entries.remove(key).is_some()
    }

    /// Remove all settings under a namespace prefix.
    pub async fn remove_namespace(&self, prefix: &str) {
        let mut inner = self.inner.write().await;
        let prefix_dot = format!("{prefix}.");
        inner
            .entries
            .retain(|k, _| !k.starts_with(&prefix_dot) && k != prefix);
    }

    // ── Plugin support ─────────────────────────────────────────────

    /// Register a plugin's settings with schema.
    /// Keys are automatically namespaced as `plugin.<plugin_name>.<key>`.
    pub async fn register_plugin(
        &self,
        plugin_name: &str,
        settings: Vec<(String, serde_json::Value, SettingSchema)>,
    ) {
        let mut inner = self.inner.write().await;
        for (key, value, schema) in settings {
            let full_key = format!("plugin.{plugin_name}.{key}");
            inner.entries.insert(
                full_key,
                SettingEntry {
                    value,
                    schema: Some(schema),
                },
            );
        }
    }

    /// Unregister all settings for a plugin.
    pub async fn unregister_plugin(&self, plugin_name: &str) {
        self.remove_namespace(&format!("plugin.{plugin_name}"))
            .await;
    }

    /// Get all settings for a specific plugin.
    pub async fn plugin_settings(&self, plugin_name: &str) -> BTreeMap<String, SettingEntry> {
        self.list(&format!("plugin.{plugin_name}")).await
    }

    // ── Persistence ────────────────────────────────────────────────

    /// Persist current settings to disk as TOML.
    pub async fn save(&self) -> anyhow::Result<()> {
        let inner = self.inner.read().await;
        let path = inner
            .persist_path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no persist path configured"))?;

        // Convert to a serializable map (value only, no schema)
        let values: BTreeMap<String, serde_json::Value> = inner
            .entries
            .iter()
            .map(|(k, e)| (k.clone(), e.value.clone()))
            .collect();

        let toml_str = toml::to_string_pretty(&values)?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml_str)?;

        Ok(())
    }

    /// Load persisted settings from disk (values only, schemas are preserved).
    pub async fn load_persisted(&self, path: &Path) -> anyhow::Result<()> {
        if !path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(path)?;
        let values: BTreeMap<String, serde_json::Value> = toml::from_str(&content)?;

        let mut inner = self.inner.write().await;
        for (key, value) in values {
            if let Some(entry) = inner.entries.get_mut(&key) {
                entry.value = value;
            } else {
                inner.entries.insert(
                    key,
                    SettingEntry {
                        value,
                        schema: None,
                    },
                );
            }
        }

        inner.persist_path = Some(path.to_path_buf());
        Ok(())
    }

    // ── Config reflection ──────────────────────────────────────────

    /// Populate the registry from a loaded ServerConfig.
    /// This seeds `core.*` keys so the frontend can read/modify them.
    pub async fn seed_from_config(&self, config: &crate::config::ServerConfig) {
        use serde_json::json;

        let mut inner = self.inner.write().await;

        let entries = vec![
            // Server
            (
                "core.server.host",
                json!(config.server.host),
                schema("Host", "string", false),
            ),
            (
                "core.server.port",
                json!(config.server.port),
                schema("Port", "number", true),
            ),
            (
                "core.server.log_level",
                json!(config.server.log_level),
                schema_select(
                    "Log Level",
                    &["trace", "debug", "info", "warn", "error"],
                    true,
                ),
            ),
            // Database
            (
                "core.database.url",
                json!(config.database.url),
                schema_secret("Database URL"),
            ),
            (
                "core.database.max_connections",
                json!(config.database.max_connections),
                schema("Max Connections", "number", true),
            ),
            // AI
            (
                "core.ai.openai_api_key",
                json!(config.ai.openai_api_key),
                schema_secret("OpenAI API Key"),
            ),
            (
                "core.ai.openai_model",
                json!(config.ai.openai_model),
                schema("OpenAI Model", "string", false),
            ),
            (
                "core.ai.openai_base_url",
                json!(config.ai.openai_base_url),
                schema("OpenAI Base URL", "string", false),
            ),
            (
                "core.ai.ollama_endpoint",
                json!(config.ai.ollama_endpoint),
                schema("Ollama Endpoint", "string", false),
            ),
            (
                "core.ai.ollama_model",
                json!(config.ai.ollama_model),
                schema("Ollama Model", "string", false),
            ),
            (
                "core.ai.streaming_enabled",
                json!(config.ai.streaming_enabled),
                schema("Streaming", "bool", false),
            ),
            (
                "core.ai.max_conversation_context",
                json!(config.ai.max_conversation_context),
                schema("Max Context Turns", "number", false),
            ),
            // Modules
            (
                "core.modules.llm",
                json!(config.modules.llm),
                schema_select("LLM Backend", &["openai", "ollama", "noop"], true),
            ),
            (
                "core.modules.memory",
                json!(config.modules.memory),
                schema_select("Memory Backend", &["graphrag", "simple", "noop"], true),
            ),
            (
                "core.modules.cache",
                json!(config.modules.cache),
                schema_select("Cache Backend", &["two_layer", "noop"], true),
            ),
            (
                "core.modules.router",
                json!(config.modules.router),
                schema_select("Router", &["multi_layer", "passthrough"], true),
            ),
        ];

        for (key, value, s) in entries {
            inner.entries.insert(
                key.to_string(),
                SettingEntry {
                    value,
                    schema: Some(s),
                },
            );
        }
    }
}

// ── Schema helpers ─────────────────────────────────────────────────────

fn schema(label: &str, value_type: &str, requires_restart: bool) -> SettingSchema {
    SettingSchema {
        label: label.to_string(),
        description: None,
        value_type: value_type.to_string(),
        default: None,
        options: None,
        requires_restart,
        secret: false,
    }
}

fn schema_secret(label: &str) -> SettingSchema {
    SettingSchema {
        label: label.to_string(),
        description: None,
        value_type: "secret".to_string(),
        default: None,
        options: None,
        requires_restart: true,
        secret: true,
    }
}

fn schema_select(label: &str, options: &[&str], requires_restart: bool) -> SettingSchema {
    SettingSchema {
        label: label.to_string(),
        description: None,
        value_type: "select".to_string(),
        default: None,
        options: Some(options.iter().map(|s| s.to_string()).collect()),
        requires_restart,
        secret: false,
    }
}
