use std::collections::HashMap;
use std::sync::Arc;

use super::lifecycle::{
    ModuleCleanup, ModuleHealth, ModuleLifecycle, ShutdownHandle, start_module,
};
use super::motion::MotionRuntime;
use super::playback_session::PlaybackSession;
use super::sensor::SensorRegistry;
use super::speech::SpeechPipeline;
use super::traits::*;
use super::vision_debug::VisionDebugServer;
use crate::plugin::{PluginRegistry, PluginState};

/// Central registry holding all active module instances.
///
/// Non-slot core infrastructure is also held here for unified lifecycle
/// management and health reporting.
pub struct EmbeddedModuleRegistry {
    pub(crate) cleanup: Arc<ModuleCleanup>,
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
    pub(crate) async fn start(&self, ctx: &super::context::ModuleContext) -> anyhow::Result<()> {
        let cancel = &ctx.cancel_token;
        start_module(self.wakeword.as_ref(), cancel).await?;
        start_module(self.edge_stt.as_ref(), cancel).await?;
        start_module(self.vad.as_ref(), cancel).await?;
        start_module(self.direct_response.as_ref(), cancel).await?;
        start_module(self.audio_playback.as_ref(), cancel).await?;
        start_module(self.mixer.as_ref(), cancel).await?;
        start_module(self.motion.as_ref(), cancel).await?;
        start_module(self.face_tracker.as_ref(), cancel).await?;
        start_module(self.vision_debug.as_ref(), cancel).await?;
        for module in &self.sensor_modules {
            start_module(module.as_ref(), cancel).await?;
        }
        start_module(self.audio_capture.as_ref(), cancel).await?;
        start_module(self.speech_pipeline.as_ref(), cancel).await?;
        anyhow::ensure!(
            !cancel.is_cancelled(),
            "startup cancelled before microphone start"
        );
        self.audio_capture
            .start_capture(&ctx.audio_bus)
            .map_err(|error| error.context("starting microphone capture"))?;
        start_module(self.discovery.as_ref(), cancel).await?;
        Ok(())
    }

    /// Request cleanup; the returned handle completes only after module workers exit.
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.cleanup.start()
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.cleanup
            .start()
            .wait_until(self.cleanup.deadline())
            .await
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

/// Plugins own process wait tasks; request termination and await their observed exit.
/// A child that ignores termination remains pending under the node shutdown handle.
pub(crate) struct PluginCleanup(pub Arc<PluginRegistry>);

#[tonic::async_trait]
impl ModuleLifecycle for PluginCleanup {
    fn name(&self) -> &'static str {
        "plugins"
    }

    async fn stop(&self) -> anyhow::Result<()> {
        let mut errors = Vec::new();
        let mut failed = std::collections::HashSet::new();
        for plugin in self.0.list().await {
            if matches!(plugin.state, PluginState::Running | PluginState::Starting) {
                if let Err(error) = self.0.stop(&plugin.name).await {
                    errors.push(format!("{}: {error:#}", plugin.name));
                    failed.insert(plugin.name);
                }
            }
        }
        loop {
            let pending: Vec<_> = self
                .0
                .list()
                .await
                .into_iter()
                .filter(|plugin| {
                    matches!(
                        plugin.state,
                        PluginState::Running | PluginState::Starting | PluginState::Stopping
                    )
                })
                .map(|plugin| plugin.name)
                .collect();
            // Failed stop requests are reported explicitly; still wait for every
            // successfully requested process exit before returning that error.
            if pending.iter().all(|name| failed.contains(name)) {
                if errors.is_empty() {
                    return Ok(());
                }
                anyhow::bail!(
                    "plugin shutdown failed: {}; still active: {:?}",
                    errors.join("; "),
                    pending
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}
