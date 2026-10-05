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
/// `detect()` runs on the speech consumer worker. Native audio callbacks only
/// publish PCM and evidence; model inference must never run inside a callback.
pub trait WakeWordEngine: ModuleLifecycle {
    /// Discard partial detection history after capture loss or a fresh start.
    fn reset(&self) -> Result<()>;

    /// Feed PCM frames, returns detection event if wake word is found.
    fn detect(&self, pcm: &[i16]) -> Option<WakeWordEvent>;
}

// ─── Voice Activity Detection ────────────────────────────────────────────────

/// Streaming voice activity state for post-wake command endpointing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceActivityState {
    /// VAD is unavailable; the continuous voice pipeline must reject startup.
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
/// Supplies local speech evidence for continuous capture. Server turn detection
/// and transcription own semantic boundaries.
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

/// Selected local cue asset. `action` is lookup metadata, never a control command.
#[derive(Debug, Clone)]
pub struct DirectResponseAction {
    pub audio_path: Option<String>,
    pub action: Option<String>,
}

/// Local cue asset lookup (slot: `direct_response`).
/// Selection supplies a bounded clip; playback still requires the active voice scope.
pub trait DirectResponseEngine: ModuleLifecycle {
    /// Resolve a cue for the detected wake keyword. This does not inspect transcripts.
    fn match_keyword(&self, text: &str) -> Option<DirectResponseAction>;

    /// Resolve a configured cue alias such as `wakeword`. Missing audio is
    /// expected and must not fail the speech pipeline.
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
    /// Returns the original native owner's scope gate. Unsupported backends
    /// cannot acknowledge a reversible hold or manufacture a replacement owner.
    fn playback_control(
        &self,
        _scope: &alia_core::delivery::VoiceScope,
        _generation: u64,
    ) -> Result<std::sync::Arc<super::playback_session::PlaybackControl>> {
        anyhow::bail!("backend does not support reversible playback control")
    }
    fn apply_playback_control(
        &self,
        _owner: &std::sync::Arc<super::playback_session::PlaybackControl>,
        _command: &alia_core::playback_control::ControlCommand,
    ) -> Result<alia_core::playback_control::ControlReceipt> {
        anyhow::bail!("backend does not support reversible playback control")
    }
    /// Local bounded reference evidence; absent means this backend cannot expose
    /// a playback reference. Consumption must run outside native audio callbacks.
    fn reference_bus(&self) -> Option<super::context::AudioReferenceBus> {
        None
    }
    fn open_submission(
        &self,
        _identity: alia_core::delivery::PlaybackIdentity,
        _sample_rate: u32,
        _generation: u64,
    ) -> Result<std::sync::Arc<super::playback_session::AudioSubmission>> {
        anyhow::bail!("backend does not provide playback submission evidence")
    }

    /// Local short cues share the voice scope's native control owner, but have
    /// an independent source identity and never consume remote sequence numbers.
    fn open_cue_submission(
        &self,
        _identity: alia_core::delivery::PlaybackIdentity,
        _sample_rate: u32,
        _generation: u64,
    ) -> Result<std::sync::Arc<super::playback_session::AudioSubmission>> {
        anyhow::bail!("backend does not provide scoped cue mixing")
    }

    fn submit_audio(
        &self,
        _submission: &std::sync::Arc<super::playback_session::AudioSubmission>,
        _source_offset: u64,
        _pcm: &[i16],
    ) -> Result<bool> {
        anyhow::bail!("backend does not provide playback submission evidence")
    }

    fn end_submission(
        &self,
        _submission: &std::sync::Arc<super::playback_session::AudioSubmission>,
        _total_source_frames: u64,
    ) -> Result<bool> {
        anyhow::bail!("backend does not provide playback submission evidence")
    }

    fn cancel_submission(
        &self,
        _submission: &std::sync::Arc<super::playback_session::AudioSubmission>,
        _reason: &str,
    ) -> Result<bool> {
        anyhow::bail!("backend does not provide precise playback cancellation")
    }

    /// Start a new scoped output and return its process-local cancellation token.
    fn begin_scoped_playback(&self) -> Result<u64> {
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
