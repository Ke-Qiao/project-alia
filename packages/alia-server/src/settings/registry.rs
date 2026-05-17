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
/// - `core.providers.openai.api_key` — provider settings
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
                schema_secret("Database URL", true),
            ),
            (
                "core.database.max_connections",
                json!(config.database.max_connections),
                schema("Max Connections", "number", true),
            ),
            // Providers
            (
                "core.providers.openai.api_key",
                json!(config.providers.openai.api_key),
                schema_secret("OpenAI API Key", false),
            ),
            (
                "core.providers.openai.chat_model",
                json!(config.providers.openai.chat_model),
                schema("OpenAI Model", "string", false),
            ),
            (
                "core.providers.openai.embedding_model",
                json!(config.providers.openai.embedding_model),
                schema("OpenAI Embedding Model", "string", false),
            ),
            (
                "core.providers.openai.temperature",
                json!(0.7),
                schema("OpenAI Temperature", "number", false),
            ),
            (
                "core.providers.openai.base_url",
                json!(config.providers.openai.base_url),
                schema("OpenAI Base URL", "string", false),
            ),
            (
                "core.providers.ollama.endpoint",
                json!(config.providers.ollama.endpoint),
                schema("Ollama Endpoint", "string", false),
            ),
            (
                "core.providers.ollama.chat_model",
                json!(config.providers.ollama.chat_model),
                schema("Ollama Model", "string", false),
            ),
            (
                "core.providers.ollama.embedding_model",
                json!(config.providers.ollama.embedding_model),
                schema("Ollama Embedding Model", "string", false),
            ),
            (
                "core.providers.ollama.temperature",
                json!(0.7),
                schema("Ollama Temperature", "number", false),
            ),
            (
                "core.providers.whisper_cpp.endpoint",
                json!(config.providers.whisper_cpp.endpoint),
                schema("Whisper.cpp Endpoint", "string", false),
            ),
            (
                "core.providers.whisper_cpp.language",
                json!("auto"),
                schema("Whisper.cpp Language", "string", false),
            ),
            (
                "core.providers.whisper_cpp.request_timeout_sec",
                json!(config.providers.whisper_cpp.request_timeout_sec),
                schema("Whisper.cpp Timeout", "number", false),
            ),
            (
                "core.providers.gpt_sovits.endpoint",
                json!(config.providers.gpt_sovits.endpoint),
                schema("GPT-SoVITS Endpoint", "string", false),
            ),
            (
                "core.providers.gpt_sovits.request_timeout_sec",
                json!(config.providers.gpt_sovits.request_timeout_sec),
                schema("GPT-SoVITS Timeout", "number", false),
            ),
            (
                "core.providers.gpt_sovits.stream_idle_timeout_sec",
                json!(config.providers.gpt_sovits.stream_idle_timeout_sec),
                schema("GPT-SoVITS Stream Idle Timeout", "number", false),
            ),
            (
                "core.providers.mem0.base_url",
                json!(config.providers.mem0.base_url),
                schema("Mem0 Base URL", "string", true),
            ),
            (
                "core.providers.mem0.api_key",
                json!(config.providers.mem0.api_key),
                schema_secret("Mem0 API Key", true),
            ),
            // Policies
            (
                "core.policies.routing.model",
                json!(config.policies.routing.model),
                schema("Router Model", "string", false),
            ),
            (
                "core.policies.routing.temperature",
                json!(config.policies.routing.temperature),
                schema("Router Temperature", "number", false),
            ),
            (
                "core.policies.routing.timeout_sec",
                json!(config.policies.routing.timeout_sec),
                schema("Router Timeout", "number", false),
            ),
            (
                "core.policies.routing.local_llm_enabled",
                json!(config.policies.routing.local_llm_enabled),
                schema("Enable Local LLM Routing", "bool", true),
            ),
            (
                "core.policies.routing.remote_llm_enabled",
                json!(config.policies.routing.remote_llm_enabled),
                schema("Enable Remote LLM Routing", "bool", true),
            ),
            (
                "core.policies.conversation.streaming_enabled",
                json!(config.policies.conversation.streaming_enabled),
                schema("Streaming", "bool", false),
            ),
            (
                "core.policies.conversation.max_context_messages",
                json!(config.policies.conversation.max_context_messages),
                schema("Max Context Turns", "number", false),
            ),
            (
                "core.policies.cache.enabled",
                json!(config.policies.cache.enabled),
                schema("Enable Cache", "bool", false),
            ),
            (
                "core.policies.cache.exact_match_enabled",
                json!(config.policies.cache.exact_match_enabled),
                schema("Enable Exact Cache", "bool", false),
            ),
            (
                "core.policies.cache.semantic_match_enabled",
                json!(config.policies.cache.semantic_match_enabled),
                schema("Enable Semantic Cache", "bool", false),
            ),
            (
                "core.policies.cache.semantic_threshold",
                json!(config.policies.cache.semantic_threshold),
                schema("Semantic Cache Threshold", "number", false),
            ),
            (
                "core.policies.cache.default_ttl_seconds",
                json!(config.policies.cache.default_ttl_seconds),
                schema("Cache TTL", "number", false),
            ),
            (
                "core.policies.cache.skill_cache_ttl_seconds",
                json!(config.policies.cache.skill_cache_ttl_seconds),
                schema("Skill Cache TTL", "number", false),
            ),
            (
                "core.policies.memory.enabled",
                json!(config.policies.memory.enabled),
                schema("Enable Memory", "bool", true),
            ),
            (
                "core.policies.memory.conversation_ingest_enabled",
                json!(config.policies.memory.conversation_ingest_enabled),
                schema("Memory Conversation Ingest", "bool", false),
            ),
            (
                "core.policies.memory.max_retrieval_results",
                json!(config.policies.memory.max_retrieval_results),
                schema("Memory Retrieval Limit", "number", false),
            ),
            (
                "core.policies.memory.similarity_threshold",
                json!(config.policies.memory.similarity_threshold),
                schema("Memory Similarity Threshold", "number", false),
            ),
            (
                "core.policies.tools.max_tool_call_depth",
                json!(config.policies.tools.max_tool_call_depth),
                schema("Max Tool Call Depth", "number", false),
            ),
            (
                "core.policies.tools.tool_call_timeout_sec",
                json!(config.policies.tools.tool_call_timeout_sec),
                schema("Tool Call Timeout", "number", false),
            ),
            (
                "core.policies.tools.max_tools_per_turn",
                json!(config.policies.tools.max_tools_per_turn),
                schema("Max Tools Per Turn", "number", false),
            ),
            (
                "core.policies.tools.candidate_pool_size",
                json!(config.policies.tools.candidate_pool_size),
                schema("Tool Candidate Pool", "number", false),
            ),
            (
                "core.policies.tools.llm_candidate_filter_enabled",
                json!(config.policies.tools.llm_candidate_filter_enabled),
                schema("Enable LLM Tool Filter", "bool", false),
            ),
            (
                "core.policies.tools.llm_candidate_filter_threshold",
                json!(config.policies.tools.llm_candidate_filter_threshold),
                schema("LLM Tool Filter Threshold", "number", false),
            ),
            (
                "core.policies.tools.allow_plugin_tools",
                json!(config.policies.tools.allow_plugin_tools),
                schema("Allow Plugin Tools", "bool", false),
            ),
            // Profiles
            (
                "core.profiles.default_voice",
                json!(config.profiles.default_voice),
                schema("Default Voice Profile", "string", true),
            ),
            (
                "core.profiles.voice.default.model_name",
                json!(config.profiles.default_voice_profile().model_name),
                schema("Voice Model Name", "string", false),
            ),
            (
                "core.profiles.voice.default.gpt_weights_path",
                json!(config.profiles.default_voice_profile().gpt_weights_path),
                schema("GPT Weights Path", "string", false),
            ),
            (
                "core.profiles.voice.default.sovits_weights_path",
                json!(config.profiles.default_voice_profile().sovits_weights_path),
                schema("SoVITS Weights Path", "string", false),
            ),
            (
                "core.profiles.voice.default.ref_audio_path",
                json!(config.profiles.default_voice_profile().ref_audio_path),
                schema("Reference Audio Path", "string", false),
            ),
            (
                "core.profiles.voice.default.prompt_text",
                json!(config.profiles.default_voice_profile().prompt_text),
                schema("Reference Prompt Text", "string", false),
            ),
            (
                "core.profiles.voice.default.text_lang",
                json!(config.profiles.default_voice_profile().text_lang),
                schema("Target Text Language", "string", false),
            ),
            (
                "core.profiles.voice.default.prompt_lang",
                json!(config.profiles.default_voice_profile().prompt_lang),
                schema("Reference Prompt Language", "string", false),
            ),
            (
                "core.profiles.voice.default.speed_factor",
                json!(config.profiles.default_voice_profile().speed_factor),
                schema("Voice Speed", "number", false),
            ),
            // Modules
            (
                "core.modules.llm",
                json!(config.modules.llm),
                schema_select("Default LLM Backend", &["openai", "ollama", "noop"], true),
            ),
            (
                "core.modules.local_llm",
                json!(config.modules.local_llm),
                schema_select("Local LLM Backend", &["ollama", "openai", "noop"], true),
            ),
            (
                "core.modules.remote_llm",
                json!(config.modules.remote_llm),
                schema_select("Remote LLM Backend", &["openai", "ollama", "noop"], true),
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

fn schema_secret(label: &str, requires_restart: bool) -> SettingSchema {
    SettingSchema {
        label: label.to_string(),
        description: None,
        value_type: "secret".to_string(),
        default: None,
        options: None,
        requires_restart,
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
