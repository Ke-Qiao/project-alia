//! Continuous capture after wake word. Network/model work owns a separate task;
//! the audio consumer only performs bounded local analysis and `try_send`.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alia_core::delivery::VoiceScope;
use alia_core::output_control::VOICE_CONTRACT_VERSION;
use alia_core::playback_control::MAX_HOLD_MS;
use alia_proto::voice::{
    CaptureFrame, DeviceFrame, OpenSession, OutputDecision, RefreshAuth, SessionEvent, TurnState,
    device_frame, session_event, voice_session_client::VoiceSessionClient,
};
use anyhow::{Context, Result, bail, ensure};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::super::context::{AudioFrame, EventBus, ModuleEvent};
use super::super::playback_session::{CaptureControlBarrier, PlaybackSession};
use super::super::traits::{VoiceActivityEngine, VoiceActivityState, WakeWordEngine};
use super::transport::{
    IngressAuth, attach_ingress_auth, ingress_auth, pcm16_to_le_bytes, voice_connection,
};
use crate::binding::BindingManager;
use crate::config::SpeechConfig;

const SAMPLE_RATE: u32 = 16000;
const MAX_FRAME_SAMPLES: usize = 1600;
const CHANNEL_ID: &str = "embedded";
const AUTH_REFRESH: Duration = Duration::from_secs(120);

pub(super) struct Runtime {
    pub audio: broadcast::Receiver<AudioFrame>,
    pub events: broadcast::Receiver<ModuleEvent>,
    pub event_bus: EventBus,
    pub wakeword: Arc<dyn WakeWordEngine>,
    pub vad: Arc<dyn VoiceActivityEngine>,
    pub playback_session: Arc<PlaybackSession>,
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
        info!(boot_id = %self.boot_id, "server_turn capture enabled with scoped network playback; hardware AEC remains unverified");
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
                    if let Some(active) = session.take() {
                        if let Err(error) = active.playback.retire_capture(&active.capture) { warn!(%error, "retiring completed capture failed"); }
                    }
                    match result {
                        Ok(Ok(())) => info!("voice session ended; wake again to reconnect"),
                        Ok(Err(error)) => warn!(%error, "voice session failed; wake again to reconnect"),
                        Err(error) => {
                            warn!(%error, "voice network task failed");
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
        let playback = self.playback_session.clone();
        let capture = CaptureControlBarrier::default();
        let task_capture = capture.clone();
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
                        if !(current.state == crate::binding::BindingState::Bound
                            && current.server_id == original.server_id
                            && current.server_endpoint == original.server_endpoint
                            && current.server_public_key == original.server_public_key
                            && current.bound_since == original.bound_since
                            && current.dev_skip_auth == original.dev_skip_auth)
                        {
                            break Err(anyhow::anyhow!("binding changed during voice session"));
                        }
                    }
                };
                tokio::select! {
                    biased;
                    result = binding_changed => result,
                    result = exchange(endpoint, auth, boot_id, rx, playback, task_capture, device_id, keypair_path) => result,
                }
            };
            let result = tokio::select! {
                biased;
                _ = task_cancel.cancelled() => Ok(()),
                result = exchange => result,
            };
            result
        });
        Session {
            playback: self.playback_session.clone(),
            capture,
            tx,
            cancel,
            task,
            sequence: 0,
            sample_offset: 0,
        }
    }
}

struct Session {
    playback: Arc<PlaybackSession>,
    capture: CaptureControlBarrier,
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
        self.playback
            .accept_capture(&self.capture, self.sequence, frame.speech_active, || {
                self.tx
                    .try_send(CaptureFrame {
                        sequence: self.sequence,
                        sample_offset: self.sample_offset,
                        pcm_s16le: pcm16_to_le_bytes(&frame.pcm),
                        speech_active: frame.speech_active,
                    })
                    .context("voice upload queue full or disconnected; session cannot continue")
            })?;
        self.sequence = next_sequence;
        self.sample_offset = next_offset;
        Ok(())
    }
}

async fn end_session(session: &mut Option<Session>, events: &EventBus) {
    if let Some(session) = session.take() {
        if let Err(error) = session.playback.retire_capture(&session.capture) {
            warn!(%error, "retiring local capture failed");
        }
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

fn capture_stream(
    boot_id: String,
    mut audio: mpsc::Receiver<CaptureFrame>,
    ready: oneshot::Receiver<()>,
    sent: Arc<AtomicU64>,
    mut renew: impl FnMut() -> Result<IngressAuth> + Send + 'static,
    failed: oneshot::Sender<anyhow::Error>,
) -> impl futures::Stream<Item = DeviceFrame> + Send {
    async_stream::stream! {
        yield DeviceFrame { payload: Some(device_frame::Payload::Open(OpenSession {
            protocol_version: VOICE_CONTRACT_VERSION, device_boot_id: boot_id, channel_id: CHANNEL_ID.to_string(),
            software_hold: true, max_hold_ms: MAX_HOLD_MS,
        })) };
        // Local playback admission must be activated before audio can cause a
        // response to race back over the separate playback RPC connection.
        if ready.await.is_err() { return; }
        let mut refresh = tokio::time::interval_at(tokio::time::Instant::now() + AUTH_REFRESH, AUTH_REFRESH);
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = refresh.tick() => {
                    let auth = match renew() {
                        Ok(auth) => auth,
                        Err(error) => { let _ = failed.send(error); return; }
                    };
                    yield DeviceFrame { payload: Some(device_frame::Payload::RefreshAuth(RefreshAuth { bearer_token: auth.token })) };
                }
                frame = audio.recv() => match frame {
                    Some(frame) => {
                        sent.store(frame.sequence + 1, Ordering::Release);
                        yield DeviceFrame { payload: Some(device_frame::Payload::Audio(frame)) };
                    }
                    None => {
                        yield DeviceFrame { payload: Some(device_frame::Payload::Close(Default::default())) };
                        return;
                    }
                }
            }
        }
    }
}

async fn exchange(
    endpoint: String,
    auth: IngressAuth,
    boot_id: String,
    audio: mpsc::Receiver<CaptureFrame>,
    playback: Arc<PlaybackSession>,
    capture: CaptureControlBarrier,
    device_id: String,
    keypair_path: PathBuf,
) -> Result<()> {
    let sent = Arc::new(AtomicU64::new(0));
    let (ready_tx, ready_rx) = oneshot::channel();
    let (failed_tx, failed_rx) = oneshot::channel();
    let outgoing = capture_stream(
        boot_id.clone(),
        audio,
        ready_rx,
        sent.clone(),
        move || ingress_auth(&device_id, &keypair_path),
        failed_tx,
    );
    let mut guard = SessionGuard::with_capture(boot_id, playback, capture);
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
        guard.accept(first, sent.load(Ordering::Acquire))?;
        ready_tx
            .send(())
            .map_err(|_| anyhow::anyhow!("voice upload ended during Ready"))?;
        Ok::<_, anyhow::Error>(incoming)
    })
    .await
    .context("voice handshake timed out")??;
    let receive = async {
        while let Some(event) = incoming.message().await? {
            guard.accept(event, sent.load(Ordering::Acquire))?;
        }
        bail!("server closed continuous voice stream")
    };
    tokio::select! {
        result = receive => result,
        error = failed_rx => Err(error.unwrap_or_else(|_| anyhow::anyhow!("voice upload stream closed"))),
    }
}

struct SessionGuard {
    playback: Arc<PlaybackSession>,
    capture: CaptureControlBarrier,
    boot_id: String,
    session_id: Option<String>,
    epoch: u64,
    through_sequence: Option<u64>,
}

impl SessionGuard {
    #[cfg(test)]
    fn new(boot_id: String, playback: Arc<PlaybackSession>) -> Self {
        Self::with_capture(boot_id, playback, CaptureControlBarrier::default())
    }
    fn with_capture(
        boot_id: String,
        playback: Arc<PlaybackSession>,
        capture: CaptureControlBarrier,
    ) -> Self {
        Self {
            playback,
            capture,
            boot_id,
            session_id: None,
            epoch: 0,
            through_sequence: None,
        }
    }

    fn accept(&mut self, event: SessionEvent, sent: u64) -> Result<()> {
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
                state != TurnState::Unspecified,
                "server turn state is unspecified"
            );
            let decision = OutputDecision::try_from(evidence.output_decision)
                .context("unknown output decision")?;
            let advances = event.generation_epoch > self.epoch;
            if evidence.response_enqueued {
                ensure!(
                    state == TurnState::Complete
                        && decision == OutputDecision::Unchanged
                        && advances,
                    "response admission requires Complete, Unchanged and a new generation"
                );
            } else {
                match decision {
                    OutputDecision::Cancelled => {
                        ensure!(advances, "output cancellation must advance generation")
                    }
                    OutputDecision::Unchanged
                    | OutputDecision::PauseRequested
                    | OutputDecision::ResumeRequested => ensure!(
                        !advances,
                        "unchanged or reversible output request must retain generation"
                    ),
                    OutputDecision::Unspecified => bail!("output decision is unspecified"),
                }
            }
            debug!(request_id = %evidence.request_id, ?state, ?decision, response_enqueued = evidence.response_enqueued,
                "voice semantic evidence received; output decision is a request, not a native control receipt");
            // Same-epoch evidence is never permission to release the native
            // gate. Only independently authenticated Control can do that.
            if advances {
                self.playback
                    .fence(
                        &VoiceScope {
                            device_boot_id: Uuid::parse_str(&event.device_boot_id)?,
                            session_id: Uuid::parse_str(&event.session_id)?,
                            generation_epoch: event.generation_epoch,
                        },
                        false,
                    )
                    .context("advancing local voice playback fence")?;
            }
            self.through_sequence = Some(evidence.through_sequence);
        } else {
            let Some(session_event::Payload::Ready(ready)) = event.payload else {
                bail!("first voice event must be Ready")
            };
            ensure!(
                ready.protocol_version == VOICE_CONTRACT_VERSION
                    && ready.software_hold
                    && (1..=MAX_HOLD_MS).contains(&ready.max_hold_ms)
                    && ready.sample_rate_hz == SAMPLE_RATE
                    && ready.channels == 1
                    && ready.max_frame_samples == MAX_FRAME_SAMPLES as u32,
                "voice Ready format is incompatible"
            );
            self.playback
                .activate_with_capture(
                    &VoiceScope {
                        device_boot_id: Uuid::parse_str(&event.device_boot_id)?,
                        session_id: Uuid::parse_str(&event.session_id)?,
                        generation_epoch: event.generation_epoch,
                    },
                    self.capture.clone(),
                    ready.max_hold_ms,
                )
                .context("activating local voice playback session")?;
            self.session_id = Some(event.session_id.clone());
            info!(session_id = %event.session_id, boot_id = %self.boot_id, "continuous voice session ready");
        }
        self.epoch = event.generation_epoch;
        Ok(())
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Err(error) = self.playback.retire_capture(&self.capture) {
            warn!(%error, "retiring voice capture guard failed");
        }
        let Some(session_id) = self.session_id.as_ref() else {
            return;
        };
        // These UUIDs were validated before Ready was accepted. Only this exact
        // local session is closed; a replacement's playback remains untouched.
        if let (Ok(session_id), Ok(device_boot_id)) =
            (Uuid::parse_str(session_id), Uuid::parse_str(&self.boot_id))
        {
            if let Err(error) = self.playback.close(&VoiceScope {
                session_id,
                device_boot_id,
                generation_epoch: self.epoch,
            }) {
                warn!(%error, "closing local voice playback session failed");
            }
        }
    }
}

#[cfg(test)]
mod tests;
