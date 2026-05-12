//! Module factory: maps configuration strings to concrete module implementations.
//!
//! Each slot checks the `[modules]` config section and instantiates the
//! appropriate implementation. Unknown values produce an error at startup.

use anyhow::{Result, bail};

use crate::config::EmbeddedConfig;

use super::discovery::{BleDiscovery, MdnsDiscovery};
use super::noop::*;
use super::traits::*;

/// Create the wake word engine based on config.
pub fn create_wakeword(config: &EmbeddedConfig) -> Result<Box<dyn WakeWordEngine>> {
    match config.modules.wakeword.as_str() {
        "sherpa_onnx" => {
            // TODO: Replace with real SherpaOnnxKws once sherpa-onnx crate is integrated
            tracing::warn!("sherpa_onnx wakeword not yet implemented, using noop");
            Ok(Box::new(NoopWakeWord))
        }
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
        "sherpa_onnx" => {
            // TODO: Replace with real SherpaOnnxStt
            tracing::warn!("sherpa_onnx edge_stt not yet implemented, using noop");
            Ok(Box::new(NoopEdgeStt))
        }
        "noop" => Ok(Box::new(NoopEdgeStt)),
        other => bail!("unknown edge_stt engine: {other}"),
    }
}

/// Create the direct response engine based on config.
pub fn create_direct_response(config: &EmbeddedConfig) -> Result<Box<dyn DirectResponseEngine>> {
    match config.modules.direct_response.as_str() {
        "keyword" => {
            // TODO: Replace with KeywordMatcher that uses config.direct_response entries
            tracing::warn!("keyword direct_response not yet implemented, using noop");
            Ok(Box::new(NoopDirectResponse))
        }
        "noop" => Ok(Box::new(NoopDirectResponse)),
        other => bail!("unknown direct_response engine: {other}"),
    }
}

/// Create the audio capture backend based on config.
pub fn create_audio_capture(config: &EmbeddedConfig) -> Result<Box<dyn AudioCaptureBackend>> {
    match config.modules.audio_capture.as_str() {
        "cpal" => {
            // TODO: Replace with CpalCapture once cpal crate is integrated
            tracing::warn!("cpal audio_capture not yet implemented, using noop");
            Ok(Box::new(NoopAudioCapture))
        }
        "noop" => Ok(Box::new(NoopAudioCapture)),
        other => bail!("unknown audio_capture backend: {other}"),
    }
}

/// Create the audio playback backend based on config.
pub fn create_audio_playback(config: &EmbeddedConfig) -> Result<Box<dyn AudioPlaybackBackend>> {
    match config.modules.audio_playback.as_str() {
        "cpal" => {
            // TODO: Replace with CpalPlayback once cpal crate is integrated
            tracing::warn!("cpal audio_playback not yet implemented, using noop");
            Ok(Box::new(NoopAudioPlayback))
        }
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
pub fn create_face_tracker(config: &EmbeddedConfig) -> Result<Box<dyn FaceTracker>> {
    match config.modules.face_tracker.as_str() {
        "mediapipe" => {
            // TODO: Replace with MediaPipeFace (Python subprocess)
            tracing::warn!("mediapipe face_tracker not yet implemented, using noop");
            Ok(Box::new(NoopFaceTracker))
        }
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
