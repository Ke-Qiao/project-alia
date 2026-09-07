use std::collections::HashMap;
use std::sync::Arc;

use super::lifecycle::{ModuleHealth, ModuleLifecycle};
use super::motion::MotionRuntime;
use super::playback_session::PlaybackSession;
use super::sensor::SensorRegistry;
use super::speech::SpeechPipeline;
use super::traits::*;
use super::vision_debug::VisionDebugServer;
use crate::plugin::PluginRegistry;

/// Central registry holding all active module instances.
///
/// Non-slot core infrastructure is also held here for unified lifecycle
/// management and health reporting.
pub struct EmbeddedModuleRegistry {
    // Audio pipeline
    pub wakeword: Arc<dyn WakeWordEngine>,
    pub edge_stt: Arc<dyn EdgeSttEngine>,
    pub vad: Arc<dyn VoiceActivityEngine>,
    pub direct_response: Arc<dyn DirectResponseEngine>,
    pub audio_capture: Arc<dyn AudioCaptureBackend>,
    pub audio_playback: Arc<dyn AudioPlaybackBackend>,
    pub playback_session: Arc<PlaybackSession>,
    pub mixer: Arc<dyn AudioMixer>,
    pub speech_pipeline: Arc<SpeechPipeline>,

    // Vision
    pub face_tracker: Arc<dyn FaceTracker>,
    pub vision_debug: Arc<VisionDebugServer>,

    // Motion / expression hardware
    pub motion: Arc<MotionRuntime>,

    // Network
    pub discovery: Arc<dyn DiscoveryBackend>,

    // Core infrastructure (non-slot, unified lifecycle)
    pub sensors: Arc<SensorRegistry>,
    pub sensor_modules: Vec<Arc<dyn ModuleLifecycle>>,
    pub plugin_registry: Arc<PluginRegistry>,
}

impl EmbeddedModuleRegistry {
    /// Shut down all modules in reverse dependency order.
    pub async fn shutdown(&self) {
        // Reverse order: network → vision → audio output → audio input → processing
        let _ = self.discovery.stop().await;
        for module in self.sensor_modules.iter().rev() {
            let _ = module.stop().await;
        }
        let _ = self.motion.stop().await;
        let _ = self.vision_debug.stop().await;
        let _ = self.face_tracker.stop().await;
        let _ = self.mixer.stop().await;
        let _ = self.speech_pipeline.stop().await;
        let _ = self.audio_playback.stop().await;
        let _ = self.audio_capture.stop().await;
        let _ = self.direct_response.stop().await;
        let _ = self.vad.stop().await;
        let _ = self.edge_stt.stop().await;
        let _ = self.wakeword.stop().await;
    }

    /// Collect health status from all modules.
    pub async fn health(&self) -> HashMap<&'static str, ModuleHealth> {
        let mut map = HashMap::new();
        map.insert(self.wakeword.name(), self.wakeword.health_check().await);
        map.insert(self.edge_stt.name(), self.edge_stt.health_check().await);
        map.insert(self.vad.name(), self.vad.health_check().await);
        map.insert(
            self.direct_response.name(),
            self.direct_response.health_check().await,
        );
        map.insert(
            self.audio_capture.name(),
            self.audio_capture.health_check().await,
        );
        map.insert(
            self.audio_playback.name(),
            self.audio_playback.health_check().await,
        );
        map.insert(self.mixer.name(), self.mixer.health_check().await);
        map.insert(
            self.speech_pipeline.name(),
            self.speech_pipeline.health_check().await,
        );
        map.insert(
            self.face_tracker.name(),
            self.face_tracker.health_check().await,
        );
        map.insert(
            self.vision_debug.name(),
            self.vision_debug.health_check().await,
        );
        map.insert(self.motion.name(), self.motion.health_check().await);
        map.insert(self.discovery.name(), self.discovery.health_check().await);
        for module in &self.sensor_modules {
            map.insert(module.name(), module.health_check().await);
        }
        map
    }
}
