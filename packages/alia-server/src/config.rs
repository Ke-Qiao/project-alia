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
    #[serde(default = "default_minor_llm_module")]
    pub minor_llm: String,
    #[serde(default = "default_major_llm_module")]
    pub major_llm: String,
    #[serde(default)]
    pub transcript_correction_llm: Option<String>,
    #[serde(default)]
    pub memory_capture_llm: Option<String>,
    pub stt: String,
    pub tts: String,
    pub embedding: String,
    pub memory: String,
}

fn default_minor_llm_module() -> String {
    "ollama".to_string()
}
fn default_major_llm_module() -> String {
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
    #[serde(default = "default_gpt_sovits_min_speed_factor")]
    pub min_speed_factor: f32,
    #[serde(default = "default_gpt_sovits_max_speed_factor")]
    pub max_speed_factor: f32,
    #[serde(default = "default_gpt_sovits_min_temperature")]
    pub min_temperature: f32,
    #[serde(default = "default_gpt_sovits_max_temperature")]
    pub max_temperature: f32,
    #[serde(default = "default_gpt_sovits_min_top_p")]
    pub min_top_p: f32,
    #[serde(default = "default_gpt_sovits_max_top_p")]
    pub max_top_p: f32,
    #[serde(default = "default_gpt_sovits_min_repetition_penalty")]
    pub min_repetition_penalty: f32,
    #[serde(default = "default_gpt_sovits_max_repetition_penalty")]
    pub max_repetition_penalty: f32,
    #[serde(default = "default_gpt_sovits_min_fragment_interval")]
    pub min_fragment_interval: f32,
    #[serde(default = "default_gpt_sovits_max_fragment_interval")]
    pub max_fragment_interval: f32,
    #[serde(default = "default_gpt_sovits_max_pitch_shift_semitones")]
    pub max_pitch_shift_semitones: f32,
    #[serde(default)]
    pub sample_rate_pitch_fallback_enabled: bool,
}

impl Default for GptSovitsProviderSection {
    fn default() -> Self {
        Self {
            endpoint: default_gpt_sovits_endpoint(),
            request_timeout_sec: default_tts_request_timeout(),
            stream_idle_timeout_sec: default_tts_stream_idle_timeout(),
            min_speed_factor: default_gpt_sovits_min_speed_factor(),
            max_speed_factor: default_gpt_sovits_max_speed_factor(),
            min_temperature: default_gpt_sovits_min_temperature(),
            max_temperature: default_gpt_sovits_max_temperature(),
            min_top_p: default_gpt_sovits_min_top_p(),
            max_top_p: default_gpt_sovits_max_top_p(),
            min_repetition_penalty: default_gpt_sovits_min_repetition_penalty(),
            max_repetition_penalty: default_gpt_sovits_max_repetition_penalty(),
            min_fragment_interval: default_gpt_sovits_min_fragment_interval(),
            max_fragment_interval: default_gpt_sovits_max_fragment_interval(),
            max_pitch_shift_semitones: default_gpt_sovits_max_pitch_shift_semitones(),
            sample_rate_pitch_fallback_enabled: false,
        }
    }
}

fn default_gpt_sovits_endpoint() -> String {
    "http://localhost:9880/tts".to_string()
}
fn default_gpt_sovits_min_speed_factor() -> f32 {
    0.85
}
fn default_gpt_sovits_max_speed_factor() -> f32 {
    1.15
}
fn default_gpt_sovits_min_temperature() -> f32 {
    0.75
}
fn default_gpt_sovits_max_temperature() -> f32 {
    1.25
}
fn default_gpt_sovits_min_top_p() -> f32 {
    0.82
}
fn default_gpt_sovits_max_top_p() -> f32 {
    1.0
}
fn default_gpt_sovits_min_repetition_penalty() -> f32 {
    1.05
}
fn default_gpt_sovits_max_repetition_penalty() -> f32 {
    1.55
}
fn default_gpt_sovits_min_fragment_interval() -> f32 {
    0.12
}
fn default_gpt_sovits_max_fragment_interval() -> f32 {
    0.75
}
fn default_gpt_sovits_max_pitch_shift_semitones() -> f32 {
    2.0
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
            conversation_infer: false,
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
    pub transcript_correction: TranscriptCorrectionPolicySection,
    #[serde(default)]
    pub affect: AffectPolicySection,
    #[serde(default)]
    pub regulation: RegulationPolicySection,
    #[serde(default)]
    pub expression: ExpressionPolicySection,
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
pub struct TranscriptCorrectionPolicySection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub apply_to_stt: bool,
    #[serde(default = "default_transcript_correction_provider")]
    pub provider: String,
    #[serde(default = "default_transcript_correction_timeout")]
    pub timeout_sec: u64,
    #[serde(default = "default_transcript_correction_max_input_chars")]
    pub max_input_chars: usize,
    #[serde(default = "default_transcript_correction_min_confidence")]
    pub min_confidence_to_replace: f32,
    #[serde(default = "default_true")]
    pub require_confirmation_on_medium_risk: bool,
    #[serde(default = "default_true")]
    pub trace_enabled: bool,
}

impl Default for TranscriptCorrectionPolicySection {
    fn default() -> Self {
        Self {
            enabled: true,
            apply_to_stt: true,
            provider: default_transcript_correction_provider(),
            timeout_sec: default_transcript_correction_timeout(),
            max_input_chars: default_transcript_correction_max_input_chars(),
            min_confidence_to_replace: default_transcript_correction_min_confidence(),
            require_confirmation_on_medium_risk: true,
            trace_enabled: true,
        }
    }
}

fn default_transcript_correction_provider() -> String {
    "default_llm".to_string()
}

fn default_transcript_correction_timeout() -> u64 {
    2
}

fn default_transcript_correction_max_input_chars() -> usize {
    500
}

fn default_transcript_correction_min_confidence() -> f32 {
    0.55
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegulationPolicySection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_regulation_max_negative_expression")]
    pub max_negative_expression: f32,
    #[serde(default = "default_regulation_max_expressed_arousal")]
    pub max_expressed_arousal: f32,
    #[serde(default = "default_regulation_default_max_intensity")]
    pub default_max_intensity: f32,
    #[serde(default = "default_regulation_low_trust_threshold")]
    pub low_trust_threshold: f32,
    #[serde(default = "default_regulation_low_trust_warmth_floor")]
    pub low_trust_warmth_floor: f32,
    #[serde(default = "default_regulation_low_trust_max_intensity")]
    pub low_trust_max_intensity: f32,
    #[serde(default = "default_regulation_low_trust_motion_scale")]
    pub low_trust_motion_scale: f32,
    #[serde(default = "default_regulation_fatigue_threshold")]
    pub fatigue_threshold: f32,
    #[serde(default = "default_regulation_fatigue_max_intensity")]
    pub fatigue_max_intensity: f32,
    #[serde(default = "default_regulation_fatigue_motion_scale")]
    pub fatigue_motion_scale: f32,
    #[serde(default = "default_regulation_deescalate_arousal_threshold")]
    pub deescalate_arousal_threshold: f32,
    #[serde(default = "default_regulation_deescalated_arousal")]
    pub deescalated_arousal: f32,
    #[serde(default = "default_regulation_deescalation_warmth_floor")]
    pub deescalation_warmth_floor: f32,
    #[serde(default = "default_regulation_deescalated_max_intensity")]
    pub deescalated_max_intensity: f32,
    #[serde(default = "default_regulation_deescalated_motion_scale")]
    pub deescalated_motion_scale: f32,
    #[serde(default = "default_regulation_apology_max_intensity")]
    pub apology_max_intensity: f32,
    #[serde(default = "default_regulation_apology_motion_scale")]
    pub apology_motion_scale: f32,
    #[serde(default = "default_regulation_warning_max_intensity")]
    pub warning_max_intensity: f32,
    #[serde(default = "default_regulation_open_trust_threshold")]
    pub open_trust_threshold: f32,
    #[serde(default = "default_regulation_open_familiarity_threshold")]
    pub open_familiarity_threshold: f32,
    #[serde(default = "default_regulation_open_positive_intensity_scale")]
    pub open_positive_intensity_scale: f32,
    #[serde(default = "default_regulation_open_positive_motion_scale")]
    pub open_positive_motion_scale: f32,
}

impl Default for RegulationPolicySection {
    fn default() -> Self {
        Self {
            enabled: true,
            max_negative_expression: default_regulation_max_negative_expression(),
            max_expressed_arousal: default_regulation_max_expressed_arousal(),
            default_max_intensity: default_regulation_default_max_intensity(),
            low_trust_threshold: default_regulation_low_trust_threshold(),
            low_trust_warmth_floor: default_regulation_low_trust_warmth_floor(),
            low_trust_max_intensity: default_regulation_low_trust_max_intensity(),
            low_trust_motion_scale: default_regulation_low_trust_motion_scale(),
            fatigue_threshold: default_regulation_fatigue_threshold(),
            fatigue_max_intensity: default_regulation_fatigue_max_intensity(),
            fatigue_motion_scale: default_regulation_fatigue_motion_scale(),
            deescalate_arousal_threshold: default_regulation_deescalate_arousal_threshold(),
            deescalated_arousal: default_regulation_deescalated_arousal(),
            deescalation_warmth_floor: default_regulation_deescalation_warmth_floor(),
            deescalated_max_intensity: default_regulation_deescalated_max_intensity(),
            deescalated_motion_scale: default_regulation_deescalated_motion_scale(),
            apology_max_intensity: default_regulation_apology_max_intensity(),
            apology_motion_scale: default_regulation_apology_motion_scale(),
            warning_max_intensity: default_regulation_warning_max_intensity(),
            open_trust_threshold: default_regulation_open_trust_threshold(),
            open_familiarity_threshold: default_regulation_open_familiarity_threshold(),
            open_positive_intensity_scale: default_regulation_open_positive_intensity_scale(),
            open_positive_motion_scale: default_regulation_open_positive_motion_scale(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AffectPolicySection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_affect_baseline_valence")]
    pub baseline_valence: f32,
    #[serde(default = "default_affect_baseline_arousal")]
    pub baseline_arousal: f32,
    #[serde(default = "default_affect_baseline_calmness")]
    pub baseline_calmness: f32,
    #[serde(default = "default_affect_baseline_engagement")]
    pub baseline_engagement: f32,
    #[serde(default = "default_affect_baseline_fatigue")]
    pub baseline_fatigue: f32,
    #[serde(default = "default_affect_baseline_battery_stress")]
    pub baseline_battery_stress: f32,
    #[serde(default = "default_affect_baseline_thermal_stress")]
    pub baseline_thermal_stress: f32,
    #[serde(default = "default_affect_baseline_sleepiness")]
    pub baseline_sleepiness: f32,
    #[serde(default = "default_affect_baseline_interaction_load")]
    pub baseline_interaction_load: f32,
    #[serde(default = "default_affect_baseline_trust")]
    pub baseline_trust: f32,
    #[serde(default = "default_affect_baseline_familiarity")]
    pub baseline_familiarity: f32,
    #[serde(default = "default_affect_homeostasis_rate")]
    pub homeostasis_rate: f32,
    #[serde(default = "default_affect_max_delta_per_turn")]
    pub max_delta_per_turn: f32,
    #[serde(default = "default_affect_fatigue_per_turn")]
    pub fatigue_per_turn: f32,
    #[serde(default = "default_affect_long_response_fatigue_delta")]
    pub long_response_fatigue_delta: f32,
    #[serde(default = "default_affect_turn_appraisal_ttl")]
    pub turn_appraisal_ttl_sec: u64,
    #[serde(default = "default_affect_session_appraisal_ttl")]
    pub session_appraisal_ttl_sec: u64,
    #[serde(default = "default_affect_relationship_appraisal_ttl")]
    pub relationship_appraisal_ttl_sec: u64,
    #[serde(default = "default_affect_appraisal_half_life")]
    pub appraisal_half_life_sec: u64,
    #[serde(default = "default_affect_relationship_appraisal_half_life")]
    pub relationship_appraisal_half_life_sec: u64,
    #[serde(default = "default_affect_max_active_appraisal_events")]
    pub max_active_appraisal_events: usize,
    #[serde(default = "default_affect_min_appraisal_confidence")]
    pub min_appraisal_confidence: f32,
    #[serde(default = "default_true")]
    pub relationship_persistence_enabled: bool,
    #[serde(default = "default_true")]
    pub prune_persisted_appraisals_on_startup: bool,
    #[serde(default = "default_true")]
    pub persist_appraisal_evidence: bool,
    #[serde(default)]
    pub persist_relationship_evidence: bool,
    #[serde(default = "default_true")]
    pub relationship_profile_enabled: bool,
    #[serde(default = "default_true")]
    pub relationship_profile_user_visible: bool,
    #[serde(default = "default_true")]
    pub relationship_profile_store_preferences: bool,
    #[serde(default = "default_true")]
    pub relationship_profile_user_clearable: bool,
    #[serde(default = "default_affect_relationship_preferences_max_items")]
    pub relationship_preferences_max_items: usize,
    #[serde(default = "default_affect_relationship_summary_max_signals")]
    pub relationship_summary_max_signals: usize,
    #[serde(default = "default_affect_relationship_profile_prune_after")]
    pub relationship_profile_prune_after_sec: u64,
    #[serde(default = "default_affect_max_persisted_evidence_chars")]
    pub max_persisted_evidence_chars: usize,
}

impl Default for AffectPolicySection {
    fn default() -> Self {
        Self {
            enabled: true,
            baseline_valence: default_affect_baseline_valence(),
            baseline_arousal: default_affect_baseline_arousal(),
            baseline_calmness: default_affect_baseline_calmness(),
            baseline_engagement: default_affect_baseline_engagement(),
            baseline_fatigue: default_affect_baseline_fatigue(),
            baseline_battery_stress: default_affect_baseline_battery_stress(),
            baseline_thermal_stress: default_affect_baseline_thermal_stress(),
            baseline_sleepiness: default_affect_baseline_sleepiness(),
            baseline_interaction_load: default_affect_baseline_interaction_load(),
            baseline_trust: default_affect_baseline_trust(),
            baseline_familiarity: default_affect_baseline_familiarity(),
            homeostasis_rate: default_affect_homeostasis_rate(),
            max_delta_per_turn: default_affect_max_delta_per_turn(),
            fatigue_per_turn: default_affect_fatigue_per_turn(),
            long_response_fatigue_delta: default_affect_long_response_fatigue_delta(),
            turn_appraisal_ttl_sec: default_affect_turn_appraisal_ttl(),
            session_appraisal_ttl_sec: default_affect_session_appraisal_ttl(),
            relationship_appraisal_ttl_sec: default_affect_relationship_appraisal_ttl(),
            appraisal_half_life_sec: default_affect_appraisal_half_life(),
            relationship_appraisal_half_life_sec: default_affect_relationship_appraisal_half_life(),
            max_active_appraisal_events: default_affect_max_active_appraisal_events(),
            min_appraisal_confidence: default_affect_min_appraisal_confidence(),
            relationship_persistence_enabled: true,
            prune_persisted_appraisals_on_startup: true,
            persist_appraisal_evidence: true,
            persist_relationship_evidence: false,
            relationship_profile_enabled: true,
            relationship_profile_user_visible: true,
            relationship_profile_store_preferences: true,
            relationship_profile_user_clearable: true,
            relationship_preferences_max_items: default_affect_relationship_preferences_max_items(),
            relationship_summary_max_signals: default_affect_relationship_summary_max_signals(),
            relationship_profile_prune_after_sec: default_affect_relationship_profile_prune_after(),
            max_persisted_evidence_chars: default_affect_max_persisted_evidence_chars(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExpressionPolicySection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub auto_motion_enabled: bool,
    #[serde(default = "default_expression_default_style")]
    pub default_style: String,
    #[serde(default = "default_expression_max_intensity")]
    pub max_expression_intensity: f32,
    #[serde(default = "default_expression_max_motion_intensity")]
    pub max_motion_intensity: f32,
    #[serde(default = "default_expression_max_timeline_actions")]
    pub max_timeline_actions: usize,
    #[serde(default = "default_expression_max_action_duration_ms")]
    pub max_action_duration_ms: u32,
    #[serde(default = "default_expression_allowed_motion_actions")]
    pub allowed_motion_actions: Vec<String>,
    #[serde(default = "default_expression_external_plugin_timeout_ms")]
    pub external_plugin_timeout_ms: u64,
    #[serde(default)]
    pub night_mode_minimal_expression: bool,
}

impl Default for ExpressionPolicySection {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_motion_enabled: true,
            default_style: default_expression_default_style(),
            max_expression_intensity: default_expression_max_intensity(),
            max_motion_intensity: default_expression_max_motion_intensity(),
            max_timeline_actions: default_expression_max_timeline_actions(),
            max_action_duration_ms: default_expression_max_action_duration_ms(),
            allowed_motion_actions: default_expression_allowed_motion_actions(),
            external_plugin_timeout_ms: default_expression_external_plugin_timeout_ms(),
            night_mode_minimal_expression: false,
        }
    }
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
    pub minor_llm_enabled: bool,
    #[serde(default = "default_true")]
    pub major_llm_enabled: bool,
}

impl Default for RoutingPolicySection {
    fn default() -> Self {
        Self {
            model: default_router_model(),
            temperature: default_router_temperature(),
            timeout_sec: default_router_timeout(),
            minor_llm_enabled: true,
            major_llm_enabled: true,
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
    #[serde(default)]
    pub expression_styles: BTreeMap<String, ExpressionStyleSection>,
}

impl Default for ProfilesSection {
    fn default() -> Self {
        Self {
            default_voice: default_voice_profile_id(),
            voice: BTreeMap::new(),
            expression_styles: default_expression_styles(),
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
    /// Provider that owns `provider_specific` interpretation.
    #[serde(default = "default_gpt_sovits_provider")]
    pub provider: String,
    /// Human-facing voice/model label. Adapters may map this to a native voice,
    /// model, reference speaker, or ignore it.
    #[serde(default)]
    pub model_name: Option<String>,
    /// Provider-neutral reference voice/audio anchor.
    #[serde(default)]
    pub reference_voice: Option<String>,
    /// Optional neutral prompt/style hint for providers that use text prompts.
    #[serde(default)]
    pub style_prompt: Option<String>,
    /// BCP-47-ish language tag for the text to synthesize.
    #[serde(default = "default_gpt_sovits_language")]
    pub language: String,
    /// Provider-owned configuration. New TTS providers should add their native
    /// fields here instead of extending the common voice profile.
    #[serde(default)]
    pub provider_specific: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExpressionStyleSection {
    #[serde(default)]
    pub voice_profile: Option<String>,
    #[serde(default = "default_expression_rate")]
    pub rate: f32,
    #[serde(default)]
    pub pitch_shift: f32,
    #[serde(default = "default_expression_energy")]
    pub energy: f32,
    #[serde(default)]
    pub pause_bias: f32,
}

impl Default for ExpressionStyleSection {
    fn default() -> Self {
        Self {
            voice_profile: None,
            rate: default_expression_rate(),
            pitch_shift: 0.0,
            energy: default_expression_energy(),
            pause_bias: 0.0,
        }
    }
}

impl ExpressionStyleSection {
    pub fn prosody(&self) -> crate::expression::ProsodyControls {
        crate::expression::ProsodyControls {
            rate: self.rate.clamp(0.5, 2.0),
            pitch_shift: self.pitch_shift.clamp(-1.0, 1.0),
            energy: self.energy.clamp(0.0, 1.0),
            pause_bias: self.pause_bias.clamp(-1.0, 1.0),
        }
    }
}

impl Default for VoiceProfileSection {
    fn default() -> Self {
        Self {
            provider: default_gpt_sovits_provider(),
            model_name: None,
            reference_voice: None,
            style_prompt: None,
            language: default_gpt_sovits_language(),
            provider_specific: default_gpt_sovits_voice_provider_specific(),
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
    /// Directory containing external expression plugin manifest TOML files.
    /// Each manifest is loaded as a JSON-stdin/stdout dispatcher.
    #[serde(default)]
    pub external_expression_plugins_dir: Option<String>,
    /// Language-scoped lexicons used by the local text classifier appraisal
    /// engine. Keys are BCP-47-ish language tags such as `zh` or `en`.
    #[serde(default)]
    pub appraisal_lexicons: BTreeMap<String, AppraisalLexiconSection>,
    /// Phrase lists used by the deterministic dialogue policy before
    /// expression planning. These are separate from appraisal lexicons because
    /// they classify the assistant's response function, not durable affect.
    #[serde(default)]
    pub dialogue_policy: DialoguePolicyResourceSection,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppraisalLexiconSection {
    #[serde(default)]
    pub metadata: LexiconMetadataSection,
    #[serde(default)]
    pub positive_phrases: Vec<String>,
    #[serde(default)]
    pub correction_or_failure: Vec<String>,
    #[serde(default)]
    pub sarcasm_markers: Vec<String>,
    #[serde(default)]
    pub support_need: Vec<String>,
    #[serde(default)]
    pub safety_salience: Vec<String>,
    #[serde(default)]
    pub minimal_expression: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LexiconMetadataSection {
    #[serde(default)]
    pub source_name: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub redistributable: Option<bool>,
    #[serde(default)]
    pub overlay: bool,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DialoguePolicyResourceSection {
    #[serde(default = "default_dialogue_policy_frustration")]
    pub frustration_phrases: Vec<String>,
    #[serde(default = "default_dialogue_policy_safety")]
    pub safety_phrases: Vec<String>,
    #[serde(default = "default_dialogue_policy_support")]
    pub support_need_phrases: Vec<String>,
    #[serde(default = "default_dialogue_policy_celebration")]
    pub celebration_phrases: Vec<String>,
    #[serde(default = "default_dialogue_policy_explanation")]
    pub explanation_phrases: Vec<String>,
    #[serde(default = "default_dialogue_policy_technical")]
    pub technical_phrases: Vec<String>,
    #[serde(default = "default_dialogue_policy_question_suffixes")]
    pub question_suffixes: Vec<String>,
}

impl Default for DialoguePolicyResourceSection {
    fn default() -> Self {
        Self {
            frustration_phrases: default_dialogue_policy_frustration(),
            safety_phrases: default_dialogue_policy_safety(),
            support_need_phrases: default_dialogue_policy_support(),
            celebration_phrases: default_dialogue_policy_celebration(),
            explanation_phrases: default_dialogue_policy_explanation(),
            technical_phrases: default_dialogue_policy_technical(),
            question_suffixes: default_dialogue_policy_question_suffixes(),
        }
    }
}

fn default_dialogue_policy_frustration() -> Vec<String> {
    strings(&[
        "错了",
        "不对",
        "不是",
        "烦",
        "生气",
        "wrong",
        "incorrect",
        "annoying",
    ])
}

fn default_dialogue_policy_safety() -> Vec<String> {
    strings(&[
        "危险", "安全", "警告", "风险", "小心", "danger", "risk", "unsafe", "warning",
    ])
}

fn default_dialogue_policy_support() -> Vec<String> {
    strings(&[
        "难过", "伤心", "焦虑", "害怕", "崩溃", "sad", "upset", "anxious", "scared",
    ])
}

fn default_dialogue_policy_celebration() -> Vec<String> {
    strings(&[
        "成功",
        "搞定",
        "完成了",
        "太好了",
        "赢了",
        "success",
        "great",
        "awesome",
        "done",
    ])
}

fn default_dialogue_policy_explanation() -> Vec<String> {
    strings(&[
        "为什么",
        "怎么",
        "如何",
        "解释",
        "原理",
        "why",
        "how",
        "explain",
    ])
}

fn default_dialogue_policy_technical() -> Vec<String> {
    strings(&[
        "代码", "技术", "接口", "错误", "编译", "api", "code", "error", "compile",
    ])
}

fn default_dialogue_policy_question_suffixes() -> Vec<String> {
    strings(&["?", "？"])
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
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
fn default_regulation_max_negative_expression() -> f32 {
    0.15
}
fn default_regulation_max_expressed_arousal() -> f32 {
    0.72
}
fn default_regulation_default_max_intensity() -> f32 {
    0.70
}
fn default_regulation_low_trust_threshold() -> f32 {
    0.40
}
fn default_regulation_low_trust_warmth_floor() -> f32 {
    0.62
}
fn default_regulation_low_trust_max_intensity() -> f32 {
    0.36
}
fn default_regulation_low_trust_motion_scale() -> f32 {
    0.45
}
fn default_regulation_fatigue_threshold() -> f32 {
    0.62
}
fn default_regulation_fatigue_max_intensity() -> f32 {
    0.34
}
fn default_regulation_fatigue_motion_scale() -> f32 {
    0.35
}
fn default_regulation_deescalate_arousal_threshold() -> f32 {
    0.65
}
fn default_regulation_deescalated_arousal() -> f32 {
    0.42
}
fn default_regulation_deescalation_warmth_floor() -> f32 {
    0.64
}
fn default_regulation_deescalated_max_intensity() -> f32 {
    0.38
}
fn default_regulation_deescalated_motion_scale() -> f32 {
    0.45
}
fn default_regulation_apology_max_intensity() -> f32 {
    0.32
}
fn default_regulation_apology_motion_scale() -> f32 {
    0.55
}
fn default_regulation_warning_max_intensity() -> f32 {
    0.50
}
fn default_regulation_open_trust_threshold() -> f32 {
    0.62
}
fn default_regulation_open_familiarity_threshold() -> f32 {
    0.28
}
fn default_regulation_open_positive_intensity_scale() -> f32 {
    1.08
}
fn default_regulation_open_positive_motion_scale() -> f32 {
    1.05
}
fn default_affect_baseline_valence() -> f32 {
    0.05
}
fn default_affect_baseline_arousal() -> f32 {
    0.35
}
fn default_affect_baseline_calmness() -> f32 {
    0.65
}
fn default_affect_baseline_engagement() -> f32 {
    0.55
}
fn default_affect_baseline_fatigue() -> f32 {
    0.05
}
fn default_affect_baseline_battery_stress() -> f32 {
    0.0
}
fn default_affect_baseline_thermal_stress() -> f32 {
    0.0
}
fn default_affect_baseline_sleepiness() -> f32 {
    0.05
}
fn default_affect_baseline_interaction_load() -> f32 {
    0.0
}
fn default_affect_baseline_trust() -> f32 {
    0.55
}
fn default_affect_baseline_familiarity() -> f32 {
    0.15
}
fn default_affect_homeostasis_rate() -> f32 {
    0.08
}
fn default_affect_max_delta_per_turn() -> f32 {
    0.18
}
fn default_affect_fatigue_per_turn() -> f32 {
    0.015
}
fn default_affect_long_response_fatigue_delta() -> f32 {
    0.025
}
fn default_affect_turn_appraisal_ttl() -> u64 {
    45
}
fn default_affect_session_appraisal_ttl() -> u64 {
    900
}
fn default_affect_relationship_appraisal_ttl() -> u64 {
    7_776_000
}
fn default_affect_appraisal_half_life() -> u64 {
    180
}
fn default_affect_relationship_appraisal_half_life() -> u64 {
    604_800
}
fn default_affect_max_active_appraisal_events() -> usize {
    64
}
fn default_affect_min_appraisal_confidence() -> f32 {
    0.6
}
fn default_affect_max_persisted_evidence_chars() -> usize {
    160
}
fn default_affect_relationship_preferences_max_items() -> usize {
    24
}
fn default_affect_relationship_summary_max_signals() -> usize {
    8
}
fn default_affect_relationship_profile_prune_after() -> u64 {
    15_552_000
}
fn default_expression_default_style() -> String {
    "gentle".to_string()
}
fn default_expression_max_intensity() -> f32 {
    0.7
}
fn default_expression_max_motion_intensity() -> f32 {
    0.45
}
fn default_expression_max_timeline_actions() -> usize {
    8
}
fn default_expression_max_action_duration_ms() -> u32 {
    2_500
}
fn default_expression_allowed_motion_actions() -> Vec<String> {
    ["look_at_user", "small_nod", "small_bow"]
        .into_iter()
        .map(str::to_string)
        .collect()
}
fn default_expression_external_plugin_timeout_ms() -> u64 {
    750
}
fn default_expression_rate() -> f32 {
    1.0
}
fn default_expression_energy() -> f32 {
    0.5
}
fn default_expression_styles() -> BTreeMap<String, ExpressionStyleSection> {
    [
        (
            "neutral".to_string(),
            ExpressionStyleSection {
                voice_profile: Some("default".to_string()),
                rate: 1.0,
                pitch_shift: 0.0,
                energy: 0.5,
                pause_bias: 0.0,
            },
        ),
        (
            "gentle".to_string(),
            ExpressionStyleSection {
                voice_profile: Some("default".to_string()),
                rate: 0.95,
                pitch_shift: 0.0,
                energy: 0.42,
                pause_bias: 0.12,
            },
        ),
        (
            "calm".to_string(),
            ExpressionStyleSection {
                voice_profile: Some("default".to_string()),
                rate: 0.92,
                pitch_shift: 0.0,
                energy: 0.38,
                pause_bias: 0.16,
            },
        ),
        (
            "cheerful".to_string(),
            ExpressionStyleSection {
                voice_profile: Some("default".to_string()),
                rate: 1.04,
                pitch_shift: 0.04,
                energy: 0.62,
                pause_bias: 0.0,
            },
        ),
        (
            "serious".to_string(),
            ExpressionStyleSection {
                voice_profile: Some("default".to_string()),
                rate: 0.96,
                pitch_shift: -0.02,
                energy: 0.44,
                pause_bias: 0.08,
            },
        ),
    ]
    .into()
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
fn default_gpt_sovits_voice_provider_specific() -> BTreeMap<String, serde_json::Value> {
    use serde_json::json;

    BTreeMap::from([
        ("gpt_weights_path".to_string(), json!("")),
        ("sovits_weights_path".to_string(), json!("")),
        ("aux_ref_audio_paths".to_string(), json!([])),
        (
            "prompt_language".to_string(),
            json!(default_gpt_sovits_prompt_language()),
        ),
        (
            "text_split_method".to_string(),
            json!(default_gpt_sovits_text_split_method()),
        ),
        (
            "batch_size".to_string(),
            json!(default_gpt_sovits_batch_size()),
        ),
        (
            "batch_threshold".to_string(),
            json!(default_gpt_sovits_batch_threshold()),
        ),
        ("split_bucket".to_string(), json!(true)),
        (
            "speed_factor".to_string(),
            json!(default_gpt_sovits_speed_factor()),
        ),
        (
            "fragment_interval".to_string(),
            json!(default_gpt_sovits_fragment_interval()),
        ),
        ("seed".to_string(), json!(default_gpt_sovits_seed())),
        ("parallel_infer".to_string(), json!(true)),
        (
            "repetition_penalty".to_string(),
            json!(default_gpt_sovits_repetition_penalty()),
        ),
        (
            "sample_steps".to_string(),
            json!(default_gpt_sovits_sample_steps()),
        ),
        ("super_sampling".to_string(), json!(false)),
        (
            "overlap_length".to_string(),
            json!(default_gpt_sovits_overlap_length()),
        ),
        (
            "min_chunk_length".to_string(),
            json!(default_gpt_sovits_min_chunk_length()),
        ),
    ])
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
    #[serde(default)]
    pub capture_mode: MemoryCaptureMode,
    #[serde(default = "default_memory_capture_timeout_sec")]
    pub capture_timeout_sec: u64,
    #[serde(default = "default_memory_capture_min_confidence")]
    pub capture_min_confidence: f32,
    #[serde(default = "default_memory_sensitive_filter_enabled")]
    pub sensitive_filter_enabled: bool,
    #[serde(default = "default_memory_retrieval_layers")]
    pub retrieval_layers: Vec<String>,
    #[serde(default = "default_memory_max_injected_chars")]
    pub max_injected_memory_chars: usize,
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
            capture_mode: MemoryCaptureMode::default(),
            capture_timeout_sec: default_memory_capture_timeout_sec(),
            capture_min_confidence: default_memory_capture_min_confidence(),
            sensitive_filter_enabled: default_memory_sensitive_filter_enabled(),
            retrieval_layers: default_memory_retrieval_layers(),
            max_injected_memory_chars: default_memory_max_injected_chars(),
            max_retrieval_results: 10,
            similarity_threshold: 0.3,
            health_check_attempts: default_memory_health_check_attempts(),
            health_check_interval_ms: default_memory_health_check_interval_ms(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCaptureMode {
    None,
    ExplicitToolOnly,
    LlmSelected,
    ProviderInferAll,
}

impl Default for MemoryCaptureMode {
    fn default() -> Self {
        Self::LlmSelected
    }
}

fn default_memory_capture_timeout_sec() -> u64 {
    3
}

fn default_memory_capture_min_confidence() -> f32 {
    0.65
}

fn default_memory_sensitive_filter_enabled() -> bool {
    true
}

fn default_memory_retrieval_layers() -> Vec<String> {
    vec![
        "core".to_string(),
        "preference".to_string(),
        "relationship".to_string(),
    ]
}

fn default_memory_max_injected_chars() -> usize {
    1200
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
    #[serde(default = "default_true")]
    pub indexed_search_enabled: bool,
    #[serde(default = "default_tool_indexed_search_min_similarity")]
    pub indexed_search_min_similarity: f32,
    #[serde(default = "default_true")]
    pub indexed_search_refresh_missing_embeddings: bool,
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
            indexed_search_enabled: true,
            indexed_search_min_similarity: default_tool_indexed_search_min_similarity(),
            indexed_search_refresh_missing_embeddings: true,
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
fn default_tool_indexed_search_min_similarity() -> f32 {
    0.30
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
