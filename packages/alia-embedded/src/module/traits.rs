use anyhow::Result;
use std::collections::HashMap;

use super::context::AudioBus;
use super::lifecycle::ModuleLifecycle;

// ─── Wake Word Engine ────────────────────────────────────────────────────────

/// Detection event from the wake word engine.
#[derive(Debug, Clone)]
pub struct WakeWordEvent {
    pub keyword: String,
    pub confidence: f32,
    pub timestamp_us: u64,
}

/// Wake word detection engine (slot: `wakeword`).
///
/// Real-time requirement: `detect()` is synchronous and must complete in < 5ms
/// since it runs in the audio capture callback path.
pub trait WakeWordEngine: ModuleLifecycle {
    /// Feed PCM frames, returns detection event if wake word is found.
    fn detect(&self, pcm: &[i16]) -> Option<WakeWordEvent>;
}

// ─── Edge STT Engine ─────────────────────────────────────────────────────────

/// Lightweight on-device STT engine (slot: `edge_stt`).
///
/// Used for short utterances (< 5s) after wake word detection.
/// Returns `None` if confidence is insufficient (fall back to server).
#[tonic::async_trait]
pub trait EdgeSttEngine: ModuleLifecycle {
    /// Transcribe a short audio segment locally.
    async fn transcribe_short(&self, pcm: &[i16], max_duration_ms: u32) -> Result<Option<String>>;
}

// ─── Voice Activity Detection ────────────────────────────────────────────────

/// Streaming voice activity state for post-wake command endpointing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceActivityState {
    /// VAD is not available; callers should use fixed-duration fallback.
    Unavailable,
    /// No active speech is currently detected.
    Silence,
    /// Speech is currently active.
    Speech,
    /// A speech segment has ended and is ready for downstream processing.
    Finished,
}

/// Voice activity detector (slot: `vad`).
///
/// Used only for endpointing short post-wake captures. It does not replace
/// wake-word detection or STT.
pub trait VoiceActivityEngine: ModuleLifecycle {
    /// Reset detector state before a new post-wake command capture.
    fn reset(&self) -> Result<()>;

    /// Feed PCM frames and return the latest endpointing state.
    fn accept(&self, pcm: &[i16], sample_rate: u32) -> Result<VoiceActivityState>;

    /// Whether this implementation can currently perform VAD.
    fn is_available(&self) -> bool {
        true
    }
}

// ─── Direct Response Engine ──────────────────────────────────────────────────

/// Action to execute when a keyword is matched locally.
#[derive(Debug, Clone)]
pub struct DirectResponseAction {
    pub audio_path: Option<String>,
    pub action: Option<String>,
}

/// Keyword-based fast response engine (slot: `direct_response`).
///
/// Matches recognized text against local keyword rules; if matched,
/// bypasses the server LLM path for sub-600ms response.
pub trait DirectResponseEngine: ModuleLifecycle {
    /// Match input text, returning an action if a keyword hits.
    fn match_keyword(&self, text: &str) -> Option<DirectResponseAction>;

    /// Resolve a named system response such as `wakeword`, `fallback`, or
    /// `listening_continue`. Missing responses are expected and should be
    /// handled by callers without failing the speech pipeline.
    fn match_action(&self, _action: &str) -> Option<DirectResponseAction> {
        None
    }
}

// ─── Audio Capture Backend ───────────────────────────────────────────────────

/// Microphone audio capture backend (slot: `audio_capture`).
///
/// Real-time path—does not use async. PCM data is pushed to the shared AudioBus.
pub trait AudioCaptureBackend: ModuleLifecycle {
    /// Start capturing audio, writing PCM frames to the bus.
    fn start_capture(&self, bus: &AudioBus) -> Result<()>;

    /// Stop capturing.
    fn stop_capture(&self) -> Result<()>;

    /// Whether the capture device is currently available (hot-plug detection).
    fn is_available(&self) -> bool;
}

// ─── Audio Playback Backend ──────────────────────────────────────────────────

/// Audio playback backend (slot: `audio_playback`).
pub trait AudioPlaybackBackend: ModuleLifecycle {
    /// Play decoded PCM audio.
    fn play(&self, pcm: &[i16], sample_rate: u32) -> Result<()>;

    /// Stop current playback.
    fn stop_playback(&self) -> Result<()>;

    /// Start a new scoped output and return its process-local cancellation token.
    fn begin_scoped_playback(&self) -> Result<u64> {
        anyhow::bail!("scoped playback is not supported by this backend")
    }

    /// Atomically enqueue only while the expected local generation is current.
    /// False means cancelled; never relabel old media with a newer generation.
    fn play_if_current(&self, _pcm: &[i16], _sample_rate: u32, _generation: u64) -> Result<bool> {
        anyhow::bail!("scoped playback is not supported by this backend")
    }

    /// A delayed cancellation must not stop a newer output.
    fn stop_if_current(&self, _generation: u64) -> Result<bool> {
        anyhow::bail!("scoped playback is not supported by this backend")
    }

    fn is_playback_current(&self, _generation: u64) -> bool {
        false
    }

    /// Whether the playback device is currently available.
    fn is_available(&self) -> bool;
}

// ─── Audio Mixer ─────────────────────────────────────────────────────────────

/// Audio source identifier for mixer submissions.
#[derive(Debug, Clone)]
pub struct AudioSource {
    pub id: String,
    pub pcm: Vec<i16>,
    pub sample_rate: u32,
}

/// Priority levels for audio mixing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AudioPriority {
    /// Background music (lowest)
    Background = 0,
    /// Notifications / alerts
    Notification = 1,
    /// Direct response local audio
    DirectResponse = 2,
    /// TTS conversation response (highest)
    Tts = 3,
}

/// Multi-source priority audio mixer (slot: `mixer`).
pub trait AudioMixer: ModuleLifecycle {
    /// Submit audio to a priority channel.
    fn submit(&self, source: AudioSource, priority: AudioPriority) -> Result<()>;

    /// Interrupt all sources below the given priority.
    fn interrupt_below(&self, priority: AudioPriority) -> Result<()>;
}

// ─── Face Tracker ────────────────────────────────────────────────────────────

/// A single face detection result.
#[derive(Debug, Clone)]
pub struct FaceDetection {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub confidence: f32,
}

/// Face tracking engine (slot: `face_tracker`).
#[tonic::async_trait]
pub trait FaceTracker: ModuleLifecycle {
    /// Get the latest face detection results.
    async fn get_faces(&self) -> Result<Vec<FaceDetection>>;

    /// Whether the tracker is currently running.
    fn is_tracking(&self) -> bool;
}

// ─── Discovery Backend ───────────────────────────────────────────────────────

/// Device info advertised via the discovery backend.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub version: String,
    pub port: u16,
    pub endpoints: Vec<DiscoveryEndpoint>,
    pub capabilities: Vec<String>,
    pub attributes: HashMap<String, String>,
}

/// A discoverable endpoint exposed by the edge node.
#[derive(Debug, Clone)]
pub struct DiscoveryEndpoint {
    pub name: String,
    pub scheme: String,
    pub port: u16,
    pub path: Option<String>,
    pub metadata: HashMap<String, String>,
}

/// Metadata updates for the discovery advertisement.
#[derive(Debug, Clone)]
pub struct DiscoveryMetadata {
    pub status: String,
    pub server_id: Option<String>,
    pub capabilities: Vec<String>,
    pub attributes: HashMap<String, String>,
}

/// Device discovery and advertising backend (slot: `discovery`).
#[tonic::async_trait]
pub trait DiscoveryBackend: ModuleLifecycle {
    /// Register this device so the server can discover it.
    async fn register_service(&self, info: &DeviceInfo) -> Result<()>;

    /// Update advertised metadata (e.g., after binding state changes).
    async fn update_metadata(&self, metadata: &DiscoveryMetadata) -> Result<()>;

    /// Unregister the service.
    async fn unregister(&self) -> Result<()>;
}
