//! Continuous capture after wake word. Network/model work owns a separate task;
//! the audio consumer only performs bounded local analysis and `try_send`.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alia_core::audio_clock::{AudioDirection, AudioFormat, CaptureTiming};
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
use super::super::traits::{
    DirectResponseEngine, VoiceActivityEngine, VoiceActivityState, WakeWordEngine,
};
use super::transport::{
    VoiceAuth, attach_voice_auth, pcm16_to_le_bytes, voice_auth, voice_connection,
};
use crate::binding::{BindingData, BindingManager};
use crate::config::SpeechConfig;

const SAMPLE_RATE: u32 = 16000;
const MAX_FRAME_SAMPLES: usize = 1600;
const AUTH_REFRESH: Duration = Duration::from_secs(120);

pub(super) struct Runtime {
    pub audio: broadcast::Receiver<AudioFrame>,
    pub events: broadcast::Receiver<ModuleEvent>,
    pub event_bus: EventBus,
    pub wakeword: Arc<dyn WakeWordEngine>,
    pub vad: Arc<dyn VoiceActivityEngine>,
    pub direct_response: Arc<dyn DirectResponseEngine>,
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
        if let Err(error) = self.wakeword.reset() {
            warn!(%error, "failed to reset wakeword before continuous capture");
            return;
        }
        let mut session: Option<Session> = None;
        let mut pre_roll = PreRoll::new(self.config.pre_roll_ms as usize * 16);
        let mut clock = CaptureClock::default();
        info!(boot_id = %self.boot_id, channel_id = %self.config.channel_id, "continuous capture enabled with scoped network playback; hardware AEC remains unverified");
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
                        active.cancel.cancel();
                        if let Err(error) = active.playback.retire_capture(&active.capture) { warn!(%error, "retiring completed capture failed"); }
                        if let Err(error) = active.cue_task.await { warn!(%error, "joining local voice cue failed"); }
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
                if let Err(error) = self.wakeword.reset() {
                    warn!(%error, "failed to reset wakeword after capture loss");
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
                    keyword: wake.keyword.clone(),
                    timestamp_us: wake.timestamp_us,
                });
                // Store the task before enqueueing so errors always cancel/join it.
                *session = Some(self.start_session(wake.keyword));
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

    fn start_session(&self, wake_keyword: String) -> Session {
        let (tx, rx) = mpsc::channel(self.config.upload_queue_frames);
        let cancel = self.cancel.child_token();
        let binding = self.binding.clone();
        let device_id = self.device_id.clone();
        let keypair_path = self.keypair_path.clone();
        let boot_id = self.boot_id.clone();
        let channel_id = self.config.channel_id.clone();
        let playback = self.playback_session.clone();
        let capture = CaptureControlBarrier::default();
        let task_capture = capture.clone();
        let task_cancel = cancel.clone();
        let (cue_ready, cue_scope) = oneshot::channel();
        let cue_task = tokio::spawn(play_ready_cue(
            cue_scope,
            wake_keyword,
            self.direct_response.clone(),
            self.playback_session.clone(),
            self.event_bus.clone(),
            cancel.clone(),
        ));
        let task = tokio::spawn(async move {
            let exchange = async {
                let original = binding.status().await;
                let (endpoint, auth) = voice_connection(&original, &device_id, &keypair_path)?;
                // Binding has no change subscription yet. Check immediately and
                // every 250 ms without involving the real-time audio consumer.
                let binding_changed = async {
                    let mut interval = tokio::time::interval(Duration::from_millis(250));
                    loop {
                        interval.tick().await;
                        if binding
                            .with_current_binding(&original, || Ok(()))
                            .await
                            .is_err()
                        {
                            break Err(anyhow::anyhow!("binding changed during voice session"));
                        }
                    }
                };
                tokio::select! {
                    biased;
                    result = binding_changed => result,
                    result = exchange(endpoint, auth, boot_id, channel_id, rx, playback, task_capture, device_id, keypair_path, binding.clone(), original.clone(), cue_ready) => result,
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
            cue_task,
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
    cue_task: JoinHandle<()>,
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
        if let Err(error) = session.cue_task.await {
            warn!(%error, "joining local voice cue failed");
        }
        events.publish(ModuleEvent::SpeechEnd);
    }
}

/// One bounded local cue per wake. The original authenticated Ready scope is
/// retained across WAV loading; it can never be replaced with a newer owner.
/// This worker is joined with its session and never blocks capture or network I/O.
async fn play_ready_cue(
    ready: oneshot::Receiver<VoiceScope>,
    wake_keyword: String,
    direct_response: Arc<dyn DirectResponseEngine>,
    playback: Arc<PlaybackSession>,
    event_bus: EventBus,
    cancel: CancellationToken,
) {
    let scope = tokio::select! {
        biased;
        _ = cancel.cancelled() => return,
        ready = ready => match ready {
            Ok(scope) => scope,
            Err(_) => return,
        },
    };
    let action = direct_response
        .match_action("wakeword")
        .or_else(|| direct_response.match_keyword(&wake_keyword));
    if let Some(action) = action {
        crate::module::direct_response_playback::play_direct_response(
            action, playback, scope, event_bus, cancel,
        )
        .await;
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
    timing: Option<CaptureTiming>,
}

impl CaptureClock {
    fn accept(&mut self, frame: &AudioFrame) -> Result<()> {
        ensure!(
            frame.sample_rate == SAMPLE_RATE && !frame.pcm.is_empty(),
            "capture format changed or frame is empty; expected 16 kHz mono"
        );
        if self.origin_us.is_some() {
            ensure!(
                self.timing.is_some() == frame.timing.is_some(),
                "native capture timing evidence changed"
            );
        }
        if let Some(timing) = frame.timing {
            ensure!(
                timing.native.stream.direction == AudioDirection::Capture
                    && timing.native.format.is_valid()
                    && timing.native.frames != 0
                    && timing.native.clock.clock_id == timing.native.stream.stream_id
                    && timing.converted_format
                        == (AudioFormat {
                            sample_rate_hz: SAMPLE_RATE,
                            channels: 1,
                        })
                    && timing.converted_frames == frame.pcm.len() as u64
                    && timing.resampled == (timing.native.format.sample_rate_hz != SAMPLE_RATE),
                "invalid native capture timing or converted format"
            );
            if let Some(previous) = self.timing {
                ensure!(
                    timing.native.stream == previous.native.stream
                        && timing.native.continuity == previous.native.continuity
                        && timing.native.format == previous.native.format,
                    "native capture stream or continuity changed"
                );
                ensure!(
                    timing.converted_start_frame
                        == previous
                            .converted_start_frame
                            .checked_add(previous.converted_frames)
                            .context("converted capture clock overflow")?,
                    "converted capture cursor discontinuity"
                );
            }
            // Native batches can produce no converted output, and resampling
            // retains a tail. Their cursors are not a calibrated sample map.
        }
        let origin = self.origin_us.unwrap_or(frame.timestamp_us);
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
        let samples = self
            .samples
            .checked_add(frame.pcm.len() as u64)
            .context("capture sample clock overflow")?;
        self.origin_us = Some(origin);
        self.samples = samples;
        self.timing = frame.timing;
        Ok(())
    }
}

fn capture_stream(
    boot_id: String,
    channel_id: String,
    mut audio: mpsc::Receiver<CaptureFrame>,
    ready: oneshot::Receiver<()>,
    sent: Arc<AtomicU64>,
    mut renew: impl FnMut() -> Result<VoiceAuth> + Send + 'static,
    failed: oneshot::Sender<anyhow::Error>,
) -> impl futures::Stream<Item = DeviceFrame> + Send {
    async_stream::stream! {
        yield DeviceFrame { payload: Some(device_frame::Payload::Open(OpenSession {
            protocol_version: VOICE_CONTRACT_VERSION, device_boot_id: boot_id, channel_id,
            software_hold: true, max_hold_ms: MAX_HOLD_MS,
            initiative_admission: true,
            initiative_capture_gate_version: alia_core::output_control::INITIATIVE_CAPTURE_GATE_VERSION,
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
    auth: VoiceAuth,
    boot_id: String,
    channel_id: String,
    audio: mpsc::Receiver<CaptureFrame>,
    playback: Arc<PlaybackSession>,
    capture: CaptureControlBarrier,
    device_id: String,
    keypair_path: PathBuf,
    binding: Arc<BindingManager>,
    original: BindingData,
    cue_ready: oneshot::Sender<VoiceScope>,
) -> Result<()> {
    crate::config::validate_speech_channel_id(&channel_id)?;
    let sent = Arc::new(AtomicU64::new(0));
    let (ready_tx, ready_rx) = oneshot::channel();
    let (failed_tx, failed_rx) = oneshot::channel();
    let outgoing = capture_stream(
        boot_id.clone(),
        channel_id.clone(),
        audio,
        ready_rx,
        sent.clone(),
        move || voice_auth(&device_id, &keypair_path),
        failed_tx,
    );
    let mut guard = SessionGuard::with_capture(boot_id, playback, capture);
    let mut incoming = tokio::time::timeout(Duration::from_secs(5), async {
        let mut client = VoiceSessionClient::connect(endpoint)
            .await?
            .max_decoding_message_size(64 * 1024)
            .max_encoding_message_size(64 * 1024);
        let mut request = tonic::Request::new(outgoing);
        attach_voice_auth(request.metadata_mut(), &auth, &channel_id)?;
        let mut incoming = client.stream(request).await?.into_inner();
        let first = incoming
            .message()
            .await?
            .context("voice stream ended before Ready")?;
        let cue_scope = VoiceScope {
            device_boot_id: Uuid::parse_str(&first.device_boot_id)?,
            session_id: Uuid::parse_str(&first.session_id)?,
            generation_epoch: first.generation_epoch,
        };
        guard
            .accept_bound(&binding, &original, first, sent.load(Ordering::Acquire))
            .await?;
        ready_tx
            .send(())
            .map_err(|_| anyhow::anyhow!("voice upload ended during Ready"))?;
        // Optional local audio can start only after binding and Ready have
        // established this exact scope. Capture is already free to upload.
        let _ = cue_ready.send(cue_scope);
        Ok::<_, anyhow::Error>(incoming)
    })
    .await
    .context("voice handshake timed out")??;
    let receive = async {
        while let Some(event) = incoming.message().await? {
            guard
                .accept_bound(&binding, &original, event, sent.load(Ordering::Acquire))
                .await?;
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
    initiative_admission: bool,
    initiative_capture_gate_version: u32,
    initiatives: std::collections::HashSet<Uuid>,
    initiative_requests: std::collections::HashSet<Uuid>,
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
            initiative_admission: false,
            initiative_capture_gate_version: 0,
            initiatives: Default::default(),
            initiative_requests: Default::default(),
        }
    }

    async fn accept_bound(
        &mut self,
        binding: &BindingManager,
        original: &BindingData,
        event: SessionEvent,
        sent: u64,
    ) -> Result<()> {
        binding
            .with_current_binding(original, || self.accept(event, sent))
            .await
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
            let (through_sequence, advances, initiative) = match event.payload {
                Some(session_event::Payload::InitiativeAdmitted(admission)) => {
                    ensure!(
                        self.initiative_admission
                            && self.initiative_capture_gate_version
                                == alia_core::output_control::INITIATIVE_CAPTURE_GATE_VERSION,
                        "initiative admission was not negotiated"
                    );
                    let initiative = Uuid::parse_str(&admission.initiative_id)?;
                    let request = Uuid::parse_str(&admission.request_id)?;
                    let context = Uuid::parse_str(&admission.context_id)?;
                    ensure!(
                        !initiative.is_nil() && !request.is_nil() && !context.is_nil(),
                        "initiative identity must not be nil"
                    );
                    ensure!(
                        self.epoch.checked_add(1) == Some(event.generation_epoch),
                        "initiative must advance exactly one generation"
                    );
                    ensure!(
                        self.initiatives.len() < 64
                            && !self.initiatives.contains(&initiative)
                            && !self.initiative_requests.contains(&request),
                        "initiative admission duplicate or capacity exceeded"
                    );
                    (
                        admission.through_sequence,
                        true,
                        Some((initiative, request, context)),
                    )
                }
                Some(session_event::Payload::Evidence(evidence)) => {
                    ensure!(
                        !evidence.request_id.trim().is_empty(),
                        "turn evidence request id is empty"
                    );
                    let state =
                        TurnState::try_from(evidence.state).context("unknown server turn state")?;
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
                    (evidence.through_sequence, advances, None)
                }
                _ => bail!("expected turn evidence or negotiated initiative after Ready"),
            };
            ensure!(
                through_sequence < sent,
                "voice admission refers to unsent capture"
            );
            ensure!(
                self.through_sequence
                    .is_none_or(|previous| through_sequence >= previous),
                "voice admission capture sequence regressed"
            );
            // Same-epoch evidence is never permission to release the native
            // gate. Only independently authenticated Control can do that.
            if advances {
                let scope = VoiceScope {
                    device_boot_id: Uuid::parse_str(&event.device_boot_id)?,
                    session_id: Uuid::parse_str(&event.session_id)?,
                    generation_epoch: event.generation_epoch,
                };
                if let Some((initiative_id, request_id, context_id)) = initiative {
                    self.playback.admit_initiative_event(
                        &scope,
                        alia_core::output_control::InitiativeCapture {
                            initiative_id,
                            request_id,
                            through_sequence,
                        },
                        context_id,
                    )?;
                } else {
                    self.playback
                        .fence(&scope, false)
                        .context("advancing local voice playback fence")?;
                }
            }
            if let Some((initiative, request, _)) = initiative {
                self.initiatives.insert(initiative);
                self.initiative_requests.insert(request);
            }
            self.through_sequence = Some(through_sequence);
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
            self.initiative_admission = ready.initiative_admission;
            self.initiative_capture_gate_version = ready.initiative_capture_gate_version;
            info!(session_id = %event.session_id, boot_id = %self.boot_id, "continuous voice session ready");
        }
        self.epoch = event.generation_epoch;
        Ok(())
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        // A replacement binding may reuse wire IDs. Only this actual capture
        // owner may close its Session; stale guard cleanup cannot close another.
        if let Err(error) = self.playback.close_capture(&self.capture) {
            warn!(%error, "closing voice capture guard failed");
        }
    }
}

#[cfg(test)]
mod tests;
