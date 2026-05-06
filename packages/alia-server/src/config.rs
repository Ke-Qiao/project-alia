use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub database: DatabaseSection,
    pub auth: AuthSection,
    pub embedded: EmbeddedSection,
    pub modules: ModulesSection,
    pub ai: AiSection,
    pub rules: RulesSection,
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
    pub presence_sensor_plugin: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RulesSection {
    pub evaluation_interval_sec: u64,
    pub max_rules_per_user: u32,
}

impl ServerConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut config: ServerConfig = toml::from_str(&content)?;

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
}
