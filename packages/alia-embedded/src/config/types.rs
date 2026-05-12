use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level configuration, deserialized from `config/embedded.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedConfig {
    pub modules: ModulesConfig,
    pub identity: IdentityConfig,
    pub wakeword: WakewordConfig,
    #[serde(default)]
    pub edge_stt: EdgeSttConfig,
    pub audio: AudioConfig,
    pub discovery: DiscoveryConfig,
    #[serde(default)]
    pub process: Vec<ProcessConfig>,
    #[serde(default)]
    pub direct_response: Vec<DirectResponseConfig>,
    #[serde(skip)]
    pub config_path: PathBuf,
    #[serde(skip)]
    pub provisioning_mode: bool,
}

/// Module slot selection: which implementation to use for each pluggable slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModulesConfig {
    #[serde(default = "default_wakeword_module")]
    pub wakeword: String,
    #[serde(default = "default_edge_stt_module")]
    pub edge_stt: String,
    #[serde(default = "default_direct_response_module")]
    pub direct_response: String,
    #[serde(default = "default_audio_capture_module")]
    pub audio_capture: String,
    #[serde(default = "default_audio_playback_module")]
    pub audio_playback: String,
    #[serde(default = "default_mixer_module")]
    pub mixer: String,
    #[serde(default = "default_face_tracker_module")]
    pub face_tracker: String,
    #[serde(default = "default_discovery_module")]
    pub discovery: String,
}

impl Default for ModulesConfig {
    fn default() -> Self {
        Self {
            wakeword: default_wakeword_module(),
            edge_stt: default_edge_stt_module(),
            direct_response: default_direct_response_module(),
            audio_capture: default_audio_capture_module(),
            audio_playback: default_audio_playback_module(),
            mixer: default_mixer_module(),
            face_tracker: default_face_tracker_module(),
            discovery: default_discovery_module(),
        }
    }
}

fn default_wakeword_module() -> String {
    "sherpa_onnx".to_string()
}
fn default_edge_stt_module() -> String {
    "sherpa_onnx".to_string()
}
fn default_direct_response_module() -> String {
    "keyword".to_string()
}
fn default_audio_capture_module() -> String {
    "cpal".to_string()
}
fn default_audio_playback_module() -> String {
    "cpal".to_string()
}
fn default_mixer_module() -> String {
    "priority".to_string()
}
fn default_face_tracker_module() -> String {
    "mediapipe".to_string()
}
fn default_discovery_module() -> String {
    "mdns".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    pub device_id: String,
    pub device_name: String,
    pub keypair_path: PathBuf,
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            device_name: "Alia Accessory".to_string(),
            keypair_path: PathBuf::from("device.key"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakewordConfig {
    pub engine: String,
    pub model_path: PathBuf,
    #[serde(default = "default_kws_encoder_path")]
    pub encoder_path: PathBuf,
    #[serde(default = "default_kws_decoder_path")]
    pub decoder_path: PathBuf,
    #[serde(default = "default_kws_joiner_path")]
    pub joiner_path: PathBuf,
    #[serde(default = "default_kws_tokens_path")]
    pub tokens_path: PathBuf,
    #[serde(default = "default_kws_keywords_file")]
    pub keywords_file: PathBuf,
    pub keywords: Vec<String>,
    #[serde(default = "default_sensitivity")]
    pub sensitivity: f32,
    #[serde(default = "default_sherpa_provider")]
    pub provider: String,
    #[serde(default = "default_kws_num_threads")]
    pub num_threads: i32,
    #[serde(default)]
    pub debug: bool,
}

impl Default for WakewordConfig {
    fn default() -> Self {
        Self {
            engine: "sherpa-onnx".to_string(),
            model_path: PathBuf::from("models/hey_alia.onnx"),
            encoder_path: default_kws_encoder_path(),
            decoder_path: default_kws_decoder_path(),
            joiner_path: default_kws_joiner_path(),
            tokens_path: default_kws_tokens_path(),
            keywords_file: default_kws_keywords_file(),
            keywords: vec!["你好阿利亚".to_string(), "Hey Alia".to_string()],
            sensitivity: default_sensitivity(),
            provider: default_sherpa_provider(),
            num_threads: default_kws_num_threads(),
            debug: false,
        }
    }
}

fn default_kws_encoder_path() -> PathBuf {
    PathBuf::from("models/sherpa-kws/encoder.onnx")
}
fn default_kws_decoder_path() -> PathBuf {
    PathBuf::from("models/sherpa-kws/decoder.onnx")
}
fn default_kws_joiner_path() -> PathBuf {
    PathBuf::from("models/sherpa-kws/joiner.onnx")
}
fn default_kws_tokens_path() -> PathBuf {
    PathBuf::from("models/sherpa-kws/tokens.txt")
}
fn default_kws_keywords_file() -> PathBuf {
    PathBuf::from("models/sherpa-kws/keywords.txt")
}
fn default_sensitivity() -> f32 {
    0.6
}
fn default_sherpa_provider() -> String {
    "cpu".to_string()
}
fn default_kws_num_threads() -> i32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeSttConfig {
    #[serde(default = "default_edge_stt_model_path")]
    pub model_path: PathBuf,
    #[serde(default = "default_edge_stt_tokens_path")]
    pub tokens_path: PathBuf,
    #[serde(default = "default_edge_stt_language")]
    pub language: String,
    #[serde(default = "default_sherpa_provider")]
    pub provider: String,
    #[serde(default = "default_edge_stt_num_threads")]
    pub num_threads: i32,
    #[serde(default = "default_true")]
    pub use_itn: bool,
    #[serde(default = "default_edge_stt_max_duration_ms")]
    pub max_duration_ms: u32,
    #[serde(default = "default_post_wake_capture_ms")]
    pub post_wake_capture_ms: u32,
    #[serde(default)]
    pub debug: bool,
}

impl Default for EdgeSttConfig {
    fn default() -> Self {
        Self {
            model_path: default_edge_stt_model_path(),
            tokens_path: default_edge_stt_tokens_path(),
            language: default_edge_stt_language(),
            provider: default_sherpa_provider(),
            num_threads: default_edge_stt_num_threads(),
            use_itn: true,
            max_duration_ms: default_edge_stt_max_duration_ms(),
            post_wake_capture_ms: default_post_wake_capture_ms(),
            debug: false,
        }
    }
}

fn default_edge_stt_model_path() -> PathBuf {
    PathBuf::from("models/sense-voice/model.int8.onnx")
}
fn default_edge_stt_tokens_path() -> PathBuf {
    PathBuf::from("models/sense-voice/tokens.txt")
}
fn default_edge_stt_language() -> String {
    "auto".to_string()
}
fn default_edge_stt_num_threads() -> i32 {
    2
}
fn default_edge_stt_max_duration_ms() -> u32 {
    5_000
}
fn default_post_wake_capture_ms() -> u32 {
    2_500
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    // Capture
    #[serde(default = "default_device")]
    pub capture_device: String,
    #[serde(default = "default_capture_sample_rate")]
    pub capture_sample_rate: u32,
    #[serde(default = "default_channels")]
    pub capture_channels: u16,

    // Playback
    #[serde(default = "default_device")]
    pub playback_device: String,
    #[serde(default = "default_playback_sample_rate")]
    pub playback_sample_rate: u32,
    #[serde(default = "default_channels")]
    pub playback_channels: u16,
    #[serde(default = "default_playback_buffer_ms")]
    pub playback_buffer_ms: u32,

    // Opus encoding
    #[serde(default = "default_opus_bitrate")]
    pub opus_bitrate: u32,
    #[serde(default = "default_opus_frame_ms")]
    pub opus_frame_ms: u32,

    // Flow control
    #[serde(default = "default_upload_buffer_frames")]
    pub upload_buffer_frames: u32,
    #[serde(default = "default_jitter_buffer_ms")]
    pub jitter_buffer_ms: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            capture_device: default_device(),
            capture_sample_rate: default_capture_sample_rate(),
            capture_channels: default_channels(),
            playback_device: default_device(),
            playback_sample_rate: default_playback_sample_rate(),
            playback_channels: default_channels(),
            playback_buffer_ms: default_playback_buffer_ms(),
            opus_bitrate: default_opus_bitrate(),
            opus_frame_ms: default_opus_frame_ms(),
            upload_buffer_frames: default_upload_buffer_frames(),
            jitter_buffer_ms: default_jitter_buffer_ms(),
        }
    }
}

fn default_device() -> String {
    "default".to_string()
}
fn default_capture_sample_rate() -> u32 {
    16000
}
fn default_playback_sample_rate() -> u32 {
    24000
}
fn default_channels() -> u16 {
    1
}
fn default_playback_buffer_ms() -> u32 {
    60
}
fn default_opus_bitrate() -> u32 {
    24000
}
fn default_opus_frame_ms() -> u32 {
    20
}
fn default_upload_buffer_frames() -> u32 {
    5
}
fn default_jitter_buffer_ms() -> u32 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    #[serde(default = "default_true")]
    pub mdns_enabled: bool,
    #[serde(default = "default_mdns_service")]
    pub mdns_service_name: String,
    #[serde(default)]
    pub mdns_instance_name: Option<String>,

    #[serde(default)]
    pub ble_provision_enabled: bool,
    #[serde(default = "default_ble_timeout")]
    pub ble_provision_timeout_sec: u64,

    #[serde(default = "default_true")]
    pub require_physical_confirm: bool,
    #[serde(default = "default_max_bind_attempts")]
    pub max_bind_attempts: u32,
    #[serde(default = "default_lockout_duration")]
    pub lockout_duration_sec: u64,

    #[serde(default = "default_reconnect_interval")]
    pub reconnect_interval_sec: u64,
    #[serde(default = "default_reconnect_max_backoff")]
    pub reconnect_max_backoff_sec: u64,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            mdns_enabled: default_true(),
            mdns_service_name: default_mdns_service(),
            mdns_instance_name: None,
            ble_provision_enabled: true,
            ble_provision_timeout_sec: default_ble_timeout(),
            require_physical_confirm: default_true(),
            max_bind_attempts: default_max_bind_attempts(),
            lockout_duration_sec: default_lockout_duration(),
            reconnect_interval_sec: default_reconnect_interval(),
            reconnect_max_backoff_sec: default_reconnect_max_backoff(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_mdns_service() -> String {
    "_alia-embedded._tcp".to_string()
}
fn default_ble_timeout() -> u64 {
    300
}
fn default_max_bind_attempts() -> u32 {
    3
}
fn default_lockout_duration() -> u64 {
    600
}
fn default_reconnect_interval() -> u64 {
    5
}
fn default_reconnect_max_backoff() -> u64 {
    60
}

/// Configuration for a managed Python process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub auto_restart: bool,
}

/// Configuration for a direct (keyword → local audio) response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectResponseConfig {
    pub keywords: Vec<String>,
    pub audio: PathBuf,
    #[serde(default)]
    pub action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeConfigMeta {
    #[serde(default)]
    pub provisioned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmbeddedRuntimeSettings {
    #[serde(default)]
    pub runtime: RuntimeConfigMeta,
    #[serde(default)]
    pub identity: IdentityConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub discovery: DiscoveryConfig,
}

impl Default for EmbeddedConfig {
    fn default() -> Self {
        Self {
            modules: ModulesConfig::default(),
            identity: IdentityConfig::default(),
            wakeword: WakewordConfig::default(),
            edge_stt: EdgeSttConfig::default(),
            audio: AudioConfig::default(),
            discovery: DiscoveryConfig::default(),
            process: Vec::new(),
            direct_response: Vec::new(),
            config_path: PathBuf::new(),
            provisioning_mode: true,
        }
    }
}

impl EmbeddedConfig {
    pub fn apply_runtime_settings(&mut self, runtime: EmbeddedRuntimeSettings) {
        self.identity = runtime.identity;
        self.audio = runtime.audio;
        self.discovery = runtime.discovery;
        self.provisioning_mode = !runtime.runtime.provisioned;
    }

    pub fn runtime_settings(&self) -> EmbeddedRuntimeSettings {
        EmbeddedRuntimeSettings {
            runtime: RuntimeConfigMeta {
                provisioned: !self.provisioning_mode,
            },
            identity: self.identity.clone(),
            audio: self.audio.clone(),
            discovery: self.discovery.clone(),
        }
    }
}
