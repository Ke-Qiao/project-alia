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
    #[serde(default)]
    pub vad: VadConfig,
    #[serde(default)]
    pub interrupt: InterruptConfig,
    pub audio: AudioConfig,
    #[serde(default)]
    pub audio_debug: AudioDebugConfig,
    pub discovery: DiscoveryConfig,
    #[serde(default)]
    pub direct_response_defaults: DirectResponseDefaultsConfig,
    #[serde(default)]
    pub dev: DevConfig,
    #[serde(default)]
    pub process: Vec<ProcessConfig>,
    #[serde(default)]
    pub direct_response: Vec<DirectResponseConfig>,
    #[serde(skip)]
    pub config_path: PathBuf,
    #[serde(skip)]
    pub provisioning_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DevConfig {
    #[serde(default)]
    pub skip_binding: bool,
    #[serde(default)]
    pub server_id: String,
    #[serde(default)]
    pub server_name: String,
    #[serde(default)]
    pub server_endpoint: String,
}

/// Module slot selection: which implementation to use for each pluggable slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModulesConfig {
    #[serde(default = "default_wakeword_module")]
    pub wakeword: String,
    #[serde(default = "default_edge_stt_module")]
    pub edge_stt: String,
    #[serde(default = "default_vad_module")]
    pub vad: String,
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
            vad: default_vad_module(),
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
fn default_vad_module() -> String {
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
    #[serde(default = "default_kws_keywords_score")]
    pub keywords_score: f32,
    #[serde(default = "default_kws_keywords_threshold")]
    pub keywords_threshold: f32,
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
            keywords_score: default_kws_keywords_score(),
            keywords_threshold: default_kws_keywords_threshold(),
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
fn default_kws_keywords_score() -> f32 {
    1.0
}
fn default_kws_keywords_threshold() -> f32 {
    0.25
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
    #[serde(default = "default_post_wake_min_capture_ms")]
    pub post_wake_min_capture_ms: u32,
    #[serde(default = "default_post_wake_capture_ms")]
    pub post_wake_capture_ms: u32,
    #[serde(default = "default_post_wake_response_delay_ms")]
    pub post_wake_response_delay_ms: u32,
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
            post_wake_min_capture_ms: default_post_wake_min_capture_ms(),
            post_wake_capture_ms: default_post_wake_capture_ms(),
            post_wake_response_delay_ms: default_post_wake_response_delay_ms(),
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
    15_000
}
fn default_post_wake_min_capture_ms() -> u32 {
    1_800
}
fn default_post_wake_capture_ms() -> u32 {
    2_500
}
fn default_post_wake_response_delay_ms() -> u32 {
    700
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VadConfig {
    #[serde(default = "default_vad_model_path")]
    pub model_path: PathBuf,
    #[serde(default = "default_vad_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_vad_threshold")]
    pub threshold: f32,
    #[serde(default = "default_vad_min_silence_duration_ms")]
    pub min_silence_duration_ms: u32,
    #[serde(default = "default_vad_min_speech_duration_ms")]
    pub min_speech_duration_ms: u32,
    #[serde(default = "default_vad_window_size")]
    pub window_size: i32,
    #[serde(default = "default_vad_max_speech_duration_ms")]
    pub max_speech_duration_ms: u32,
    #[serde(default = "default_vad_buffer_size_sec")]
    pub buffer_size_sec: f32,
    #[serde(default = "default_sherpa_provider")]
    pub provider: String,
    #[serde(default = "default_vad_num_threads")]
    pub num_threads: i32,
    #[serde(default)]
    pub debug: bool,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            model_path: default_vad_model_path(),
            sample_rate: default_vad_sample_rate(),
            threshold: default_vad_threshold(),
            min_silence_duration_ms: default_vad_min_silence_duration_ms(),
            min_speech_duration_ms: default_vad_min_speech_duration_ms(),
            window_size: default_vad_window_size(),
            max_speech_duration_ms: default_vad_max_speech_duration_ms(),
            buffer_size_sec: default_vad_buffer_size_sec(),
            provider: default_sherpa_provider(),
            num_threads: default_vad_num_threads(),
            debug: false,
        }
    }
}

fn default_vad_model_path() -> PathBuf {
    PathBuf::from("models/silero-vad/silero_vad.onnx")
}
fn default_vad_sample_rate() -> u32 {
    16_000
}
fn default_vad_threshold() -> f32 {
    0.5
}
fn default_vad_min_silence_duration_ms() -> u32 {
    700
}
fn default_vad_min_speech_duration_ms() -> u32 {
    250
}
fn default_vad_window_size() -> i32 {
    512
}
fn default_vad_max_speech_duration_ms() -> u32 {
    15_000
}
fn default_vad_buffer_size_sec() -> f32 {
    10.0
}
fn default_vad_num_threads() -> i32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptConfig {
    #[serde(default = "default_true")]
    pub interrupt_on_wakeword_during_playback: bool,
    #[serde(default = "default_true")]
    pub interrupt_skip_direct_response: bool,
    #[serde(default = "default_interrupt_debounce_ms")]
    pub interrupt_debounce_ms: u32,
}

impl Default for InterruptConfig {
    fn default() -> Self {
        Self {
            interrupt_on_wakeword_during_playback: true,
            interrupt_skip_direct_response: true,
            interrupt_debounce_ms: default_interrupt_debounce_ms(),
        }
    }
}

fn default_interrupt_debounce_ms() -> u32 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(default)]
    pub capture_backend: AudioBackend,
    // Capture
    #[serde(default = "default_device")]
    pub capture_device: String,
    #[serde(default = "default_capture_sample_rate")]
    pub capture_sample_rate: u32,
    #[serde(default = "default_channels")]
    pub capture_channels: u16,
    #[serde(default = "default_audio_volume")]
    pub capture_volume: f32,

    #[serde(default)]
    pub playback_backend: AudioBackend,
    // Playback
    #[serde(default = "default_device")]
    pub playback_device: String,
    #[serde(default = "default_playback_sample_rate")]
    pub playback_sample_rate: u32,
    #[serde(default = "default_channels")]
    pub playback_channels: u16,
    #[serde(default = "default_audio_volume")]
    pub playback_volume: f32,
    #[serde(default = "default_playback_buffer_ms")]
    pub playback_buffer_ms: u32,
    #[serde(default = "default_playback_target_buffer_ms")]
    pub playback_target_buffer_ms: u32,
    #[serde(default = "default_playback_max_buffer_ms")]
    pub playback_max_buffer_ms: u32,
    #[serde(default = "default_playback_command_queue_capacity")]
    pub playback_command_queue_capacity: usize,
    #[serde(default = "default_alsa_playback_follow_source_rate")]
    pub alsa_playback_follow_source_rate: bool,
    #[serde(default)]
    pub alsa_playback_lock_to_capture_rate: bool,
    #[serde(default)]
    pub alsa_capture_s32_alignment: AlsaCaptureS32Alignment,

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
            capture_backend: AudioBackend::default(),
            capture_device: default_device(),
            capture_sample_rate: default_capture_sample_rate(),
            capture_channels: default_channels(),
            capture_volume: default_audio_volume(),
            playback_backend: AudioBackend::default(),
            playback_device: default_device(),
            playback_sample_rate: default_playback_sample_rate(),
            playback_channels: default_channels(),
            playback_volume: default_audio_volume(),
            playback_buffer_ms: default_playback_buffer_ms(),
            playback_target_buffer_ms: default_playback_target_buffer_ms(),
            playback_max_buffer_ms: default_playback_max_buffer_ms(),
            playback_command_queue_capacity: default_playback_command_queue_capacity(),
            alsa_playback_follow_source_rate: default_alsa_playback_follow_source_rate(),
            alsa_playback_lock_to_capture_rate: false,
            alsa_capture_s32_alignment: AlsaCaptureS32Alignment::default(),
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
fn default_audio_volume() -> f32 {
    1.0
}
fn default_playback_buffer_ms() -> u32 {
    60
}
fn default_playback_target_buffer_ms() -> u32 {
    300
}
fn default_playback_max_buffer_ms() -> u32 {
    2_000
}
fn default_playback_command_queue_capacity() -> usize {
    8
}
fn default_alsa_playback_follow_source_rate() -> bool {
    true
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
pub struct AudioDebugConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_audio_debug_dir")]
    pub dir: PathBuf,
    #[serde(default = "default_true")]
    pub record_capture_stream: bool,
    #[serde(default = "default_true")]
    pub record_wake_capture: bool,
    #[serde(default = "default_true")]
    pub record_playback: bool,
    #[serde(default = "default_audio_debug_capture_segment_ms")]
    pub capture_segment_ms: u32,
}

impl Default for AudioDebugConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dir: default_audio_debug_dir(),
            record_capture_stream: true,
            record_wake_capture: true,
            record_playback: true,
            capture_segment_ms: default_audio_debug_capture_segment_ms(),
        }
    }
}

fn default_audio_debug_dir() -> PathBuf {
    PathBuf::from("run/audio-debug")
}

fn default_audio_debug_capture_segment_ms() -> u32 {
    10_000
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioBackend {
    #[default]
    Auto,
    Cpal,
    Alsa,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlsaCaptureS32Alignment {
    #[default]
    Auto,
    LeftJustified,
    RightJustified,
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
    #[serde(default)]
    pub slug: String,
    pub keywords: Vec<String>,
    pub audio: PathBuf,
    #[serde(default)]
    pub audio_variants: Vec<DirectResponseAudioConfig>,
    #[serde(default)]
    pub action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectResponseAudioConfig {
    pub audio: PathBuf,
    #[serde(default = "default_direct_response_weight")]
    pub weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectResponseDefaultsConfig {
    #[serde(default = "default_direct_response_audio_dir")]
    pub audio_dir: PathBuf,
}

impl Default for DirectResponseDefaultsConfig {
    fn default() -> Self {
        Self {
            audio_dir: default_direct_response_audio_dir(),
        }
    }
}

fn default_direct_response_audio_dir() -> PathBuf {
    PathBuf::from("run/direct-responses")
}

fn default_direct_response_weight() -> f32 {
    1.0
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
    pub wakeword: WakewordConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub audio_debug: AudioDebugConfig,
    #[serde(default)]
    pub edge_stt: EdgeSttConfig,
    #[serde(default)]
    pub vad: VadConfig,
    #[serde(default)]
    pub interrupt: InterruptConfig,
    #[serde(default)]
    pub discovery: DiscoveryConfig,
    #[serde(default)]
    pub direct_response_defaults: DirectResponseDefaultsConfig,
    #[serde(default)]
    pub dev: DevConfig,
}

impl Default for EmbeddedConfig {
    fn default() -> Self {
        Self {
            modules: ModulesConfig::default(),
            identity: IdentityConfig::default(),
            wakeword: WakewordConfig::default(),
            edge_stt: EdgeSttConfig::default(),
            vad: VadConfig::default(),
            interrupt: InterruptConfig::default(),
            audio: AudioConfig::default(),
            audio_debug: AudioDebugConfig::default(),
            discovery: DiscoveryConfig::default(),
            direct_response_defaults: DirectResponseDefaultsConfig::default(),
            dev: DevConfig::default(),
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
        self.wakeword = runtime.wakeword;
        self.audio = runtime.audio;
        self.audio_debug = runtime.audio_debug;
        self.edge_stt = runtime.edge_stt;
        self.vad = runtime.vad;
        self.interrupt = runtime.interrupt;
        self.discovery = runtime.discovery;
        self.direct_response_defaults = runtime.direct_response_defaults;
        self.dev = runtime.dev;
        self.provisioning_mode = !runtime.runtime.provisioned;
    }

    pub fn runtime_settings(&self) -> EmbeddedRuntimeSettings {
        EmbeddedRuntimeSettings {
            runtime: RuntimeConfigMeta {
                provisioned: !self.provisioning_mode,
            },
            identity: self.identity.clone(),
            wakeword: self.wakeword.clone(),
            audio: self.audio.clone(),
            audio_debug: self.audio_debug.clone(),
            edge_stt: self.edge_stt.clone(),
            vad: self.vad.clone(),
            interrupt: self.interrupt.clone(),
            discovery: self.discovery.clone(),
            direct_response_defaults: self.direct_response_defaults.clone(),
            dev: self.dev.clone(),
        }
    }
}
