//! Continuous capture after wake word. Network/model work owns a separate task;
//! the audio consumer only performs bounded local analysis and `try_send`.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alia_proto::voice::{
    CaptureFrame, DeviceFrame, OpenSession, SessionEvent, TurnState, device_frame, session_event,
    voice_session_client::VoiceSessionClient,
};
use anyhow::{Context, Result, bail, ensure};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::super::context::{AudioFrame, EventBus, ModuleEvent};
use super::super::traits::{
    AudioPlaybackBackend, VoiceActivityEngine, VoiceActivityState, WakeWordEngine,
};
use super::transport::{IngressAuth, attach_ingress_auth, pcm16_to_le_bytes, voice_connection};
use crate::binding::BindingManager;
use crate::config::SpeechConfig;

const SAMPLE_RATE: u32 = 16000;
const MAX_FRAME_SAMPLES: usize = 1600;
const CHANNEL_ID: &str = "embedded";

pub(super) struct Runtime {
    pub audio: broadcast::Receiver<AudioFrame>,
    pub events: broadcast::Receiver<ModuleEvent>,
    pub event_bus: EventBus,
    pub wakeword: Arc<dyn WakeWordEngine>,
    pub vad: Arc<dyn VoiceActivityEngine>,
    pub playback: Arc<dyn AudioPlaybackBackend>,
    pub binding: Arc<BindingManager>,
    pub device_id: String,
    pub keypair_path: PathBuf,
    pub boot_id: String,
    pub config: SpeechConfig,
    pub cancel: CancellationToken,
}

impl Runtime {
    pub async fn run(mut self) {
        let _cancel_on_exit = self.cancel.clone().drop_guard();
        let mut session: Option<Session> = None;
        let mut pre_roll = PreRoll::new(self.config.pre_roll_ms as usize * 16);
        let mut clock = CaptureClock::default();
        info!(boot_id = %self.boot_id, "server_turn capture enabled; AEC and network playback fencing remain unverified");
        loop {
            let damage = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => break,
                result = async {
                    match session.as_mut() {
                        Some(session) => (&mut session.task).await,
                        None => std::future::pending().await,
                    }
                } => {
                    // The handle was joined by select; do not poll it again.
                    session.take();
                    match result {
                        Ok(Ok(())) => info!("voice session ended; wake again to reconnect"),
                        Ok(Err(error)) => warn!(%error, "voice session failed; wake again to reconnect"),
                        Err(error) => {
                            warn!(%error, "voice network task failed");
                            if let Err(error) = self.playback.stop_playback() {
                                warn!(%error, "failed to stop playback after network task panic");
                            }
                        },
                    }
                    self.event_bus.publish(ModuleEvent::SpeechEnd);
                    Some("network session ended".to_string())
                }
                event = self.events.recv() => match event {
                    Ok(ModuleEvent::ContinuousDialogueEndRequested { reason }) => Some(reason),
                    Ok(ModuleEvent::PlaybackStarted { source }) => {
                        debug!(%source, "playback started; continuous capture remains active");
                        None
                    }
                    Ok(ModuleEvent::PlaybackFinished { source }) => {
                        debug!(%source, "playback event received; capture session remains active");
                        None
                    }
                    Ok(_) => None,
                    Err(broadcast::error::RecvError::Lagged(skipped)) =>
                        Some(format!("module events lagged by {skipped}; control state is uncertain")),
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                frame = self.audio.recv() => match frame {
                    Ok(frame) => self.accept_frame(frame, &mut clock, &mut pre_roll, &mut session)
                        .err().map(|error| error.to_string()),
                    Err(broadcast::error::RecvError::Lagged(skipped)) =>
                        Some(format!("audio bus lagged by {skipped}; capture continuity lost")),
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            };
            if let Some(reason) = damage {
                warn!(%reason, "ending damaged or closed voice session; fresh wake required");
                end_session(&mut session, &self.event_bus).await;
                pre_roll.clear();
                clock = CaptureClock::default();
                // Buffered audio from the failed session must not trigger its
                // replacement; reconnect starts with a genuinely fresh wake.
                self.audio = self.audio.resubscribe();
                if let Err(error) = self.vad.reset() {
                    warn!(%error, "failed to reset continuous VAD");
                    break;
                }
            }
        }
        end_session(&mut session, &self.event_bus).await;
    }

    fn accept_frame(
        &self,
        frame: AudioFrame,
        clock: &mut CaptureClock,
        pre_roll: &mut PreRoll,
        session: &mut Option<Session>,
    ) -> Result<()> {
        clock.accept(&frame)?;
        for pcm in frame.pcm.chunks(MAX_FRAME_SAMPLES) {
            let speech_active = match self.vad.accept(pcm, SAMPLE_RATE)? {
                VoiceActivityState::Speech => true,
                VoiceActivityState::Silence | VoiceActivityState::Finished => false,
                VoiceActivityState::Unavailable => bail!("continuous VAD became unavailable"),
            };
            let captured = Captured {
                pcm: pcm.to_vec(),
                speech_active,
            };
            if let Some(active) = session.as_mut() {
                active.enqueue(captured)?;
            } else if let Some(wake) = self.wakeword.detect(pcm) {
                self.event_bus.publish(ModuleEvent::WakeWordDetected {
                    keyword: wake.keyword,
                    timestamp_us: wake.timestamp_us,
                });
                // Store the task before enqueueing so errors always cancel/join it.
                *session = Some(self.start_session());
                let active = session.as_mut().expect("session was just stored");
                self.event_bus.publish(ModuleEvent::SpeechStart);
                for frame in pre_roll.drain() {
                    active.enqueue(frame)?;
                }
                active.enqueue(captured)?;
            } else {
                pre_roll.push(captured);
            }
        }
        Ok(())
    }

    fn start_session(&self) -> Session {
        let (tx, rx) = mpsc::channel(self.config.upload_queue_frames);
        let cancel = self.cancel.child_token();
        let binding = self.binding.clone();
        let device_id = self.device_id.clone();
        let keypair_path = self.keypair_path.clone();
        let boot_id = self.boot_id.clone();
        let playback = self.playback.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let exchange = async {
                let original = binding.status().await;
                let (endpoint, auth) =
                    voice_connection(&binding, &device_id, &keypair_path).await?;
                // Binding has no change subscription yet. Check immediately and
                // every 250 ms without involving the real-time audio consumer.
                let binding_changed = async {
                    let mut interval = tokio::time::interval(Duration::from_millis(250));
                    loop {
                        interval.tick().await;
                        let current = binding.status().await;
                        if !(current.state != crate::binding::BindingState::Unbound
                            && current.server_id == original.server_id
                            && current.server_endpoint == original.server_endpoint
                            && current.server_public_key == original.server_public_key)
                        {
                            break Err(anyhow::anyhow!("binding changed during voice session"));
                        }
                    }
                };
                tokio::select! {
                    biased;
                    result = binding_changed => result,
                    result = exchange(endpoint, auth, boot_id, rx, playback.as_ref()) => result,
                }
            };
            let result = tokio::select! {
                biased;
                _ = task_cancel.cancelled() => Ok(()),
                result = exchange => result,
            };
            // This is local stop only; stale network playback needs its own identity fence.
            let stopped = playback
                .stop_playback()
                .context("stopping playback after voice session closed");
            result.and(stopped)
        });
        Session {
            tx,
            cancel,
            task,
            sequence: 0,
            sample_offset: 0,
        }
    }
}

struct Session {
    tx: mpsc::Sender<CaptureFrame>,
    cancel: CancellationToken,
    task: JoinHandle<Result<()>>,
    sequence: u64,
    sample_offset: u64,
}

impl Session {
    fn enqueue(&mut self, frame: Captured) -> Result<()> {
        ensure!(
            !frame.pcm.is_empty() && frame.pcm.len() <= MAX_FRAME_SAMPLES,
            "invalid capture frame size"
        );
        let next_sequence = self
            .sequence
            .checked_add(1)
            .context("capture sequence overflow")?;
        let next_offset = self
            .sample_offset
            .checked_add(frame.pcm.len() as u64)
            .context("capture sample clock overflow")?;
        self.tx
            .try_send(CaptureFrame {
                sequence: self.sequence,
                sample_offset: self.sample_offset,
                pcm_s16le: pcm16_to_le_bytes(&frame.pcm),
                speech_active: frame.speech_active,
            })
            .context("voice upload queue full or disconnected; session cannot continue")?;
        self.sequence = next_sequence;
        self.sample_offset = next_offset;
        Ok(())
    }
}

async fn end_session(session: &mut Option<Session>, events: &EventBus) {
    if let Some(session) = session.take() {
        session.cancel.cancel();
        drop(session.tx);
        match session.task.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!(%error, "voice session shutdown failed"),
            Err(error) => warn!(%error, "voice session task join failed"),
        }
        events.publish(ModuleEvent::SpeechEnd);
    }
}

struct Captured {
    pcm: Vec<i16>,
    speech_active: bool,
}

struct PreRoll {
    frames: VecDeque<Captured>,
    samples: usize,
    limit: usize,
}

impl PreRoll {
    fn new(limit: usize) -> Self {
        Self {
            frames: VecDeque::new(),
            samples: 0,
            limit,
        }
    }
    fn push(&mut self, frame: Captured) {
        self.samples += frame.pcm.len();
        self.frames.push_back(frame);
        while self.samples > self.limit {
            let front = self.frames.front_mut().expect("nonempty pre-roll");
            let excess = (self.samples - self.limit).min(front.pcm.len());
            front.pcm.drain(..excess);
            self.samples -= excess;
            if front.pcm.is_empty() {
                self.frames.pop_front();
            }
        }
    }
    fn drain(&mut self) -> impl Iterator<Item = Captured> + '_ {
        self.samples = 0;
        self.frames.drain(..)
    }
    fn clear(&mut self) {
        self.frames.clear();
        self.samples = 0;
    }
}

#[derive(Default)]
struct CaptureClock {
    origin_us: Option<u64>,
    samples: u64,
}

impl CaptureClock {
    fn accept(&mut self, frame: &AudioFrame) -> Result<()> {
        ensure!(
            frame.sample_rate == SAMPLE_RATE && !frame.pcm.is_empty(),
            "capture format changed or frame is empty; expected 16 kHz mono"
        );
        let origin = *self.origin_us.get_or_insert(frame.timestamp_us);
        let expected = origin
            .checked_add(
                self.samples
                    .checked_mul(1_000_000)
                    .context("capture clock overflow")?
                    / SAMPLE_RATE as u64,
            )
            .context("capture clock overflow")?;
        // Capture timestamps are rounded to whole microseconds by the producer.
        ensure!(
            frame.timestamp_us.abs_diff(expected) <= 1,
            "capture timestamp discontinuity"
        );
        self.samples = self
            .samples
            .checked_add(frame.pcm.len() as u64)
            .context("capture sample clock overflow")?;
        Ok(())
    }
}

async fn exchange(
    endpoint: String,
    auth: IngressAuth,
    boot_id: String,
    mut audio: mpsc::Receiver<CaptureFrame>,
    playback: &dyn AudioPlaybackBackend,
) -> Result<()> {
    let sent = Arc::new(AtomicU64::new(0));
    let sent_count = sent.clone();
    let open_boot = boot_id.clone();
    let outgoing = async_stream::stream! {
        yield DeviceFrame { payload: Some(device_frame::Payload::Open(OpenSession {
            protocol_version: 1, device_boot_id: open_boot, channel_id: CHANNEL_ID.to_string(),
        })) };
        while let Some(frame) = audio.recv().await {
            sent_count.store(frame.sequence + 1, Ordering::Release);
            yield DeviceFrame { payload: Some(device_frame::Payload::Audio(frame)) };
        }
    };
    let mut guard = SessionGuard::new(boot_id);
    let mut incoming = tokio::time::timeout(Duration::from_secs(5), async {
        let mut client = VoiceSessionClient::connect(endpoint)
            .await?
            .max_decoding_message_size(64 * 1024)
            .max_encoding_message_size(64 * 1024);
        let mut request = tonic::Request::new(outgoing);
        attach_ingress_auth(request.metadata_mut(), &auth, CHANNEL_ID)?;
        let mut incoming = client.stream(request).await?.into_inner();
        let first = incoming
            .message()
            .await?
            .context("voice stream ended before Ready")?;
        guard.accept(first, sent.load(Ordering::Acquire), playback)?;
        Ok::<_, anyhow::Error>(incoming)
    })
    .await
    .context("voice handshake timed out")??;
    while let Some(event) = incoming.message().await? {
        guard.accept(event, sent.load(Ordering::Acquire), playback)?;
    }
    bail!("server closed continuous voice stream")
}

struct SessionGuard {
    boot_id: String,
    session_id: Option<String>,
    epoch: u64,
    through_sequence: Option<u64>,
}

impl SessionGuard {
    fn new(boot_id: String) -> Self {
        Self {
            boot_id,
            session_id: None,
            epoch: 0,
            through_sequence: None,
        }
    }

    fn accept(
        &mut self,
        event: SessionEvent,
        sent: u64,
        playback: &dyn AudioPlaybackBackend,
    ) -> Result<()> {
        ensure!(
            event.device_boot_id == self.boot_id,
            "voice event boot identity mismatch"
        );
        ensure!(
            !Uuid::parse_str(&event.device_boot_id)?.is_nil()
                && !Uuid::parse_str(&event.session_id)?.is_nil(),
            "voice identity UUID must not be nil"
        );
        ensure!(
            event.generation_epoch > 0 && event.generation_epoch >= self.epoch,
            "voice epoch invalid or regressed"
        );
        if let Some(session_id) = &self.session_id {
            ensure!(
                event.session_id == *session_id,
                "voice event session identity changed"
            );
            let Some(session_event::Payload::Evidence(evidence)) = event.payload else {
                bail!("expected turn evidence after Ready")
            };
            ensure!(
                evidence.through_sequence < sent,
                "turn evidence refers to unsent capture"
            );
            ensure!(
                self.through_sequence
                    .is_none_or(|previous| evidence.through_sequence >= previous),
                "turn evidence capture sequence regressed"
            );
            ensure!(
                !evidence.request_id.trim().is_empty(),
                "turn evidence request id is empty"
            );
            let state = TurnState::try_from(evidence.state).context("unknown server turn state")?;
            ensure!(
                state == TurnState::Complete || !evidence.response_enqueued,
                "non-complete evidence cannot enqueue a response"
            );
            match state {
                TurnState::Complete => {
                    info!(request_id = %evidence.request_id, response_enqueued = evidence.response_enqueued,
                    "turn complete evidence received; response_enqueued means bounded ingress acceptance only")
                }
                TurnState::Incomplete => debug!("turn incomplete; continuing capture"),
                TurnState::Backchannel => {
                    debug!("backchannel observed; keeping playback and capture running")
                }
                TurnState::Wait => {
                    playback
                        .stop_playback()
                        .context("stopping local playback for Wait")?;
                    warn!(
                        resume_policy = "not_implemented",
                        "Wait stops playback; pause/resume is not implemented"
                    );
                }
                TurnState::Unspecified => bail!("server turn state is unspecified"),
            }
            self.through_sequence = Some(evidence.through_sequence);
        } else {
            let Some(session_event::Payload::Ready(ready)) = event.payload else {
                bail!("first voice event must be Ready")
            };
            ensure!(
                ready.protocol_version == 1
                    && ready.sample_rate_hz == SAMPLE_RATE
                    && ready.channels == 1
                    && ready.max_frame_samples == MAX_FRAME_SAMPLES as u32,
                "voice Ready format is incompatible"
            );
            self.session_id = Some(event.session_id.clone());
            info!(session_id = %event.session_id, boot_id = %self.boot_id, "continuous voice session ready");
        }
        self.epoch = event.generation_epoch;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
