//! Module factory: maps configuration strings to concrete module implementations.
//!
//! Each slot checks the `[modules]` config section and instantiates the
//! appropriate implementation. Unknown values produce an error at startup.

use std::sync::Arc;

use anyhow::{Result, bail};

use crate::config::EmbeddedConfig;

use super::cpal_audio::{CpalAudioCapture, CpalAudioPlayback};
use super::direct_response::KeywordMatcher;
use super::discovery::{BleDiscovery, MdnsDiscovery};
use super::motion::MotionRuntime;
use super::noop::*;
#[cfg(feature = "sherpa-onnx")]
use super::sherpa::{SherpaOnnxKws, SherpaOnnxStt, SherpaOnnxVad};
use super::traits::*;
use super::vision::YuNetFaceTracker;
use super::vision_debug::VisionDebugHub;
use super::volume::AudioVolumeState;

/// Create the wake word engine based on config.
pub fn create_wakeword(config: &EmbeddedConfig) -> Result<Box<dyn WakeWordEngine>> {
    match config.modules.wakeword.as_str() {
        "sherpa_onnx" => create_sherpa_wakeword(config),
        "porcupine" => {
            // TODO: Replace with PorcupineKws once pv_porcupine crate is integrated
            tracing::warn!("porcupine wakeword not yet implemented, using noop");
            Ok(Box::new(NoopWakeWord))
        }
        "noop" => Ok(Box::new(NoopWakeWord)),
        other => bail!("unknown wakeword engine: {other}"),
    }
}

/// Create the edge STT engine based on config.
pub fn create_edge_stt(config: &EmbeddedConfig) -> Result<Box<dyn EdgeSttEngine>> {
    match config.modules.edge_stt.as_str() {
        "sherpa_onnx" => create_sherpa_edge_stt(config),
        "noop" => Ok(Box::new(NoopEdgeStt)),
        other => bail!("unknown edge_stt engine: {other}"),
    }
}

/// Create the voice activity detector based on config.
pub fn create_vad(config: &EmbeddedConfig) -> Result<Box<dyn VoiceActivityEngine>> {
    match config.modules.vad.as_str() {
        "sherpa_onnx" => create_sherpa_vad(config),
        "noop" => Ok(Box::new(NoopVad)),
        other => bail!("unknown vad engine: {other}"),
    }
}

/// Create the direct response engine based on config.
pub fn create_direct_response(config: &EmbeddedConfig) -> Result<Box<dyn DirectResponseEngine>> {
    match config.modules.direct_response.as_str() {
        "keyword" => Ok(Box::new(KeywordMatcher::new(&config.direct_response))),
        "noop" => Ok(Box::new(NoopDirectResponse)),
        other => bail!("unknown direct_response engine: {other}"),
    }
}

#[cfg(feature = "sherpa-onnx")]
fn create_sherpa_wakeword(config: &EmbeddedConfig) -> Result<Box<dyn WakeWordEngine>> {
    Ok(Box::new(SherpaOnnxKws::new(&config.wakeword)?))
}

#[cfg(not(feature = "sherpa-onnx"))]
fn create_sherpa_wakeword(_config: &EmbeddedConfig) -> Result<Box<dyn WakeWordEngine>> {
    bail!("module wakeword='sherpa_onnx' requires the alia-embedded 'sherpa-onnx' feature")
}

#[cfg(feature = "sherpa-onnx")]
fn create_sherpa_edge_stt(config: &EmbeddedConfig) -> Result<Box<dyn EdgeSttEngine>> {
    Ok(Box::new(SherpaOnnxStt::new(&config.edge_stt)?))
}

#[cfg(not(feature = "sherpa-onnx"))]
fn create_sherpa_edge_stt(_config: &EmbeddedConfig) -> Result<Box<dyn EdgeSttEngine>> {
    bail!("module edge_stt='sherpa_onnx' requires the alia-embedded 'sherpa-onnx' feature")
}

#[cfg(feature = "sherpa-onnx")]
fn create_sherpa_vad(config: &EmbeddedConfig) -> Result<Box<dyn VoiceActivityEngine>> {
    Ok(Box::new(SherpaOnnxVad::new(&config.vad)?))
}

#[cfg(not(feature = "sherpa-onnx"))]
fn create_sherpa_vad(_config: &EmbeddedConfig) -> Result<Box<dyn VoiceActivityEngine>> {
    bail!("module vad='sherpa_onnx' requires the alia-embedded 'sherpa-onnx' feature")
}

/// Create the audio capture backend based on config.
pub fn create_audio_capture(
    config: &EmbeddedConfig,
    volume: AudioVolumeState,
) -> Result<Box<dyn AudioCaptureBackend>> {
    match config.modules.audio_capture.as_str() {
        "cpal" => Ok(Box::new(CpalAudioCapture::new(
            config.audio.clone(),
            volume,
        ))),
        "noop" => Ok(Box::new(NoopAudioCapture)),
        other => bail!("unknown audio_capture backend: {other}"),
    }
}

/// Create the audio playback backend based on config.
pub fn create_audio_playback(
    config: &EmbeddedConfig,
    volume: AudioVolumeState,
) -> Result<Box<dyn AudioPlaybackBackend>> {
    match config.modules.audio_playback.as_str() {
        "cpal" => Ok(Box::new(CpalAudioPlayback::new(
            config.audio.clone(),
            volume,
        ))),
        "noop" => Ok(Box::new(NoopAudioPlayback)),
        other => bail!("unknown audio_playback backend: {other}"),
    }
}

/// Create the audio mixer based on config.
pub fn create_mixer(config: &EmbeddedConfig) -> Result<Box<dyn AudioMixer>> {
    match config.modules.mixer.as_str() {
        "priority" => {
            // TODO: Replace with PriorityMixer
            tracing::warn!("priority mixer not yet implemented, using noop");
            Ok(Box::new(NoopMixer))
        }
        "noop" => Ok(Box::new(NoopMixer)),
        other => bail!("unknown mixer: {other}"),
    }
}

/// Create the face tracker based on config.
pub fn create_face_tracker(
    config: &EmbeddedConfig,
    motion: Arc<MotionRuntime>,
    debug_hub: Option<Arc<VisionDebugHub>>,
) -> Result<Box<dyn FaceTracker>> {
    match config.modules.face_tracker.as_str() {
        "yunet" => Ok(Box::new(YuNetFaceTracker::new(
            config.vision.clone(),
            motion,
            debug_hub,
        )?)),
        "noop" => Ok(Box::new(NoopFaceTracker)),
        other => bail!("unknown face_tracker: {other}"),
    }
}

/// Create the discovery backend based on config.
pub fn create_discovery(config: &EmbeddedConfig) -> Result<Box<dyn DiscoveryBackend>> {
    match config.modules.discovery.as_str() {
        "mdns" => Ok(Box::new(MdnsDiscovery::new(
            config.discovery.clone(),
            config.identity.clone(),
        ))),
        "ble" => Ok(Box::new(BleDiscovery::new(
            config.discovery.clone(),
            config.identity.clone(),
        ))),
        "noop" => Ok(Box::new(NoopDiscovery)),
        other => bail!("unknown discovery backend: {other}"),
    }
}
