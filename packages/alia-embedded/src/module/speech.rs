use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

mod duplex;
pub mod transport;

use super::audio_debug::AudioDebugRecorder;
use super::context::{AudioBus, EventBus};
use super::lifecycle::{ModuleHealth, ModuleLifecycle};
use super::playback_session::PlaybackSession;
use super::traits::{DirectResponseEngine, VoiceActivityEngine, WakeWordEngine};
use crate::binding::BindingManager;
use crate::config::{IdentityConfig, SpeechConfig};

/// The single device voice path: wake, then continuous capture and scoped playback.
/// Local VAD supplies evidence; only the server decides turn boundaries.
pub struct SpeechPipeline {
    audio_bus: AudioBus,
    event_bus: EventBus,
    wakeword: Arc<dyn WakeWordEngine>,
    vad: Arc<dyn VoiceActivityEngine>,
    direct_response: Arc<dyn DirectResponseEngine>,
    playback_session: Arc<PlaybackSession>,
    binding: Arc<BindingManager>,
    audio_debug: AudioDebugRecorder,
    speech_config: SpeechConfig,
    device_boot_id: String,
    device_id: String,
    keypair_path: PathBuf,
    cancel_token: CancellationToken,
    task: Mutex<Option<SpeechTask>>,
}

struct SpeechTask {
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

impl SpeechPipeline {
    pub fn new(
        audio_bus: AudioBus,
        event_bus: EventBus,
        wakeword: Arc<dyn WakeWordEngine>,
        vad: Arc<dyn VoiceActivityEngine>,
        direct_response: Arc<dyn DirectResponseEngine>,
        playback_session: Arc<PlaybackSession>,
        device_boot_id: uuid::Uuid,
        binding: Arc<BindingManager>,
        speech_config: &SpeechConfig,
        identity_config: &IdentityConfig,
        audio_debug: AudioDebugRecorder,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            audio_bus,
            event_bus,
            wakeword,
            vad,
            direct_response,
            playback_session,
            binding,
            audio_debug,
            speech_config: speech_config.clone(),
            device_boot_id: device_boot_id.to_string(),
            device_id: identity_config.device_id.clone(),
            keypair_path: identity_config.keypair_path.clone(),
            cancel_token,
            task: Mutex::new(None),
        }
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for SpeechPipeline {
    fn name(&self) -> &'static str {
        "speech_pipeline"
    }

    async fn start(&self) -> Result<()> {
        let mut task = self.task.lock().await;
        if task.as_ref().is_some_and(|task| !task.handle.is_finished()) {
            return Ok(());
        }
        if let Some(previous) = task.take() {
            previous.cancel.cancel();
            previous.handle.await?;
            warn!("speech pipeline task had stopped; restarting");
        }
        anyhow::ensure!(
            self.vad.is_available(),
            "continuous voice requires an available VAD"
        );
        self.vad.reset()?;
        let cancel = self.cancel_token.child_token();
        let runtime = duplex::Runtime {
            audio: self.audio_bus.subscribe(),
            events: self.event_bus.subscribe(),
            event_bus: self.event_bus.clone(),
            wakeword: self.wakeword.clone(),
            vad: self.vad.clone(),
            direct_response: self.direct_response.clone(),
            audio_debug: self.audio_debug.clone(),
            playback_session: self.playback_session.clone(),
            binding: self.binding.clone(),
            device_id: self.device_id.clone(),
            keypair_path: self.keypair_path.clone(),
            boot_id: self.device_boot_id.clone(),
            config: self.speech_config.clone(),
            cancel: cancel.clone(),
        };
        *task = Some(SpeechTask {
            cancel,
            handle: tokio::spawn(runtime.run()),
        });
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        if let Some(task) = self.task.lock().await.take() {
            task.cancel.cancel();
            task.handle.await?;
        }
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if self
            .task
            .lock()
            .await
            .as_ref()
            .is_some_and(|task| !task.handle.is_finished())
        {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Degraded
        }
    }
}
