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
    pub vad: Arc<dyn VoiceActivityEngine>,
    pub direct_response: Arc<dyn DirectResponseEngine>,
    pub audio_capture: Arc<dyn AudioCaptureBackend>,
    pub audio_playback: Arc<dyn AudioPlaybackBackend>,
    pub playback_session: Arc<PlaybackSession>,
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
    pub(crate) async fn start(
        &self,
        ctx: &super::context::ModuleContext,
        r1_cold_boot: Option<uuid::Uuid>,
    ) -> anyhow::Result<()> {
        let cancel = &ctx.cancel_token;
        start_module(self.wakeword.as_ref(), cancel).await?;
        start_module(self.vad.as_ref(), cancel).await?;
        start_module(self.direct_response.as_ref(), cancel).await?;
        start_module(self.audio_playback.as_ref(), cancel).await?;
        start_module(self.motion.as_ref(), cancel).await?;
        start_module(self.face_tracker.as_ref(), cancel).await?;
        start_module(self.vision_debug.as_ref(), cancel).await?;
        for module in &self.sensor_modules {
            start_module(module.as_ref(), cancel).await?;
        }
        start_module(self.audio_capture.as_ref(), cancel).await?;
        anyhow::ensure!(
            !cancel.is_cancelled(),
            "startup cancelled before microphone start"
        );
        let mut initialization_audio = r1_cold_boot.map(|_| ctx.audio_bus.subscribe());
        self.audio_capture
            .start_capture(&ctx.audio_bus)
            .map_err(|error| error.context("starting microphone capture"))?;
        if let Some(cold_boot_id) = r1_cold_boot {
            anyhow::ensure!(
                self.audio_capture.is_available(),
                "R1 capture is unavailable"
            );
            let first = tokio::select! {
                biased;
                _ = cancel.cancelled() => anyhow::bail!("R1 startup cancelled before initialization"),
                frame = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    initialization_audio.as_mut().expect("subscribed before capture").recv(),
                ) => frame.map_err(|_| anyhow::anyhow!("R1 capture produced no PCM before initialization"))??,
            };
            validate_r1_capture(&first)?;
            drop(initialization_audio);
            let stopped = ctx.audio_bus.capture_stopped();
            anyhow::ensure!(
                !cancel.is_cancelled()
                    && !stopped
                        .as_ref()
                        .is_some_and(|stopped| stopped.is_cancelled())
                    && self.audio_capture.is_available(),
                "R1 capture stopped before initialization"
            );
            let config = ctx.config.clone();
            let init_cancel = cancel.child_token();
            let worker_cancel = init_cancel.clone();
            let mut worker = tokio::task::spawn_blocking(move || {
                super::r1::initialize(
                    &config.r1,
                    &config.device_runtime.state_path,
                    cold_boot_id,
                    worker_cancel,
                )
            });
            let result = tokio::select! {
                result = &mut worker => result?,
                _ = async {
                    match &stopped {
                        Some(stopped) => stopped.cancelled().await,
                        None => std::future::pending().await,
                    }
                } => {
                    init_cancel.cancel();
                    // Join the serial owner before startup rollback closes the capture stream.
                    let result = worker.await?;
                    anyhow::bail!("R1 capture stopped during initialization: {result:?}");
                }
            }?;
            anyhow::ensure!(
                !cancel.is_cancelled()
                    && !stopped.is_some_and(|stopped| stopped.is_cancelled())
                    && self.audio_capture.is_available(),
                "R1 capture stopped at initialization completion; command may already be accepted"
            );
            tracing::info!(
                ?result,
                "R1 manual command accepted; audio validity requires separate evidence"
            );
        }
        if self.audio_capture.is_available() {
            start_module(self.speech_pipeline.as_ref(), cancel).await?;
        }
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

fn validate_r1_capture(frame: &super::context::AudioFrame) -> anyhow::Result<()> {
    use alia_core::audio_clock::{AudioDirection, AudioFormat};
    let format = AudioFormat {
        sample_rate_hz: 16000,
        channels: 1,
    };
    anyhow::ensure!(
        frame.sample_rate == 16000
            && !frame.pcm.is_empty()
            && frame
                .timing
                .is_some_and(
                    |timing| timing.native.stream.direction == AudioDirection::Capture
                        && timing.native.format == format
                        && timing.converted_format == format
                        && !timing.resampled
                        && timing.native.frames == frame.pcm.len() as u64
                        && timing.converted_frames == frame.pcm.len() as u64
                ),
        "R1 initialization requires observed native 16 kHz mono PCM without resampling"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::context::{AudioBus, AudioFrame};
    use alia_core::audio_clock::{AudioDirection, AudioFormat, CaptureTiming};

    #[test]
    fn r1_initialization_checks_observed_capture_format_not_just_config() {
        let format = AudioFormat {
            sample_rate_hz: 16000,
            channels: 1,
        };
        let bus = AudioBus::new(4);
        let mut clock = bus.reference().open_stream(AudioDirection::Capture, format);
        let native = clock.observe(320, clock.probe.unknown_clock());
        let mut frame = AudioFrame {
            pcm: vec![0; 320],
            sample_rate: 16000,
            timestamp_us: 0,
            timing: Some(CaptureTiming {
                native,
                converted_format: format,
                converted_start_frame: 0,
                converted_frames: 320,
                resampled: false,
            }),
        };
        validate_r1_capture(&frame).unwrap(); // Silence is valid PCM, not proof of speech.
        frame.timing.as_mut().unwrap().native.format.sample_rate_hz = 48000;
        assert!(validate_r1_capture(&frame).is_err());
        frame.timing.as_mut().unwrap().native.format = format;
        frame.timing.as_mut().unwrap().native.format.channels = 2;
        assert!(validate_r1_capture(&frame).is_err());
        frame.timing.as_mut().unwrap().native.format = format;
        frame.timing.as_mut().unwrap().resampled = true;
        assert!(validate_r1_capture(&frame).is_err());
        frame.timing = None;
        assert!(validate_r1_capture(&frame).is_err());
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
