use anyhow::Context;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub database: DatabaseSection,
    pub auth: AuthSection,
    #[serde(default)]
    pub channels: Vec<ChannelConfig>,
    pub modules: ModulesSection,
    #[serde(default)]
    pub providers: ProvidersSection,
    #[serde(default)]
    pub policies: PoliciesSection,
    #[serde(default)]
    pub profiles: ProfilesSection,
    #[serde(default)]
    pub resources: ResourcesSection,

    /// Directory where the base config file lives — set automatically by `load()`.
    /// Used to resolve relative paths in config fragments (skills_dir, etc.).
    #[serde(skip)]
    pub config_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSection {
    #[serde(default = "default_server_id")]
    pub id: String,
    pub host: String,
    pub port: u16,
    pub log_level: String,
    #[serde(default)]
    pub debug_output: bool,
}

fn default_server_id() -> String {
    "alia-server".to_string()
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
    pub server_keypair_path: PathBuf,
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
        #[serde(default = "default_embedded_discovery_service")]
        discovery_service_name: String,
        #[serde(default = "default_embedded_discovery_timeout")]
        discovery_timeout_sec: u64,
        #[serde(default = "default_reconnect_interval")]
        reconnect_interval_sec: u64,
        #[serde(default = "default_reconnect_max_backoff")]
        reconnect_max_backoff_sec: u64,
        #[serde(default = "default_grpc_timeout")]
        grpc_timeout_sec: u64,
        #[serde(default = "default_true")]
        voice_turn_interrupt_enabled: bool,
        #[serde(default = "default_true")]
        cancel_streaming_tts_on_interrupt: bool,
        #[serde(default = "default_true")]
        discard_interrupted_turn_history: bool,
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

    pub fn channel_type(&self) -> &'static str {
        match self {
            ChannelConfig::Embedded { .. } => "embedded",
            ChannelConfig::Cli { .. } => "cli",
        }
    }
}

fn default_reconnect_interval() -> u64 {
    5
}
fn default_embedded_discovery_service() -> String {
    "_alia-embedded._tcp".to_string()
}
fn default_embedded_discovery_timeout() -> u64 {
    3
}
fn default_reconnect_max_backoff() -> u64 {
    60
}
fn default_grpc_timeout() -> u64 {
    10
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModulesSection {
    #[serde(default = "default_remote_llm_module")]
    pub llm: String,
    #[serde(default = "default_local_llm_module")]
    pub local_llm: String,
    #[serde(default = "default_remote_llm_module")]
    pub remote_llm: String,
    pub stt: String,
    pub tts: String,
    pub embedding: String,
    pub memory: String,
    pub cache: String,
    pub skills: String,
    pub persona: String,
    pub insights: String,
    pub router: String,
    pub rule_engine: String,
    #[serde(default = "default_scheduler_module")]
    pub scheduler: String,
    #[serde(default = "default_tools_module")]
    pub tools: String,
}

fn default_scheduler_module() -> String {
    "automation".to_string()
}

fn default_tools_module() -> String {
    "builtin".to_string()
}
fn default_local_llm_module() -> String {
    "ollama".to_string()
}
fn default_remote_llm_module() -> String {
    "openai".to_string()
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProvidersSection {
    #[serde(default)]
    pub openai: OpenAiProviderSection,
    #[serde(default)]
    pub ollama: OllamaProviderSection,
    #[serde(default)]
    pub whisper_cpp: WhisperCppProviderSection,
    #[serde(default)]
    pub gpt_sovits: GptSovitsProviderSection,
    #[serde(default)]
    pub mem0: Mem0ProviderSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiProviderSection {
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_openai_chat_model")]
    pub chat_model: String,
    #[serde(default = "default_openai_embedding_model")]
    pub embedding_model: String,
    #[serde(default = "default_openai_base_url")]
    pub base_url: String,
}

impl Default for OpenAiProviderSection {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            chat_model: default_openai_chat_model(),
            embedding_model: default_openai_embedding_model(),
            base_url: default_openai_base_url(),
        }
    }
}

fn default_openai_chat_model() -> String {
    "gpt-4o-mini".to_string()
}

fn default_openai_embedding_model() -> String {
    "text-embedding-3-small".to_string()
}

fn default_openai_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaProviderSection {
    #[serde(default = "default_ollama_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_ollama_chat_model")]
    pub chat_model: String,
    #[serde(default = "default_ollama_embedding_model")]
    pub embedding_model: String,
}

impl Default for OllamaProviderSection {
    fn default() -> Self {
        Self {
            endpoint: default_ollama_endpoint(),
            chat_model: default_ollama_chat_model(),
            embedding_model: default_ollama_embedding_model(),
        }
    }
}

fn default_ollama_endpoint() -> String {
    "http://localhost:11434".to_string()
}

fn default_ollama_chat_model() -> String {
    "qwen2.5:7b".to_string()
}

fn default_ollama_embedding_model() -> String {
    "bge-m3".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct WhisperCppProviderSection {
    #[serde(default = "default_whisper_cpp_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_whisper_cpp_model")]
    pub model: String,
    #[serde(default = "default_stt_request_timeout")]
    pub request_timeout_sec: u64,
}

impl Default for WhisperCppProviderSection {
    fn default() -> Self {
        Self {
            endpoint: default_whisper_cpp_endpoint(),
            model: default_whisper_cpp_model(),
            request_timeout_sec: default_stt_request_timeout(),
        }
    }
}

fn default_whisper_cpp_endpoint() -> String {
    "http://localhost:8080".to_string()
}

fn default_whisper_cpp_model() -> String {
    "large-v3".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct GptSovitsProviderSection {
    #[serde(default = "default_gpt_sovits_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_tts_request_timeout")]
    pub request_timeout_sec: u64,
    #[serde(default = "default_tts_stream_idle_timeout")]
    pub stream_idle_timeout_sec: u64,
}

impl Default for GptSovitsProviderSection {
    fn default() -> Self {
        Self {
            endpoint: default_gpt_sovits_endpoint(),
            request_timeout_sec: default_tts_request_timeout(),
            stream_idle_timeout_sec: default_tts_stream_idle_timeout(),
        }
    }
}

fn default_gpt_sovits_endpoint() -> String {
    "http://localhost:9880/tts".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Mem0ProviderSection {
    #[serde(default = "default_mem0_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub user_id_prefix: String,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub custom_instructions: Option<String>,
    #[serde(default)]
    pub infer: bool,
    #[serde(default = "default_true")]
    pub conversation_infer: bool,
    #[serde(default = "default_mem0_request_timeout")]
    pub request_timeout_sec: u64,
    #[serde(default = "default_mem0_page_size")]
    pub page_size: u32,
    #[serde(default)]
    pub add_poll_timeout_ms: u64,
}

impl Default for Mem0ProviderSection {
    fn default() -> Self {
        Self {
            base_url: default_mem0_base_url(),
            api_key: String::new(),
            user_id_prefix: String::new(),
            app_id: None,
            agent_id: None,
            run_id: None,
            custom_instructions: None,
            infer: false,
            conversation_infer: true,
            request_timeout_sec: default_mem0_request_timeout(),
            page_size: default_mem0_page_size(),
            add_poll_timeout_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PoliciesSection {
    #[serde(default)]
    pub routing: RoutingPolicySection,
    #[serde(default)]
    pub conversation: ConversationSection,
    #[serde(default)]
    pub memory: MemorySection,
    #[serde(default)]
    pub cache: CacheSection,
    #[serde(default)]
    pub insights: InsightsSection,
    #[serde(default)]
    pub rules: RulesSection,
    #[serde(default)]
    pub scheduler: SchedulerSection,
    #[serde(default)]
    pub tools: ToolsSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RoutingPolicySection {
    #[serde(default = "default_router_model")]
    pub model: String,
    #[serde(default = "default_router_temperature")]
    pub temperature: f32,
    #[serde(default = "default_router_timeout")]
    pub timeout_sec: u64,
    #[serde(default = "default_true")]
    pub local_llm_enabled: bool,
    #[serde(default = "default_true")]
    pub remote_llm_enabled: bool,
}

impl Default for RoutingPolicySection {
    fn default() -> Self {
        Self {
            model: default_router_model(),
            temperature: default_router_temperature(),
            timeout_sec: default_router_timeout(),
            local_llm_enabled: true,
            remote_llm_enabled: true,
        }
    }
}

fn default_router_model() -> String {
    "qwen3:4b".to_string()
}

fn default_router_temperature() -> f32 {
    0.1
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfilesSection {
    #[serde(default = "default_voice_profile_id")]
    pub default_voice: String,
    #[serde(default)]
    pub voice: BTreeMap<String, VoiceProfileSection>,
}

impl Default for ProfilesSection {
    fn default() -> Self {
        Self {
            default_voice: default_voice_profile_id(),
            voice: BTreeMap::new(),
        }
    }
}

impl ProfilesSection {
    pub fn default_voice_profile(&self) -> VoiceProfileSection {
        self.voice
            .get(&self.default_voice)
            .cloned()
            .unwrap_or_default()
    }
}

fn default_voice_profile_id() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceProfileSection {
    #[serde(default = "default_gpt_sovits_provider")]
    pub provider: String,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub gpt_weights_path: Option<String>,
    #[serde(default)]
    pub sovits_weights_path: Option<String>,
    #[serde(default)]
    pub ref_audio_path: Option<String>,
    #[serde(default)]
    pub aux_ref_audio_paths: Vec<String>,
    #[serde(default)]
    pub prompt_text: Option<String>,
    #[serde(default = "default_gpt_sovits_language")]
    pub text_lang: String,
    #[serde(default = "default_gpt_sovits_prompt_language")]
    pub prompt_lang: String,
    #[serde(default = "default_gpt_sovits_text_split_method")]
    pub text_split_method: String,
    #[serde(default = "default_gpt_sovits_batch_size")]
    pub batch_size: u32,
    #[serde(default = "default_gpt_sovits_batch_threshold")]
    pub batch_threshold: f32,
    #[serde(default = "default_true")]
    pub split_bucket: bool,
    #[serde(default = "default_gpt_sovits_speed_factor")]
    pub speed_factor: f32,
    #[serde(default = "default_gpt_sovits_fragment_interval")]
    pub fragment_interval: f32,
    #[serde(default = "default_gpt_sovits_seed")]
    pub seed: i32,
    #[serde(default = "default_true")]
    pub parallel_infer: bool,
    #[serde(default = "default_gpt_sovits_repetition_penalty")]
    pub repetition_penalty: f32,
    #[serde(default = "default_gpt_sovits_sample_steps")]
    pub sample_steps: u32,
    #[serde(default)]
    pub super_sampling: bool,
    #[serde(default = "default_gpt_sovits_overlap_length")]
    pub overlap_length: u32,
    #[serde(default = "default_gpt_sovits_min_chunk_length")]
    pub min_chunk_length: u32,
}

impl Default for VoiceProfileSection {
    fn default() -> Self {
        Self {
            provider: default_gpt_sovits_provider(),
            model_name: None,
            gpt_weights_path: None,
            sovits_weights_path: None,
            ref_audio_path: None,
            aux_ref_audio_paths: Vec::new(),
            prompt_text: None,
            text_lang: default_gpt_sovits_language(),
            prompt_lang: default_gpt_sovits_prompt_language(),
            text_split_method: default_gpt_sovits_text_split_method(),
            batch_size: default_gpt_sovits_batch_size(),
            batch_threshold: default_gpt_sovits_batch_threshold(),
            split_bucket: true,
            speed_factor: default_gpt_sovits_speed_factor(),
            fragment_interval: default_gpt_sovits_fragment_interval(),
            seed: default_gpt_sovits_seed(),
            parallel_infer: true,
            repetition_penalty: default_gpt_sovits_repetition_penalty(),
            sample_steps: default_gpt_sovits_sample_steps(),
            super_sampling: false,
            overlap_length: default_gpt_sovits_overlap_length(),
            min_chunk_length: default_gpt_sovits_min_chunk_length(),
        }
    }
}

fn default_gpt_sovits_provider() -> String {
    "gpt_sovits".to_string()
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ResourcesSection {
    /// Directory containing `.md` skill definition files (TOML frontmatter).
    /// Relative paths are resolved from the config file's parent directory.
    #[serde(default)]
    pub skills_dir: Option<String>,
    #[serde(default)]
    pub cache_policy: CachePolicyResourceSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CachePolicyResourceSection {
    #[serde(default)]
    pub dynamic_query_keywords: Vec<String>,
    #[serde(default)]
    pub memory_query_keywords: Vec<String>,
    #[serde(default)]
    pub action_query_keywords: Vec<String>,
}

impl Default for CachePolicyResourceSection {
    fn default() -> Self {
        Self {
            dynamic_query_keywords: Vec::new(),
            memory_query_keywords: Vec::new(),
            action_query_keywords: Vec::new(),
        }
    }
}

fn default_router_timeout() -> u64 {
    5
}
fn default_stt_request_timeout() -> u64 {
    30
}
fn default_tts_request_timeout() -> u64 {
    60
}
fn default_tts_stream_idle_timeout() -> u64 {
    10
}

fn default_true() -> bool {
    true
}

fn default_gpt_sovits_language() -> String {
    "zh".to_string()
}
fn default_gpt_sovits_prompt_language() -> String {
    default_gpt_sovits_language()
}
fn default_gpt_sovits_text_split_method() -> String {
    "cut5".to_string()
}
fn default_gpt_sovits_batch_size() -> u32 {
    1
}
fn default_gpt_sovits_batch_threshold() -> f32 {
    0.75
}
fn default_gpt_sovits_speed_factor() -> f32 {
    1.0
}
fn default_gpt_sovits_fragment_interval() -> f32 {
    0.3
}
fn default_gpt_sovits_seed() -> i32 {
    -1
}
fn default_gpt_sovits_repetition_penalty() -> f32 {
    1.35
}
fn default_gpt_sovits_sample_steps() -> u32 {
    32
}
fn default_gpt_sovits_overlap_length() -> u32 {
    2
}
fn default_gpt_sovits_min_chunk_length() -> u32 {
    16
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConversationSection {
    pub max_context_messages: usize,
    pub streaming_enabled: bool,
}

impl Default for ConversationSection {
    fn default() -> Self {
        Self {
            max_context_messages: 20,
            streaming_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemorySection {
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub conversation_ingest_enabled: bool,
    pub max_retrieval_results: u32,
    pub similarity_threshold: f32,
    #[serde(default = "default_memory_health_check_attempts")]
    pub health_check_attempts: u32,
    #[serde(default = "default_memory_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
}

impl Default for MemorySection {
    fn default() -> Self {
        Self {
            enabled: true,
            conversation_ingest_enabled: true,
            max_retrieval_results: 10,
            similarity_threshold: 0.3,
            health_check_attempts: default_memory_health_check_attempts(),
            health_check_interval_ms: default_memory_health_check_interval_ms(),
        }
    }
}

fn default_memory_health_check_attempts() -> u32 {
    3
}

fn default_memory_health_check_interval_ms() -> u64 {
    500
}

fn default_mem0_base_url() -> String {
    "https://api.mem0.ai".to_string()
}

fn default_mem0_request_timeout() -> u64 {
    20
}

fn default_mem0_page_size() -> u32 {
    50
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheSection {
    pub enabled: bool,
    pub exact_match_enabled: bool,
    pub semantic_match_enabled: bool,
    pub semantic_threshold: f32,
    pub default_ttl_seconds: u64,
    pub skill_cache_ttl_seconds: u64,
}

impl Default for CacheSection {
    fn default() -> Self {
        Self {
            enabled: true,
            exact_match_enabled: true,
            semantic_match_enabled: true,
            semantic_threshold: 0.92,
            default_ttl_seconds: 3600,
            skill_cache_ttl_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct InsightsSection {
    pub enabled: bool,
    pub extraction_enabled: bool,
    pub consolidation_idle_minutes: u64,
    pub reflection_schedule: String,
    pub habit_detection_schedule: String,
    pub memory_threshold: u32,
    pub max_concurrent_insights: u32,
    /// Sensor ID for presence detection (e.g. "presence"); empty to disable sensor-triggered insights
    #[serde(default)]
    pub presence_sensor_id: String,
    /// Default conditions applied to insights cron tasks (JSON condition objects).
    /// Users can override per-user via API.
    /// Example: [{"type": "time_range", "after": "00:00", "before": "06:00"}]
    #[serde(default)]
    pub conditions: Vec<serde_json::Value>,
}

impl Default for InsightsSection {
    fn default() -> Self {
        Self {
            enabled: false,
            extraction_enabled: false,
            consolidation_idle_minutes: 30,
            reflection_schedule: "0 3 * * *".to_string(),
            habit_detection_schedule: "0 4 * * *".to_string(),
            memory_threshold: 50,
            max_concurrent_insights: 1,
            presence_sensor_id: String::new(),
            conditions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RulesSection {
    pub max_rules_per_user: u32,
}

impl Default for RulesSection {
    fn default() -> Self {
        Self {
            max_rules_per_user: 50,
        }
    }
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

#[derive(Debug, Clone, Deserialize)]
pub struct ToolsSection {
    #[serde(default = "default_tool_call_depth")]
    pub max_tool_call_depth: u32,
    #[serde(default = "default_tool_timeout")]
    pub tool_call_timeout_sec: u64,
    #[serde(default = "default_max_tools_per_turn")]
    pub max_tools_per_turn: usize,
    #[serde(default = "default_tool_candidate_pool_size")]
    pub candidate_pool_size: usize,
    #[serde(default = "default_true")]
    pub llm_candidate_filter_enabled: bool,
    #[serde(default = "default_llm_candidate_filter_threshold")]
    pub llm_candidate_filter_threshold: usize,
    #[serde(default = "default_allow_plugin_tools")]
    pub allow_plugin_tools: bool,
    #[serde(default)]
    pub require_confirmation: Vec<String>,
}

impl Default for ToolsSection {
    fn default() -> Self {
        Self {
            max_tool_call_depth: default_tool_call_depth(),
            tool_call_timeout_sec: default_tool_timeout(),
            max_tools_per_turn: default_max_tools_per_turn(),
            candidate_pool_size: default_tool_candidate_pool_size(),
            llm_candidate_filter_enabled: true,
            llm_candidate_filter_threshold: default_llm_candidate_filter_threshold(),
            allow_plugin_tools: default_allow_plugin_tools(),
            require_confirmation: Vec::new(),
        }
    }
}

fn default_tool_call_depth() -> u32 {
    5
}
fn default_tool_timeout() -> u64 {
    30
}
fn default_max_tools_per_turn() -> usize {
    8
}
fn default_tool_candidate_pool_size() -> usize {
    24
}
fn default_llm_candidate_filter_threshold() -> usize {
    12
}
fn default_allow_plugin_tools() -> bool {
    true
}

impl ServerConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let config_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();

        let content = load_config_with_fragments(&canonical)?;
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
            config.providers.openai.api_key = key;
        }
        if let Ok(key) = std::env::var("MEM0_API_KEY") {
            config.providers.mem0.api_key = key;
        }
        if let Ok(url) = std::env::var("MEM0_BASE_URL") {
            config.providers.mem0.base_url = url;
        }
        if config.auth.server_keypair_path.is_relative() {
            config.auth.server_keypair_path =
                config.config_dir.join(&config.auth.server_keypair_path);
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

pub fn load_config_with_fragments(path: &Path) -> anyhow::Result<String> {
    let mut content = read_toml_value(path)?;
    for fragment in config_fragments(path)? {
        let fragment_value = read_toml_value(&fragment)?;
        merge_toml_values(&mut content, fragment_value);
    }
    toml::to_string(&content).context("failed to serialize merged server config")
}

fn read_toml_value(path: &Path) -> anyhow::Result<toml::Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    content
        .parse::<toml::Value>()
        .with_context(|| format!("failed to parse config file {}", path.display()))
}

fn merge_toml_values(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => merge_toml_values(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => {
            *base = overlay;
        }
    }
}

fn config_fragments(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let fragment_dir = path.with_extension("d");
    let mut fragments = Vec::new();
    collect_toml_fragments(&fragment_dir, &mut fragments)?;
    fragments.sort();
    Ok(fragments)
}

fn collect_toml_fragments(dir: &Path, fragments: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_toml_fragments(&path, fragments)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
            fragments.push(path);
        }
    }

    Ok(())
}
