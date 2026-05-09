use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub database: DatabaseSection,
    pub auth: AuthSection,
    #[serde(default)]
    pub channels: Vec<ChannelConfig>,
    pub modules: ModulesSection,
    pub ai: AiSection,
    pub rules: RulesSection,
    #[serde(default)]
    pub scheduler: SchedulerSection,
    #[serde(default)]
    pub tools: ToolsSection,
    #[serde(default)]
    pub skills: SkillsSection,

    /// Directory where the config file lives — set automatically by `load()`.
    /// Used to resolve relative paths in the config (skills_dir, etc.).
    #[serde(skip)]
    pub config_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSection {
    pub host: String,
    pub port: u16,
    pub log_level: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseSection {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthSection {
    pub jwt_secret: String,
    pub jwt_expiry_hours: u64,
    pub refresh_expiry_days: u64,
    pub argon2_memory_kib: u32,
    pub argon2_iterations: u32,
    pub argon2_parallelism: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddedSection {
    pub endpoint: String,
    pub reconnect_interval_sec: u64,
    pub reconnect_max_backoff_sec: u64,
    pub grpc_timeout_sec: u64,
}

/// Configuration for a single channel instance.
/// The `type` field determines which adapter to create.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ChannelConfig {
    #[serde(rename = "embedded")]
    Embedded {
        id: String,
        #[serde(default = "default_true")]
        enabled: bool,
        #[serde(default)]
        endpoint: String,
        #[serde(default = "default_reconnect_interval")]
        reconnect_interval_sec: u64,
        #[serde(default = "default_reconnect_max_backoff")]
        reconnect_max_backoff_sec: u64,
        #[serde(default = "default_grpc_timeout")]
        grpc_timeout_sec: u64,
    },
    #[serde(rename = "cli")]
    Cli {
        id: String,
        #[serde(default = "default_true")]
        enabled: bool,
    },
}

impl ChannelConfig {
    pub fn id(&self) -> &str {
        match self {
            ChannelConfig::Embedded { id, .. } => id,
            ChannelConfig::Cli { id, .. } => id,
        }
    }

    pub fn enabled(&self) -> bool {
        match self {
            ChannelConfig::Embedded { enabled, .. } => *enabled,
            ChannelConfig::Cli { enabled, .. } => *enabled,
        }
    }
}

fn default_reconnect_interval() -> u64 {
    5
}
fn default_reconnect_max_backoff() -> u64 {
    60
}
fn default_grpc_timeout() -> u64 {
    10
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModulesSection {
    pub llm: String,
    pub stt: String,
    pub tts: String,
    pub embedding: String,
    pub memory: String,
    pub cache: String,
    pub skills: String,
    pub persona: String,
    pub meditation: String,
    pub router: String,
    pub rule_engine: String,
    #[serde(default = "default_scheduler_module")]
    pub scheduler: String,
    #[serde(default = "default_tools_module")]
    pub tools: String,
}

fn default_scheduler_module() -> String {
    "cron".to_string()
}

fn default_tools_module() -> String {
    "builtin".to_string()
}

fn default_router_timeout() -> u64 {
    5
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct AiSection {
    pub llm_adapter: String,
    pub openai_api_key: String,
    pub openai_model: String,
    pub openai_base_url: String,
    pub ollama_endpoint: String,
    pub ollama_model: String,
    pub router_model: String,
    pub router_temperature: f32,
    #[serde(default = "default_router_timeout")]
    pub router_timeout_sec: u64,
    #[serde(default = "default_true")]
    pub router_local_llm_enabled: bool,
    #[serde(default = "default_true")]
    pub router_remote_llm_enabled: bool,
    pub embedding_adapter: String,
    pub ollama_embedding_model: String,
    pub openai_embedding_model: String,
    pub stt_adapter: String,
    pub whisper_cpp_endpoint: String,
    pub whisper_cpp_model: String,
    pub tts_adapter: String,
    pub gpt_sovits_tts_endpoint: String,
    pub max_conversation_context: usize,
    pub streaming_enabled: bool,
    pub memory: AiMemorySection,
    pub cache: AiCacheSection,
    pub meditation: AiMeditationSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AiMemorySection {
    pub enabled: bool,
    pub epidosic_ttl_days: u32,
    pub max_core_memories: u32,
    pub max_retrieval_results: u32,
    pub similarity_threshold: f32,
    pub graph_community_algorithm: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AiCacheSection {
    pub enabled: bool,
    pub exact_match_enabled: bool,
    pub semantic_match_enabled: bool,
    pub semantic_threshold: f32,
    pub default_ttl_seconds: u64,
    pub skill_cache_ttl_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AiMeditationSection {
    pub enabled: bool,
    pub extraction_enabled: bool,
    pub consolidation_idle_minutes: u64,
    pub reflection_schedule: String,
    pub habit_detection_schedule: String,
    pub memory_threshold: u32,
    pub max_concurrent_meditations: u32,
    /// Sensor ID for presence detection (e.g. "presence"); empty to disable sensor-triggered meditation
    #[serde(default)]
    pub presence_sensor_id: String,
    /// Default conditions applied to meditation cron tasks (JSON condition objects).
    /// Users can override per-user via API.
    /// Example: [{"type": "time_range", "after": "00:00", "before": "06:00"}]
    #[serde(default)]
    pub conditions: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RulesSection {
    pub evaluation_interval_sec: u64,
    pub max_rules_per_user: u32,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulerSection {
    #[serde(default = "default_scheduler_enabled")]
    pub enabled: bool,
    #[serde(default = "default_max_tasks")]
    pub max_tasks_per_user: u32,
    #[serde(default = "default_max_subscriptions")]
    pub max_subscriptions_per_user: u32,
    #[serde(default = "default_tick_interval")]
    pub tick_interval_sec: u64,
    #[serde(default = "default_missed_task_policy")]
    pub missed_task_policy: String,
    #[serde(default = "default_task_timeout")]
    pub task_timeout_sec: u64,
    #[serde(default = "default_max_concurrent_tasks")]
    pub max_concurrent_tasks: u32,
}

fn default_scheduler_enabled() -> bool {
    true
}
fn default_max_tasks() -> u32 {
    100
}
fn default_max_subscriptions() -> u32 {
    50
}
fn default_tick_interval() -> u64 {
    1
}
fn default_missed_task_policy() -> String {
    "skip".to_string()
}
fn default_task_timeout() -> u64 {
    30
}
fn default_max_concurrent_tasks() -> u32 {
    5
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ToolsSection {
    #[serde(default = "default_tool_call_depth")]
    pub max_tool_call_depth: u32,
    #[serde(default = "default_tool_timeout")]
    pub tool_call_timeout_sec: u64,
    #[serde(default = "default_allow_plugin_tools")]
    pub allow_plugin_tools: bool,
    #[serde(default)]
    pub require_confirmation: Vec<String>,
}

fn default_tool_call_depth() -> u32 {
    5
}
fn default_tool_timeout() -> u64 {
    30
}
fn default_allow_plugin_tools() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SkillsSection {
    /// Directory containing `.md` skill definition files (TOML frontmatter).
    /// Relative paths are resolved from the config file's parent directory.
    #[serde(default)]
    pub skills_dir: Option<String>,
}

impl ServerConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let config_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();

        let content = std::fs::read_to_string(path)?;
        let mut config: ServerConfig = toml::from_str(&content)?;
        config.config_dir = config_dir;

        // Environment variable overrides for sensitive fields
        if let Ok(url) = std::env::var("DATABASE_URL") {
            config.database.url = url;
        }
        if let Ok(secret) = std::env::var("ALIA_JWT_SECRET") {
            config.auth.jwt_secret = secret;
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            config.ai.openai_api_key = key;
        }

        Ok(config)
    }

    /// Resolve a path that may be relative. Relative paths are resolved
    /// against the config file's parent directory, so the binary can be
    /// run from any working directory.
    pub fn resolve_path(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.config_dir.join(p)
        }
    }

    /// Search for the config file in standard locations.
    /// Priority: `ALIA_CONFIG_PATH` env > `./config/server.toml` > `~/.config/alia/server.toml` > `/etc/alia/server.toml`
    pub fn discover_config_path() -> String {
        if let Ok(p) = std::env::var("ALIA_CONFIG_PATH") {
            return p;
        }

        let candidates = [
            "config/server.toml".to_string(),
            dirs::config_dir()
                .map(|d| {
                    d.join("alia")
                        .join("server.toml")
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_default(),
            "/etc/alia/server.toml".to_string(),
        ];

        for c in &candidates {
            if !c.is_empty() && Path::new(c).exists() {
                return c.clone();
            }
        }

        // Fall back to the first candidate even if it doesn't exist,
        // so the error message is clear.
        candidates[0].clone()
    }
}
