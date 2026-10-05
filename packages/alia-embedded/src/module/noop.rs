//! Noop implementations for optional module slots.
//!
//! These allow disabling functionality without conditional branches—
//! the noop simply does nothing and returns neutral values.

use anyhow::Result;

use super::context::AudioBus;
use super::lifecycle::ModuleLifecycle;
use super::traits::*;

// ─── NoopVad ─────────────────────────────────────────────────────────────────

/// Noop VAD: reports unavailable so speech capture uses fixed-duration fallback.
pub struct NoopVad;

#[tonic::async_trait]
impl ModuleLifecycle for NoopVad {
    fn name(&self) -> &'static str {
        "noop_vad"
    }
}

impl VoiceActivityEngine for NoopVad {
    fn reset(&self) -> Result<()> {
        Ok(())
    }

    fn accept(&self, _pcm: &[i16], _sample_rate: u32) -> Result<VoiceActivityState> {
        Ok(VoiceActivityState::Unavailable)
    }

    fn is_available(&self) -> bool {
        false
    }
}

// ─── NoopDirectResponse ──────────────────────────────────────────────────────

/// Noop direct response: never matches any keyword.
pub struct NoopDirectResponse;

#[tonic::async_trait]
impl ModuleLifecycle for NoopDirectResponse {
    fn name(&self) -> &'static str {
        "noop_direct_response"
    }
}

impl DirectResponseEngine for NoopDirectResponse {
    fn match_keyword(&self, _text: &str) -> Option<DirectResponseAction> {
        None
    }

    fn match_action(&self, _action: &str) -> Option<DirectResponseAction> {
        None
    }
}

// ─── NoopFaceTracker ─────────────────────────────────────────────────────────

/// Noop face tracker: reports no faces and no active tracking.
pub struct NoopFaceTracker;

#[tonic::async_trait]
impl ModuleLifecycle for NoopFaceTracker {
    fn name(&self) -> &'static str {
        "noop_face_tracker"
    }
}

#[tonic::async_trait]
impl FaceTracker for NoopFaceTracker {
    async fn get_faces(&self) -> Result<Vec<FaceDetection>> {
        Ok(Vec::new())
    }

    fn is_tracking(&self) -> bool {
        false
    }
}

// ─── NoopWakeWord ────────────────────────────────────────────────────────────

/// Noop wake word engine: never detects a wake word (useful for testing).
pub struct NoopWakeWord;

#[tonic::async_trait]
impl ModuleLifecycle for NoopWakeWord {
    fn name(&self) -> &'static str {
        "noop_wakeword"
    }
}

impl WakeWordEngine for NoopWakeWord {
    fn reset(&self) -> Result<()> {
        Ok(())
    }

    fn detect(&self, _pcm: &[i16]) -> Option<WakeWordEvent> {
        None
    }
}

// ─── NoopAudioCapture ────────────────────────────────────────────────────────

/// Noop audio capture: does not capture any audio (useful for testing).
pub struct NoopAudioCapture;

#[tonic::async_trait]
impl ModuleLifecycle for NoopAudioCapture {
    fn name(&self) -> &'static str {
        "noop_audio_capture"
    }
}

impl AudioCaptureBackend for NoopAudioCapture {
    fn start_capture(&self, _bus: &AudioBus) -> Result<()> {
        Ok(())
    }

    fn stop_capture(&self) -> Result<()> {
        Ok(())
    }

    fn is_available(&self) -> bool {
        false
    }
}

// ─── NoopAudioPlayback ───────────────────────────────────────────────────────

/// Noop audio playback: discards all audio (useful for testing).
pub struct NoopAudioPlayback;

#[tonic::async_trait]
impl ModuleLifecycle for NoopAudioPlayback {
    fn name(&self) -> &'static str {
        "noop_audio_playback"
    }
}

impl AudioPlaybackBackend for NoopAudioPlayback {
    fn is_available(&self) -> bool {
        false
    }
}

// ─── NoopDiscovery ───────────────────────────────────────────────────────────

/// Noop discovery: does not advertise or register the device.
pub struct NoopDiscovery;

#[tonic::async_trait]
impl ModuleLifecycle for NoopDiscovery {
    fn name(&self) -> &'static str {
        "noop_discovery"
    }
}

#[tonic::async_trait]
impl DiscoveryBackend for NoopDiscovery {
    async fn register_service(&self, _info: &DeviceInfo) -> Result<()> {
        Ok(())
    }

    async fn update_metadata(&self, _metadata: &DiscoveryMetadata) -> Result<()> {
        Ok(())
    }

    async fn unregister(&self) -> Result<()> {
        Ok(())
    }
}
