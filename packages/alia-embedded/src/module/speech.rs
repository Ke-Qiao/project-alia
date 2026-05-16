use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

mod capture;
mod continuous;
mod responses;
mod transport;
mod utterance;

use capture::{PendingSpeechCapture, SpeechCapture, prepare_vad_for_capture};
use continuous::ContinuousDialogueState;
use responses::{
    ACTION_LISTENING_CONTINUE, ACTION_LISTENING_START, ACTION_WAKEWORD,
    play_direct_response_action, play_direct_response_action_or_keyword,
};
use transport::interrupt_server_active_turn;
use utterance::{UtteranceOutcome, handle_utterance};

use super::audio_debug::AudioDebugRecorder;
use super::context::{AudioBus, EventBus, ModuleEvent};
use super::lifecycle::{ModuleHealth, ModuleLifecycle};
use super::traits::{
    AudioPlaybackBackend, DirectResponseEngine, EdgeSttEngine, VoiceActivityEngine,
    VoiceActivityState, WakeWordEngine,
};
use crate::binding::BindingManager;
use crate::config::{ContinuousDialogueConfig, EdgeSttConfig, InterruptConfig};

pub struct SpeechPipeline {
    audio_bus: AudioBus,
    event_bus: EventBus,
    wakeword: Arc<dyn WakeWordEngine>,
    edge_stt: Arc<dyn EdgeSttEngine>,
    vad: Arc<dyn VoiceActivityEngine>,
    direct_response: Arc<dyn DirectResponseEngine>,
    playback: Arc<dyn AudioPlaybackBackend>,
    binding: Arc<BindingManager>,
    audio_debug: AudioDebugRecorder,
    max_duration_ms: u32,
    post_wake_capture_ms: u32,
    post_wake_min_capture_ms: u32,
    post_wake_response_delay_ms: u32,
    interrupt_config: InterruptConfig,
    continuous_config: ContinuousDialogueConfig,
    cancel_token: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl SpeechPipeline {
    pub fn new(
        audio_bus: AudioBus,
        event_bus: EventBus,
        wakeword: Arc<dyn WakeWordEngine>,
        edge_stt: Arc<dyn EdgeSttEngine>,
        vad: Arc<dyn VoiceActivityEngine>,
        direct_response: Arc<dyn DirectResponseEngine>,
        playback: Arc<dyn AudioPlaybackBackend>,
        binding: Arc<BindingManager>,
        edge_stt_config: &EdgeSttConfig,
        interrupt_config: &InterruptConfig,
        continuous_config: &ContinuousDialogueConfig,
        audio_debug: AudioDebugRecorder,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            audio_bus,
            event_bus,
            wakeword,
            edge_stt,
            vad,
            direct_response,
            playback,
            binding,
            audio_debug,
            max_duration_ms: edge_stt_config.max_duration_ms,
            post_wake_capture_ms: edge_stt_config.post_wake_capture_ms,
            post_wake_min_capture_ms: edge_stt_config.post_wake_min_capture_ms,
            post_wake_response_delay_ms: edge_stt_config.post_wake_response_delay_ms,
            interrupt_config: interrupt_config.clone(),
            continuous_config: continuous_config.clone(),
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
        if task.as_ref().is_some_and(|task| !task.is_finished()) {
            return Ok(());
        }
        if task.take().is_some() {
            warn!("speech pipeline task had stopped; restarting");
        }

        let mut rx = self.audio_bus.subscribe();
        let mut event_rx = self.event_bus.subscribe();
        let event_bus = self.event_bus.clone();
        let wakeword = Arc::clone(&self.wakeword);
        let edge_stt = Arc::clone(&self.edge_stt);
        let vad = Arc::clone(&self.vad);
        let direct_response = Arc::clone(&self.direct_response);
        let playback = Arc::clone(&self.playback);
        let binding = Arc::clone(&self.binding);
        let audio_debug = self.audio_debug.clone();
        let max_duration_ms = self.max_duration_ms;
        let post_wake_capture_ms = self.post_wake_capture_ms;
        let post_wake_min_capture_ms = self.post_wake_min_capture_ms;
        let post_wake_response_delay_ms = self.post_wake_response_delay_ms;
        let interrupt_config = self.interrupt_config.clone();
        let continuous_config = self.continuous_config.clone();
        let cancel = self.cancel_token.child_token();

        *task = Some(tokio::spawn(async move {
            let mut capture: Option<SpeechCapture> = None;
            let mut pending_capture: Option<PendingSpeechCapture> = None;
            let mut continuous = ContinuousDialogueState::new(continuous_config);
            let mut playback_active = false;
            let mut last_interrupt_at_us = 0u64;

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    event = event_rx.recv() => {
                        match event {
                            Ok(ModuleEvent::PlaybackStarted { source }) => {
                                playback_active = true;
                                if is_assistant_playback_source(&source) {
                                    continuous.mark_assistant_speaking();
                                }
                            }
                            Ok(ModuleEvent::PlaybackFinished { source }) => {
                                playback_active = false;
                                if is_assistant_playback_source(&source) {
                                    continuous.schedule_followup();
                                }
                            }
                            Ok(_) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                debug!(skipped, "speech pipeline lagged behind module event bus");
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    frame = rx.recv() => {
                        let frame = match frame {
                            Ok(frame) => frame,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                warn!(skipped, "speech pipeline lagged behind audio bus");
                                continue;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        };
                        audio_debug.record_capture_frame(&frame).await;

                        if capture.is_none() && pending_capture.is_none() {
                            if continuous.start_followup_if_scheduled(frame.timestamp_us) {
                                if prepare_vad_for_capture(vad.as_ref()) {
                                    info!("continuous dialogue follow-up listening started");
                                    debug!(
                                        timeout_ms = continuous.followup_timeout_ms(),
                                        max_followup_turns = continuous.max_followup_turns(),
                                        "continuous dialogue follow-up listening started"
                                    );
                                    play_direct_response_action(
                                        ACTION_LISTENING_CONTINUE,
                                        Arc::clone(&direct_response),
                                        Arc::clone(&playback),
                                        event_bus.clone(),
                                        audio_debug.clone(),
                                    )
                                    .await;
                                    rx = rx.resubscribe();
                                    continue;
                                } else {
                                    continuous.end_session();
                                    warn!("continuous dialogue follow-up requires VAD; ending session");
                                }
                            }

                            if continuous.expire_if_needed(frame.timestamp_us) {
                                info!("continuous dialogue session ended after follow-up timeout");
                            }

                            if continuous.is_followup_listening() && !playback_active {
                                match vad.accept(&frame.pcm, frame.sample_rate) {
                                    Ok(VoiceActivityState::Speech) => {
                                        continuous.mark_followup_capture_started();
                                        pending_capture = Some(PendingSpeechCapture::followup(
                                            frame.timestamp_us,
                                            VoiceActivityState::Speech,
                                        ));
                                    }
                                    Ok(VoiceActivityState::Unavailable) => {
                                        continuous.end_session();
                                        warn!("continuous dialogue VAD became unavailable; ending session");
                                    }
                                    Ok(VoiceActivityState::Silence | VoiceActivityState::Finished) => {}
                                    Err(error) => {
                                        continuous.end_session();
                                        warn!(error = %error, "continuous dialogue VAD failed; ending session");
                                        debug!(error = ?error, "continuous dialogue VAD failure details");
                                    }
                                }
                            }
                        }

                        if let Some(pending) = pending_capture.take() {
                            event_bus.publish(ModuleEvent::SpeechStart);
                            let vad_enabled = if pending.reset_vad {
                                prepare_vad_for_capture(vad.as_ref())
                            } else {
                                vad.is_available()
                            };
                            info!(
                                origin = pending.origin.as_str(),
                                "started speech capture"
                            );
                            debug!(
                                keyword = %pending.event.keyword,
                                origin = pending.origin.as_str(),
                                vad_enabled,
                                capture_ms = post_wake_capture_ms,
                                min_capture_ms = post_wake_min_capture_ms,
                                max_ms = max_duration_ms,
                                "started speech capture"
                            );
                            let mut next_capture = SpeechCapture::new(
                                pending.event,
                                pending.origin,
                                frame.sample_rate,
                                frame.timestamp_us,
                                Duration::from_millis(post_wake_capture_ms as u64),
                                Duration::from_millis(post_wake_min_capture_ms as u64),
                                Duration::from_millis(max_duration_ms as u64),
                                vad_enabled,
                            );
                            if let Some(state) = pending.initial_vad_state {
                                next_capture.observe_vad(state);
                                next_capture.skip_next_vad_accept();
                            }
                            capture = Some(next_capture);
                        }

                        if let Some(active) = capture.as_mut() {
                            active.push(&frame);
                            if active.vad_enabled() && !active.take_skip_next_vad_accept() {
                                match vad.accept(&frame.pcm, frame.sample_rate) {
                                    Ok(state) => active.observe_vad(state),
                                    Err(error) => {
                                        warn!(error = %error, "VAD failed during speech capture; falling back to fixed capture window");
                                        debug!(error = ?error, "VAD failure details");
                                        active.disable_vad();
                                    }
                                }
                            }
                            if let Some(reason) = active.completion_reason(frame.timestamp_us) {
                                let finished = capture.take().expect("capture is active");
                                if vad.is_available()
                                    && let Err(error) = vad.reset()
                                {
                                    warn!(error = %error, "failed to reset VAD after speech capture");
                                    debug!(error = ?error, "VAD reset failure details");
                                }
                                info!(
                                    origin = finished.origin.as_str(),
                                    "finished speech capture"
                                );
                                debug!(
                                    reason = reason.as_str(),
                                    duration_ms = finished.duration_ms(),
                                    samples = finished.pcm.len(),
                                    "finished post-wake speech capture"
                                );
                                event_bus.publish(ModuleEvent::SpeechEnd);
                                let outcome = handle_utterance(
                                    finished,
                                    Arc::clone(&edge_stt),
                                    Arc::clone(&direct_response),
                                    Arc::clone(&playback),
                                    Arc::clone(&binding),
                                    audio_debug.clone(),
                                    event_bus.clone(),
                                    max_duration_ms,
                                )
                                .await;
                                match outcome {
                                    UtteranceOutcome::UploadedToServer => {
                                        continuous.mark_waiting_for_assistant();
                                    }
                                    UtteranceOutcome::LocalDirectResponse => {
                                        continuous.schedule_followup();
                                    }
                                    UtteranceOutcome::Ignored
                                    | UtteranceOutcome::FallbackResponse
                                    | UtteranceOutcome::Cancelled => {
                                        continuous.end_session();
                                    }
                                }
                                rx = rx.resubscribe();
                            }
                            continue;
                        }

                        if let Some(event) = wakeword.detect(&frame.pcm) {
                            info!("wakeword detected");
                            debug!(
                                keyword = %event.keyword,
                                confidence = event.confidence,
                                "wakeword detection details"
                            );
                            continuous.start_session();
                            event_bus.publish(ModuleEvent::WakeWordDetected {
                                keyword: event.keyword.clone(),
                                timestamp_us: event.timestamp_us,
                            });
                            let interrupting_playback = interrupt_config.interrupt_on_wakeword_during_playback
                                && playback_active
                                && event.timestamp_us.saturating_sub(last_interrupt_at_us)
                                    >= u64::from(interrupt_config.interrupt_debounce_ms) * 1_000;

                            if interrupting_playback {
                                last_interrupt_at_us = event.timestamp_us;
                                playback_active = false;
                                if let Err(error) = playback.stop_playback() {
                                    warn!(error = %error, "failed to stop local playback after wakeword interrupt");
                                    debug!(error = ?error, "local playback stop failure details");
                                }
                                event_bus.publish(ModuleEvent::PlaybackFinished {
                                    source: "wakeword_interrupt".to_string(),
                                });

                                let binding = Arc::clone(&binding);
                                let timestamp_us = event.timestamp_us;
                                tokio::spawn(async move {
                                    interrupt_server_active_turn(
                                        binding,
                                        "embedded".to_string(),
                                        "embedded-user".to_string(),
                                        "wakeword_during_playback".to_string(),
                                        timestamp_us,
                                    )
                                    .await;
                                });
                            } else if interrupt_config.interrupt_on_wakeword_during_playback
                                && playback_active
                            {
                                debug!(
                                    debounce_ms = interrupt_config.interrupt_debounce_ms,
                                    "wakeword interrupt ignored by debounce"
                                );
                            }

                            if !(interrupting_playback
                                && interrupt_config.interrupt_skip_direct_response)
                            {
                                let wakeword_audio_played = play_direct_response_action_or_keyword(
                                    ACTION_WAKEWORD,
                                    &event.keyword,
                                    Arc::clone(&direct_response),
                                    Arc::clone(&playback),
                                    event_bus.clone(),
                                    audio_debug.clone(),
                                )
                                .await;
                                if wakeword_audio_played && post_wake_response_delay_ms > 0 {
                                    tokio::time::sleep(Duration::from_millis(
                                        post_wake_response_delay_ms as u64,
                                    ))
                                    .await;
                                }

                                play_direct_response_action(
                                    ACTION_LISTENING_START,
                                    Arc::clone(&direct_response),
                                    Arc::clone(&playback),
                                    event_bus.clone(),
                                    audio_debug.clone(),
                                )
                                .await;
                                rx = rx.resubscribe();
                            }
                            pending_capture = Some(PendingSpeechCapture::wakeword(event));
                        }
                    }
                }
            }
        }));

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.cancel_token.cancel();
        if let Some(task) = self.task.lock().await.take() {
            task.abort();
        }
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if self
            .task
            .lock()
            .await
            .as_ref()
            .is_some_and(|task| !task.is_finished())
        {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Degraded
        }
    }
}

fn is_assistant_playback_source(source: &str) -> bool {
    source == "tts"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_playback_source_is_tts_only() {
        assert!(is_assistant_playback_source("tts"));
        assert!(!is_assistant_playback_source("wakeword_interrupt"));
        assert!(!is_assistant_playback_source(
            "run/direct-responses/wakeword/hello.wav"
        ));
    }
}
