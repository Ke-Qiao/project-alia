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
                "core.providers.openai.api_family",
                json!(match config.providers.openai.api_family {
                    crate::config::OpenAiApiFamily::ChatCompletions => "chat_completions",
                    crate::config::OpenAiApiFamily::Responses => "responses",
                }),
                schema_select(
                    "OpenAI API Family",
                    &["chat_completions", "responses"],
                    false,
                ),
            ),
            (
                "core.providers.openai.base_url",
                json!(config.providers.openai.base_url),
                schema("OpenAI Base URL", "string", false),
            ),
            (
                "core.providers.anthropic.api_key",
                json!(config.providers.anthropic.api_key),
                schema_secret("Anthropic API Key", false),
            ),
            (
                "core.providers.anthropic.base_url",
                json!(config.providers.anthropic.base_url),
                schema("Anthropic Base URL", "string", false),
            ),
            (
                "core.providers.anthropic.version",
                json!(config.providers.anthropic.version),
                schema("Anthropic API Version", "string", false),
            ),
            (
                "core.providers.gemini.api_key",
                json!(config.providers.gemini.api_key),
                schema_secret("Gemini API Key", false),
            ),
            (
                "core.providers.gemini.base_url",
                json!(config.providers.gemini.base_url),
                schema("Gemini Base URL", "string", false),
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
                "core.providers.gpt_sovits.min_speed_factor",
                json!(config.providers.gpt_sovits.min_speed_factor),
                schema("GPT-SoVITS Min Speed Factor", "number", false),
            ),
            (
                "core.providers.gpt_sovits.max_speed_factor",
                json!(config.providers.gpt_sovits.max_speed_factor),
                schema("GPT-SoVITS Max Speed Factor", "number", false),
            ),
            (
                "core.providers.gpt_sovits.min_temperature",
                json!(config.providers.gpt_sovits.min_temperature),
                schema("GPT-SoVITS Min Temperature", "number", false),
            ),
            (
                "core.providers.gpt_sovits.max_temperature",
                json!(config.providers.gpt_sovits.max_temperature),
                schema("GPT-SoVITS Max Temperature", "number", false),
            ),
            (
                "core.providers.gpt_sovits.min_top_p",
                json!(config.providers.gpt_sovits.min_top_p),
                schema("GPT-SoVITS Min Top P", "number", false),
            ),
            (
                "core.providers.gpt_sovits.max_top_p",
                json!(config.providers.gpt_sovits.max_top_p),
                schema("GPT-SoVITS Max Top P", "number", false),
            ),
            (
                "core.providers.gpt_sovits.min_repetition_penalty",
                json!(config.providers.gpt_sovits.min_repetition_penalty),
                schema("GPT-SoVITS Min Repetition Penalty", "number", false),
            ),
            (
                "core.providers.gpt_sovits.max_repetition_penalty",
                json!(config.providers.gpt_sovits.max_repetition_penalty),
                schema("GPT-SoVITS Max Repetition Penalty", "number", false),
            ),
            (
                "core.providers.gpt_sovits.min_fragment_interval",
                json!(config.providers.gpt_sovits.min_fragment_interval),
                schema("GPT-SoVITS Min Fragment Interval", "number", false),
            ),
            (
                "core.providers.gpt_sovits.max_fragment_interval",
                json!(config.providers.gpt_sovits.max_fragment_interval),
                schema("GPT-SoVITS Max Fragment Interval", "number", false),
            ),
            (
                "core.providers.gpt_sovits.max_pitch_shift_semitones",
                json!(config.providers.gpt_sovits.max_pitch_shift_semitones),
                schema("GPT-SoVITS Max Pitch Shift Semitones", "number", false),
            ),
            (
                "core.providers.gpt_sovits.sample_rate_pitch_fallback_enabled",
                json!(
                    config
                        .providers
                        .gpt_sovits
                        .sample_rate_pitch_fallback_enabled
                ),
                schema(
                    "Enable GPT-SoVITS Sample-Rate Pitch Fallback",
                    "bool",
                    false,
                ),
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
                "core.policies.routing.minor_llm_enabled",
                json!(config.policies.routing.minor_llm_enabled),
                schema("Enable Minor LLM Routing", "bool", true),
            ),
            (
                "core.policies.routing.major_llm_enabled",
                json!(config.policies.routing.major_llm_enabled),
                schema("Enable Major LLM Routing", "bool", true),
            ),
            (
                "core.policies.transcript_correction.enabled",
                json!(config.policies.transcript_correction.enabled),
                schema("Enable STT Transcript Correction", "bool", true),
            ),
            (
                "core.policies.transcript_correction.apply_to_stt",
                json!(config.policies.transcript_correction.apply_to_stt),
                schema("Apply Correction To STT", "bool", true),
            ),
            (
                "core.policies.transcript_correction.provider",
                json!(config.policies.transcript_correction.provider),
                schema("Transcript Correction Provider", "string", false),
            ),
            (
                "core.policies.transcript_correction.timeout_sec",
                json!(config.policies.transcript_correction.timeout_sec),
                schema("Transcript Correction Timeout", "number", false),
            ),
            (
                "core.policies.transcript_correction.max_input_chars",
                json!(config.policies.transcript_correction.max_input_chars),
                schema("Transcript Correction Max Input Chars", "number", false),
            ),
            (
                "core.policies.transcript_correction.min_confidence_to_replace",
                json!(
                    config
                        .policies
                        .transcript_correction
                        .min_confidence_to_replace
                ),
                schema("Transcript Correction Min Confidence", "number", false),
            ),
            (
                "core.policies.transcript_correction.require_confirmation_on_medium_risk",
                json!(
                    config
                        .policies
                        .transcript_correction
                        .require_confirmation_on_medium_risk
                ),
                schema(
                    "Require Confirmation For Risky Transcript Corrections",
                    "bool",
                    true,
                ),
            ),
            (
                "core.policies.transcript_correction.trace_enabled",
                json!(config.policies.transcript_correction.trace_enabled),
                schema("Trace STT Transcript Correction", "bool", true),
            ),
            (
                "core.policies.affect.enabled",
                json!(config.policies.affect.enabled),
                schema("Enable Affect Runtime", "bool", true),
            ),
            (
                "core.policies.affect.baseline_valence",
                json!(config.policies.affect.baseline_valence),
                schema("Affect Baseline Valence", "number", true),
            ),
            (
                "core.policies.affect.baseline_arousal",
                json!(config.policies.affect.baseline_arousal),
                schema("Affect Baseline Arousal", "number", true),
            ),
            (
                "core.policies.affect.baseline_calmness",
                json!(config.policies.affect.baseline_calmness),
                schema("Affect Baseline Calmness", "number", true),
            ),
            (
                "core.policies.affect.baseline_engagement",
                json!(config.policies.affect.baseline_engagement),
                schema("Social Baseline Engagement", "number", true),
            ),
            (
                "core.policies.affect.baseline_fatigue",
                json!(config.policies.affect.baseline_fatigue),
                schema("Resource Baseline Fatigue", "number", true),
            ),
            (
                "core.policies.affect.baseline_battery_stress",
                json!(config.policies.affect.baseline_battery_stress),
                schema("Resource Baseline Battery Stress", "number", true),
            ),
            (
                "core.policies.affect.baseline_thermal_stress",
                json!(config.policies.affect.baseline_thermal_stress),
                schema("Resource Baseline Thermal Stress", "number", true),
            ),
            (
                "core.policies.affect.baseline_sleepiness",
                json!(config.policies.affect.baseline_sleepiness),
                schema("Resource Baseline Sleepiness", "number", true),
            ),
            (
                "core.policies.affect.baseline_interaction_load",
                json!(config.policies.affect.baseline_interaction_load),
                schema("Resource Baseline Interaction Load", "number", true),
            ),
            (
                "core.policies.affect.baseline_trust",
                json!(config.policies.affect.baseline_trust),
                schema("Social Baseline Trust", "number", true),
            ),
            (
                "core.policies.affect.baseline_familiarity",
                json!(config.policies.affect.baseline_familiarity),
                schema("Social Baseline Familiarity", "number", true),
            ),
            (
                "core.policies.affect.homeostasis_rate",
                json!(config.policies.affect.homeostasis_rate),
                schema("Affect Homeostasis Rate", "number", true),
            ),
            (
                "core.policies.affect.max_delta_per_turn",
                json!(config.policies.affect.max_delta_per_turn),
                schema("Affect Max Delta Per Turn", "number", true),
            ),
            (
                "core.policies.affect.fatigue_per_turn",
                json!(config.policies.affect.fatigue_per_turn),
                schema("Affect Fatigue Per Turn", "number", true),
            ),
            (
                "core.policies.affect.long_response_fatigue_delta",
                json!(config.policies.affect.long_response_fatigue_delta),
                schema("Affect Long Response Fatigue Delta", "number", true),
            ),
            (
                "core.policies.affect.turn_appraisal_ttl_sec",
                json!(config.policies.affect.turn_appraisal_ttl_sec),
                schema("Affect Turn Appraisal TTL", "number", true),
            ),
            (
                "core.policies.affect.session_appraisal_ttl_sec",
                json!(config.policies.affect.session_appraisal_ttl_sec),
                schema("Affect Session Appraisal TTL", "number", true),
            ),
            (
                "core.policies.affect.relationship_appraisal_ttl_sec",
                json!(config.policies.affect.relationship_appraisal_ttl_sec),
                schema("Relationship Appraisal TTL", "number", true),
            ),
            (
                "core.policies.affect.appraisal_half_life_sec",
                json!(config.policies.affect.appraisal_half_life_sec),
                schema("Affect Appraisal Half-Life", "number", true),
            ),
            (
                "core.policies.affect.relationship_appraisal_half_life_sec",
                json!(config.policies.affect.relationship_appraisal_half_life_sec),
                schema("Relationship Appraisal Half-Life", "number", true),
            ),
            (
                "core.policies.affect.max_active_appraisal_events",
                json!(config.policies.affect.max_active_appraisal_events),
                schema("Affect Max Active Appraisal Events", "number", true),
            ),
            (
                "core.policies.affect.min_appraisal_confidence",
                json!(config.policies.affect.min_appraisal_confidence),
                schema("Affect Min Appraisal Confidence", "number", true),
            ),
            (
                "core.policies.affect.relationship_persistence_enabled",
                json!(config.policies.affect.relationship_persistence_enabled),
                schema("Persist Relationship Appraisals", "bool", true),
            ),
            (
                "core.policies.affect.prune_persisted_appraisals_on_startup",
                json!(config.policies.affect.prune_persisted_appraisals_on_startup),
                schema("Prune Persisted Appraisals On Startup", "bool", true),
            ),
            (
                "core.policies.affect.persist_appraisal_evidence",
                json!(config.policies.affect.persist_appraisal_evidence),
                schema("Persist Appraisal Evidence", "bool", true),
            ),
            (
                "core.policies.affect.persist_relationship_evidence",
                json!(config.policies.affect.persist_relationship_evidence),
                schema("Persist Relationship Evidence", "bool", true),
            ),
            (
                "core.policies.affect.max_persisted_evidence_chars",
                json!(config.policies.affect.max_persisted_evidence_chars),
                schema("Max Persisted Evidence Characters", "number", true),
            ),
            (
                "core.policies.regulation.enabled",
                json!(config.policies.regulation.enabled),
                schema("Enable Expression Regulation", "bool", true),
            ),
            (
                "core.policies.regulation.max_negative_expression",
                json!(config.policies.regulation.max_negative_expression),
                schema("Max Negative Expression", "number", true),
            ),
            (
                "core.policies.regulation.low_trust_threshold",
                json!(config.policies.regulation.low_trust_threshold),
                schema("Low Trust Threshold", "number", true),
            ),
            (
                "core.policies.regulation.fatigue_threshold",
                json!(config.policies.regulation.fatigue_threshold),
                schema("Fatigue Regulation Threshold", "number", true),
            ),
            (
                "core.policies.regulation.deescalated_max_intensity",
                json!(config.policies.regulation.deescalated_max_intensity),
                schema("De-escalated Max Intensity", "number", true),
            ),
            (
                "core.policies.regulation.open_trust_threshold",
                json!(config.policies.regulation.open_trust_threshold),
                schema("Open Expression Trust Threshold", "number", true),
            ),
            (
                "core.policies.expression.enabled",
                json!(config.policies.expression.enabled),
                schema("Enable Expression Runtime", "bool", true),
            ),
            (
                "core.policies.expression.auto_motion_enabled",
                json!(config.policies.expression.auto_motion_enabled),
                schema("Enable Automatic Expression Motion", "bool", true),
            ),
            (
                "core.policies.expression.default_style",
                json!(config.policies.expression.default_style),
                schema("Default Expression Style", "string", true),
            ),
            (
                "core.policies.expression.max_expression_intensity",
                json!(config.policies.expression.max_expression_intensity),
                schema("Max Expression Intensity", "number", true),
            ),
            (
                "core.policies.expression.max_motion_intensity",
                json!(config.policies.expression.max_motion_intensity),
                schema("Max Expression Motion Intensity", "number", true),
            ),
            (
                "core.policies.expression.max_timeline_actions",
                json!(config.policies.expression.max_timeline_actions),
                schema("Max Expression Timeline Actions", "number", true),
            ),
            (
                "core.policies.expression.max_action_duration_ms",
                json!(config.policies.expression.max_action_duration_ms),
                schema("Max Expression Action Duration", "number", true),
            ),
            (
                "core.policies.expression.allowed_motion_actions",
                json!(config.policies.expression.allowed_motion_actions),
                schema("Allowed Expression Motion Actions", "string[]", true),
            ),
            (
                "core.policies.expression.external_plugin_timeout_ms",
                json!(config.policies.expression.external_plugin_timeout_ms),
                schema("External Expression Plugin Timeout", "number", true),
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
                "core.policies.memory.capture_mode",
                json!(config.policies.memory.capture_mode),
                schema("Memory Capture Mode", "string", false),
            ),
            (
                "core.policies.memory.capture_timeout_sec",
                json!(config.policies.memory.capture_timeout_sec),
                schema("Memory Capture Timeout", "number", false),
            ),
            (
                "core.policies.memory.capture_min_confidence",
                json!(config.policies.memory.capture_min_confidence),
                schema("Memory Capture Min Confidence", "number", false),
            ),
            (
                "core.policies.memory.capture_min_importance",
                json!(config.policies.memory.capture_min_importance),
                schema("Memory Capture Min Importance", "number", false),
            ),
            (
                "core.policies.memory.sensitive_filter_enabled",
                json!(config.policies.memory.sensitive_filter_enabled),
                schema("Block Sensitive Automatic Memories", "bool", false),
            ),
            (
                "core.policies.memory.active_embedding_profile",
                json!(config.policies.memory.active_embedding_profile),
                schema("Active Memory Embedding Profile", "string", false),
            ),
            (
                "core.policies.memory.graph_depth",
                json!(config.policies.memory.graph_depth),
                schema("Memory Graph Depth", "number", false),
            ),
            (
                "core.policies.memory.consolidation_enabled",
                json!(config.policies.memory.consolidation_enabled),
                schema("Memory Consolidation", "bool", false),
            ),
            (
                "core.policies.memory.paused_until",
                json!(config.policies.memory.paused_until),
                schema("Memory Paused Until", "string", false),
            ),
            (
                "core.policies.memory.retrieval_layers",
                json!(config.policies.memory.retrieval_layers),
                schema("Memory Retrieval Layers", "array", false),
            ),
            (
                "core.policies.memory.max_injected_memory_chars",
                json!(config.policies.memory.max_injected_memory_chars),
                schema("Max Injected Memory Characters", "number", false),
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
                "core.policies.insights.memory_extraction_prompt",
                json!(config.policies.insights.memory_extraction_prompt),
                schema("Insights Memory Extraction Prompt", "string", true),
            ),
            (
                "core.policies.insights.memory_consolidation_prompt",
                json!(config.policies.insights.memory_consolidation_prompt),
                schema("Insights Memory Consolidation Prompt", "string", true),
            ),
            (
                "core.policies.insights.reflection_prompt",
                json!(config.policies.insights.reflection_prompt),
                schema("Insights Reflection Prompt", "string", true),
            ),
            (
                "core.policies.insights.habit_detection_prompt",
                json!(config.policies.insights.habit_detection_prompt),
                schema("Insights Habit Detection Prompt", "string", true),
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
            (
                "core.policies.tools.indexed_search_enabled",
                json!(config.policies.tools.indexed_search_enabled),
                schema("Enable Indexed Tool Search", "bool", false),
            ),
            (
                "core.policies.tools.indexed_search_min_similarity",
                json!(config.policies.tools.indexed_search_min_similarity),
                schema("Tool Search Similarity Threshold", "number", false),
            ),
            (
                "core.policies.tools.indexed_search_refresh_missing_embeddings",
                json!(
                    config
                        .policies
                        .tools
                        .indexed_search_refresh_missing_embeddings
                ),
                schema("Refresh Tool Search Embeddings", "bool", false),
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
                "core.profiles.voice.default.reference_voice",
                json!(config.profiles.default_voice_profile().reference_voice),
                schema("Reference Voice", "string", false),
            ),
            (
                "core.profiles.voice.default.style_prompt",
                json!(config.profiles.default_voice_profile().style_prompt),
                schema("Style Prompt", "string", false),
            ),
            (
                "core.profiles.voice.default.language",
                json!(config.profiles.default_voice_profile().language),
                schema("Voice Language", "string", false),
            ),
            (
                "core.profiles.voice.default.provider_specific.gpt_weights_path",
                json!(
                    config
                        .profiles
                        .default_voice_profile()
                        .provider_specific
                        .get("gpt_weights_path")
                ),
                schema("GPT Weights Path", "string", false),
            ),
            (
                "core.profiles.voice.default.provider_specific.sovits_weights_path",
                json!(
                    config
                        .profiles
                        .default_voice_profile()
                        .provider_specific
                        .get("sovits_weights_path")
                ),
                schema("SoVITS Weights Path", "string", false),
            ),
            (
                "core.profiles.voice.default.provider_specific.prompt_language",
                json!(
                    config
                        .profiles
                        .default_voice_profile()
                        .provider_specific
                        .get("prompt_language")
                ),
                schema("Reference Prompt Language", "string", false),
            ),
            (
                "core.profiles.voice.default.provider_specific.speed_factor",
                json!(
                    config
                        .profiles
                        .default_voice_profile()
                        .provider_specific
                        .get("speed_factor")
                ),
                schema("Voice Speed", "number", false),
            ),
            // Provider selection. Core registries/state machines are fixed
            // services and expose extension registries instead of backend slots.
            (
                "core.llm.roles.minor",
                json!(config.llm.roles.minor),
                schema("Minor LLM Profile", "string", true),
            ),
            (
                "core.llm.roles.major",
                json!(config.llm.roles.major),
                schema("Major LLM Profile", "string", true),
            ),
            (
                "core.llm.roles.transcript_correction",
                json!(config.llm.roles.transcript_correction),
                schema("Transcript Correction LLM Profile", "string", true),
            ),
            (
                "core.llm.roles.memory_capture",
                json!(config.llm.roles.memory_capture),
                schema("Memory Capture LLM Profile", "string", true),
            ),
            (
                "core.llm.roles.default",
                json!(config.llm.roles.default),
                schema("Default LLM Profile", "string", true),
            ),
            (
                "core.llm.roles.embedding",
                json!(config.llm.roles.embedding),
                schema("Embedding Profile", "string", true),
            ),
            (
                "core.modules.stt",
                json!(config.modules.stt),
                schema_select("STT Backend", &["whisper_cpp", "noop"], true),
            ),
            (
                "core.modules.tts",
                json!(config.modules.tts),
                schema_select("TTS Backend", &["gpt_sovits", "noop"], true),
            ),
        ];

        let mut profile_entries = Vec::new();
        for (profile_id, profile) in &config.profiles.llm {
            let prefix = format!("core.profiles.llm.{profile_id}");
            profile_entries.push((
                format!("{prefix}.provider"),
                json!(profile.provider),
                schema_select(
                    "LLM Provider",
                    &["openai", "anthropic", "gemini", "noop"],
                    true,
                ),
            ));
            profile_entries.push((
                format!("{prefix}.kind"),
                json!(match profile.kind {
                    crate::config::LlmProfileKind::Chat => "chat",
                    crate::config::LlmProfileKind::Embedding => "embedding",
                }),
                schema_select("LLM Profile Kind", &["chat", "embedding"], true),
            ));
            profile_entries.push((
                format!("{prefix}.model"),
                json!(profile.model),
                schema("LLM Model", "string", false),
            ));
            if profile.kind == crate::config::LlmProfileKind::Embedding {
                profile_entries.push((
                    format!("{prefix}.dimensions"),
                    json!(profile.dimensions),
                    schema("Embedding Dimensions", "number", false),
                ));
            } else {
                profile_entries.push((
                    format!("{prefix}.temperature"),
                    json!(profile.request.temperature),
                    schema("LLM Temperature", "number", false),
                ));
                profile_entries.push((
                    format!("{prefix}.max_output_tokens"),
                    json!(profile.request.max_output_tokens),
                    schema("LLM Max Output Tokens", "number", false),
                ));
                profile_entries.push((
                    format!("{prefix}.top_p"),
                    json!(profile.request.top_p),
                    schema("LLM Top P", "number", false),
                ));
            }
        }

        for (key, value, s) in entries {
            inner.entries.insert(
                key.to_string(),
                SettingEntry {
                    value,
                    schema: Some(s),
                },
            );
        }
        for (key, value, s) in profile_entries {
            inner.entries.insert(
                key,
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
