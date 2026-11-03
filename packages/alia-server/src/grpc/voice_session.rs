//! Authenticated continuous capture. One live session owns each device channel.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use alia_core::capture::{CaptureWindow, MAX_FRAME_SAMPLES};
use alia_core::delivery::VoiceScope;
use alia_core::output_control::VOICE_CONTRACT_VERSION;
use alia_core::playback_control::{MAX_HOLD_MS, ResumeAuthorization, ResumeKind};
use alia_core::turn::TurnController;
use alia_core::voice::{TurnObservation, TurnState};
use alia_proto::voice::voice_session_server::VoiceSession;
use alia_proto::voice::{self, DeviceFrame, SessionEvent, device_frame, session_event};
use futures::{FutureExt, Stream, StreamExt};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use super::voice_auth::{AdmittedSource, VoiceIngressAuth};
use crate::channels::adapter::ChannelAdapter;
use crate::modules::delivery::{DeliveryService, VoiceOutput};
use crate::modules::interaction::initiative::VoiceInitiativeHandle;
use crate::modules::interaction::turn_detector::{DetectionError, TurnDetector};
use crate::modules::interaction::voice_input::{
    RecognizedVoiceInput, RecognizedVoiceOutput, VoiceInputHandle,
};
use crate::modules::memory::context::ContextService;

mod initiative;

const STREAM_IDLE: Duration = Duration::from_secs(10);
const MAX_SESSIONS: usize = 16;
const AUTH_RECHECK: Duration = Duration::from_secs(30);
const AUTH_DEADLINE: Duration = Duration::from_secs(5);
const MAX_PENDING_FRAMES: usize = 64;
type Sessions = Arc<std::sync::Mutex<HashMap<String, Arc<SessionSlot>>>>;
#[derive(Default)]
struct SessionSlot {
    owner: Mutex<Option<(Uuid, CancellationToken)>>,
}
type Detection = Pin<Box<dyn Future<Output = Result<TurnObservation, DetectionError>> + Send>>;

pub struct VoiceSessionService {
    auth: VoiceIngressAuth,
    detector: Option<Arc<dyn TurnDetector>>,
    input: VoiceInputHandle,
    delivery: Arc<DeliveryService>,
    hold_budget: Duration,
    barge_in_enabled: bool,
    shutdown: CancellationToken,
    runtime_tasks: Arc<crate::modules::platform::lifecycle::RuntimeTasks>,
    sessions: Sessions,
    initiatives: Option<(VoiceInitiativeHandle, Arc<ContextService>)>,
}

impl VoiceSessionService {
    pub fn new(
        auth: VoiceIngressAuth,
        detector: Option<Arc<dyn TurnDetector>>,
        input: VoiceInputHandle,
        delivery: Arc<DeliveryService>,
        hold_budget: Duration,
        shutdown: CancellationToken,
        runtime_tasks: Arc<crate::modules::platform::lifecycle::RuntimeTasks>,
    ) -> Self {
        Self {
            auth,
            detector,
            input,
            delivery,
            hold_budget,
            barge_in_enabled: false,
            shutdown,
            runtime_tasks,
            sessions: Arc::default(),
            initiatives: None,
        }
    }

    /// Ordinary replies are not interrupted by microphone input unless enabled.
    pub fn with_barge_in_enabled(mut self, enabled: bool) -> Self {
        self.barge_in_enabled = enabled;
        self
    }

    /// Explicit wiring only; the default service never advertises initiatives.
    pub fn with_initiatives(
        mut self,
        handle: VoiceInitiativeHandle,
        context: Arc<ContextService>,
    ) -> Self {
        self.initiatives = Some((handle, context));
        self
    }
}

struct OutputRuntime {
    delivery: Arc<DeliveryService>,
    hold_budget: Duration,
    barge_in_enabled: bool,
    maximum_hold_ms: u64,
    initiatives: Option<(VoiceInitiativeHandle, Arc<ContextService>)>,
    initiative_output_enabled: bool,
    initiative_capture_gate_version: u32,
}

struct CapturePolicy {
    barge_in_enabled: bool,
    wait_for_silence: bool,
}

impl CapturePolicy {
    fn suppress(&mut self, output: Option<&VoiceOutput>, speech: bool) -> bool {
        let active = output.filter(|output| output.is_alive());
        // Proactive output retains its separate user-presence protection.
        if self.barge_in_enabled || active.is_some_and(VoiceOutput::is_initiative) {
            self.wait_for_silence = false;
            return false;
        }
        let suppressed = active.is_some() || self.wait_for_silence;
        if suppressed && speech && !self.wait_for_silence {
            tracing::debug!(
                "voice input suppressed until ordinary reply and current speech finish"
            );
        }
        // An utterance starting during output must not become a truncated new
        // request when the last native media owner finishes halfway through it.
        self.wait_for_silence = suppressed && speech;
        suppressed
    }
}

fn negotiated_hold_limit(open: &voice::OpenSession) -> Result<u64, Status> {
    if open.protocol_version != VOICE_CONTRACT_VERSION {
        return Err(Status::failed_precondition(
            "unsupported voice protocol version",
        ));
    }
    if !open.software_hold || !(1..=MAX_HOLD_MS).contains(&open.max_hold_ms) {
        return Err(Status::failed_precondition(
            "voice software hold capabilities are incompatible",
        ));
    }
    Ok(open.max_hold_ms)
}

#[derive(Debug, PartialEq, Eq)]
enum SemanticRequest {
    Listen,
    Pause,
    Resume(ResumeKind),
    Cancel,
    Respond(String),
}

fn semantic_request(observation: &TurnObservation) -> SemanticRequest {
    match observation.state {
        TurnState::Incomplete => SemanticRequest::Listen,
        TurnState::Wait => SemanticRequest::Pause,
        TurnState::Backchannel => SemanticRequest::Resume(ResumeKind::FreshBackchannel),
        TurnState::Complete => {
            let transcript = observation.transcript.trim();
            // Match the whole utterance only. A request such as "继续解释新的
            // 问题" is a new turn, never permission to release an old hold.
            let phrase = transcript
                .trim_matches(|c: char| ".,!?;:，。！？；：".contains(c))
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            match phrase.as_str() {
                "" => SemanticRequest::Listen,
                "继续" | "继续说" | "接着说" | "请继续" | "继续吧" | "continue"
                | "please continue" | "go on" | "resume" | "resume speaking" => {
                    SemanticRequest::Resume(ResumeKind::ExplicitContinue)
                }
                "停止" | "停止播放" | "别说了" | "不用说了" | "不要继续" | "不要继续说了"
                | "stop" | "stop speaking" | "cancel" | "cancel that" => SemanticRequest::Cancel,
                _ => SemanticRequest::Respond(transcript.to_owned()),
            }
        }
    }
}

struct SessionLease {
    channel: String,
    id: Uuid,
    sessions: Sessions,
    cancel: CancellationToken,
    slot: Arc<SessionSlot>,
}

impl SessionLease {
    async fn acquire(
        sessions: Sessions,
        channel: String,
        shutdown: &CancellationToken,
    ) -> Result<Self, Status> {
        let slot = {
            let mut active = sessions.lock().unwrap_or_else(|e| e.into_inner());
            active.retain(|_, slot| Arc::strong_count(slot) > 1);
            if !active.contains_key(&channel) && active.len() >= MAX_SESSIONS {
                return Err(Status::resource_exhausted("voice session capacity reached"));
            }
            active.entry(channel.clone()).or_default().clone()
        };
        let mut owner = slot.owner.lock().await;
        let id = Uuid::new_v4();
        let cancel = shutdown.child_token();
        if let Some((_, previous)) = owner.replace((id, cancel.clone())) {
            previous.cancel();
        }
        drop(owner);
        Ok(Self {
            channel,
            id,
            sessions,
            cancel,
            slot,
        })
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        self.cancel.cancel();
        let mut active = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if Arc::strong_count(&self.slot) == 2
            && active
                .get(&self.channel)
                .is_some_and(|slot| Arc::ptr_eq(slot, &self.slot))
        {
            active.remove(&self.channel);
        }
    }
}

#[tonic::async_trait]
impl VoiceSession for VoiceSessionService {
    type StreamStream = ReceiverStream<Result<SessionEvent, Status>>;

    async fn stream(
        &self,
        request: Request<tonic::Streaming<DeviceFrame>>,
    ) -> Result<Response<Self::StreamStream>, Status> {
        let detector = self
            .detector
            .clone()
            .ok_or_else(|| Status::failed_precondition("server turn detector is not configured"))?;
        // Authenticate before waiting for client audio or allocating a model window.
        let mut auth_request = Request::new(());
        *auth_request.metadata_mut() = request.metadata().clone();
        let channel_id = self.auth.authenticate_ingress(&auth_request, None).await?;
        self.auth.owner_user_id(&channel_id).await?;
        let mut stream = request.into_inner();
        let first = tokio::time::timeout(Duration::from_secs(5), stream.message())
            .await
            .map_err(|_| Status::deadline_exceeded("voice handshake deadline"))??
            .ok_or_else(|| Status::invalid_argument("voice stream requires a handshake"))?;
        let Some(device_frame::Payload::Open(open)) = first.payload else {
            return Err(Status::invalid_argument(
                "first voice frame must open a session",
            ));
        };
        let maximum_hold_ms = negotiated_hold_limit(&open)?;
        let claimed_channel = self
            .auth
            .authenticate_ingress(&auth_request, Some(&open.channel_id))
            .await?;
        if claimed_channel != channel_id {
            return Err(Status::permission_denied(
                "voice handshake binding mismatch",
            ));
        }
        let boot = Uuid::parse_str(&open.device_boot_id)
            .map_err(|_| Status::invalid_argument("invalid device boot UUID"))?;
        if boot.is_nil() {
            return Err(Status::invalid_argument("device boot UUID must not be nil"));
        }
        let lease =
            SessionLease::acquire(self.sessions.clone(), channel_id, &self.shutdown).await?;
        let (tx, rx) = mpsc::channel(8);
        let auth = self.auth.clone();
        let input = self.input.clone();
        let output_runtime = OutputRuntime {
            delivery: self.delivery.clone(),
            hold_budget: self.hold_budget.min(Duration::from_millis(maximum_hold_ms)),
            barge_in_enabled: self.barge_in_enabled,
            maximum_hold_ms,
            initiatives: open
                .initiative_admission
                .then(|| self.initiatives.clone())
                .flatten(),
            initiative_output_enabled: self.initiatives.is_some(),
            initiative_capture_gate_version: if open.initiative_capture_gate_version
                == alia_core::output_control::INITIATIVE_CAPTURE_GATE_VERSION
            {
                open.initiative_capture_gate_version
            } else {
                0
            },
        };
        self.runtime_tasks
            .try_spawn("voice_session", async move {
                let result = run_session(
                    stream,
                    &tx,
                    &lease,
                    boot,
                    detector,
                    input,
                    SessionCredentials {
                        auth,
                        request: auth_request,
                        channel: lease.channel.clone(),
                        source: None,
                        last_frame_at: tokio::time::Instant::now(),
                    },
                    output_runtime,
                    AUTH_RECHECK,
                )
                .await;
                if let Err(error) = result {
                    let _ = tx.try_send(Err(error));
                }
                Ok(())
            })
            .map_err(|_| Status::unavailable("server is stopping"))?;
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

/// A stream owns its credential state. Refresh and revalidation run in this
/// actor, so an old in-flight verification cannot overwrite a newer credential.
struct SessionCredentials {
    auth: VoiceIngressAuth,
    request: Request<()>,
    channel: String,
    /// Captured once before Ready. Refresh may invalidate, never replace it.
    source: Option<AdmittedSource>,
    last_frame_at: tokio::time::Instant,
}

impl SessionCredentials {
    async fn verify(&self, request: &Request<()>) -> Result<(), Status> {
        let channel = tokio::time::timeout(
            AUTH_DEADLINE,
            self.auth.authenticate_ingress(request, Some(&self.channel)),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("voice authentication deadline"))??;
        if channel != self.channel {
            return Err(Status::permission_denied(
                "voice credential binding changed",
            ));
        }
        if let Some(source) = &self.source {
            self.auth.revalidate_source(&self.channel, source).await?;
        }
        Ok(())
    }

    async fn revalidate(&self) -> Result<(), Status> {
        self.verify(&self.request).await
    }

    async fn refresh(&mut self, token: String) -> Result<(), Status> {
        if token.is_empty() || token.len() > 4096 {
            return Err(Status::unauthenticated("invalid voice refresh credential"));
        }
        let mut next = Request::new(());
        *next.metadata_mut() = self.request.metadata().clone();
        next.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}")
                .parse()
                .map_err(|_| Status::unauthenticated("invalid voice refresh credential"))?,
        );
        self.verify(&next).await?;
        self.request = next;
        Ok(())
    }
}

async fn fence_output(
    channel: &Arc<dyn ChannelAdapter>,
    scope: VoiceScope,
    close_session: bool,
    reason: &str,
) -> Result<(), Status> {
    tokio::time::timeout(
        Duration::from_secs(1),
        channel.fence_voice(scope, close_session, reason),
    )
    .await
    .map_err(|_| Status::deadline_exceeded("voice playback fence deadline"))?
    .map_err(|_| Status::unavailable("voice playback fence failed"))
}

async fn send_event(
    tx: &mpsc::Sender<Result<SessionEvent, Status>>,
    lease: &SessionLease,
    boot: Uuid,
    epoch: u64,
    payload: session_event::Payload,
) -> Result<(), Status> {
    let event = SessionEvent {
        session_id: lease.id.to_string(),
        device_boot_id: boot.to_string(),
        generation_epoch: epoch,
        payload: Some(payload),
    };
    tokio::select! {
        biased;
        _ = lease.cancel.cancelled() => Err(Status::cancelled("voice session cancelled")),
        result = tokio::time::timeout(Duration::from_secs(1), tx.send(Ok(event))) => {
            result.map_err(|_| Status::resource_exhausted("voice control receiver stalled"))?
                .map_err(|_| Status::cancelled("voice receiver closed"))
        }
    }
}

async fn accept_capture(
    frame: DeviceFrame,
    window: &mut CaptureWindow,
    credentials: &mut SessionCredentials,
    output: Option<&VoiceOutput>,
    policy: &mut CapturePolicy,
) -> Result<bool, Status> {
    // Only an actual peer frame refreshes transport liveness. Internal
    // opportunity requests must not keep a stalled capture session alive.
    credentials.last_frame_at = tokio::time::Instant::now();
    match frame.payload {
        Some(device_frame::Payload::Audio(frame)) => {
            let suppressed = policy.suppress(output, frame.speech_active);
            if suppressed {
                window.finish_utterance();
            }
            window
                .push(
                    frame.sequence,
                    frame.sample_offset,
                    &frame.pcm_s16le,
                    frame.speech_active,
                    std::time::SystemTime::now(),
                )
                .map_err(|error| Status::invalid_argument(error.to_string()))?;
            if suppressed {
                // Keep real sequence, sample, speech revision and receiver time;
                // discard only the utterance bytes, never the capture stream.
                window.finish_utterance();
            } else if frame.speech_active
                && let Some(output) = output
            {
                output
                    .near_end_speech(frame.sequence)
                    .map_err(|_| Status::unavailable("original output pause queue unavailable"))?;
            }
            Ok(true)
        }
        Some(device_frame::Payload::Close(_)) => Ok(false),
        Some(device_frame::Payload::RefreshAuth(refresh)) => {
            credentials.refresh(refresh.bearer_token).await?;
            Ok(true)
        }
        _ => Err(Status::invalid_argument(
            "unexpected voice frame after handshake",
        )),
    }
}

async fn drain_pending<S>(
    stream: &mut S,
    window: &mut CaptureWindow,
    credentials: &mut SessionCredentials,
    output: Option<&VoiceOutput>,
    policy: &mut CapturePolicy,
) -> Result<bool, Status>
where
    S: Stream<Item = Result<DeviceFrame, Status>> + Unpin,
{
    for drained in 0..MAX_PENDING_FRAMES {
        match stream.next().now_or_never() {
            None => break,
            Some(None) => return Ok(false),
            Some(Some(frame)) => {
                if !accept_capture(frame?, window, credentials, output, policy).await? {
                    return Ok(false);
                }
                if drained == MAX_PENDING_FRAMES - 1 {
                    return Err(Status::resource_exhausted(
                        "capture backlog exceeded semantic commit budget",
                    ));
                }
            }
        }
    }
    Ok(true)
}

async fn run_session<S>(
    stream: S,
    tx: &mpsc::Sender<Result<SessionEvent, Status>>,
    lease: &SessionLease,
    boot: Uuid,
    detector: Arc<dyn TurnDetector>,
    input: VoiceInputHandle,
    mut credentials: SessionCredentials,
    output_runtime: OutputRuntime,
    auth_recheck_interval: Duration,
) -> Result<(), Status>
where
    S: Stream<Item = Result<DeviceFrame, Status>> + Unpin,
{
    let source = credentials.auth.admit_source(&lease.channel).await?;
    let channel = source.channel();
    credentials.source = Some(source);
    let result = tokio::select! {
        biased;
        _ = lease.cancel.cancelled() => Ok(()),
        result = run_session_inner(
        stream,
        tx,
        lease,
        boot,
        detector,
        input,
        credentials,
        output_runtime,
        auth_recheck_interval,
    )
    => result,
    };
    lease.cancel.cancel();
    let closed = fence_output(
        &channel,
        VoiceScope {
            session_id: lease.id,
            device_boot_id: boot,
            generation_epoch: 1,
        },
        true,
        "voice_session_closed",
    )
    .await;
    if let Err(error) = &closed {
        tracing::warn!(%error, "voice close fence failed");
    }
    result.and(closed)
}

async fn run_session_inner<S>(
    mut stream: S,
    tx: &mpsc::Sender<Result<SessionEvent, Status>>,
    lease: &SessionLease,
    boot: Uuid,
    detector: Arc<dyn TurnDetector>,
    input: VoiceInputHandle,
    mut credentials: SessionCredentials,
    output_runtime: OutputRuntime,
    auth_recheck_interval: Duration,
) -> Result<(), Status>
where
    S: Stream<Item = Result<DeviceFrame, Status>> + Unpin,
{
    let source = credentials
        .source
        .as_ref()
        .expect("source admitted before session loop");
    let channel = source.channel();
    let owner_user_id = source.user_id;
    let source_privacy_epoch = source.privacy_epoch;
    let source_relationship_epoch = source.relationship_epoch;
    // A channel can be replaced between the initial handshake authentication
    // and admission. Verify the credential and pinned source together before
    // allowing capture or model work.
    credentials.revalidate().await?;
    let initial_owner = lease.slot.owner.lock().await;
    if lease.cancel.is_cancelled()
        || !initial_owner
            .as_ref()
            .is_some_and(|(id, _)| *id == lease.id)
    {
        return Err(Status::cancelled(
            "original voice session replaced before Ready",
        ));
    }
    let mut initiatives = output_runtime
        .initiatives
        .clone()
        .map(|(handle, context)| initiative::Session::new(handle, context, lease, boot));
    drop(initial_owner);
    send_event(
        tx,
        lease,
        boot,
        1,
        session_event::Payload::Ready(voice::SessionReady {
            protocol_version: VOICE_CONTRACT_VERSION,
            software_hold: true,
            max_hold_ms: output_runtime.maximum_hold_ms,
            sample_rate_hz: 16_000,
            channels: 1,
            max_frame_samples: MAX_FRAME_SAMPLES as u32,
            initiative_admission: initiatives.is_some(),
            initiative_capture_gate_version: output_runtime.initiative_capture_gate_version,
        }),
    )
    .await?;
    credentials.last_frame_at = tokio::time::Instant::now();
    let mut window = CaptureWindow::default();
    let mut capture_policy = CapturePolicy {
        barge_in_enabled: output_runtime.barge_in_enabled,
        wait_for_silence: false,
    };
    let mut controller = TurnController::default();
    let mut detection: Option<Detection> = None;
    let mut through_sequence = 0;
    let mut response_cancel = lease.cancel.child_token();
    let mut output: Option<VoiceOutput> = None;
    let mut retry_at: Option<tokio::time::Instant> = None;
    let mut busy_deadline: Option<tokio::time::Instant> = None;
    let mut auth_recheck = tokio::time::interval_at(
        tokio::time::Instant::now() + auth_recheck_interval,
        auth_recheck_interval,
    );
    auth_recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if lease.cancel.is_cancelled() || tx.is_closed() {
            return Ok(());
        }
        if detection.is_none()
            && retry_at.is_none()
            && let Some(audio) = window.take_ready()
        {
            through_sequence = window.through_sequence();
            let id = controller.begin_detection(window.revision());
            let detector = detector.clone();
            let cancel = lease.cancel.child_token();
            detection = Some(Box::pin(async move {
                detector.detect(id, &audio, &cancel).await
            }));
        }
        tokio::select! {
            _ = lease.cancel.cancelled() => return Ok(()),
            _ = tx.closed() => return Ok(()),
            request = async { initiatives.as_mut().expect("select guard").receiver.recv().await }, if initiatives.is_some() => {
                if let Some(request) = request {
                    initiatives.as_mut().expect("initiative receiver").handle(
                        request, &mut stream, tx, lease, &mut credentials, &mut window,
                        &mut capture_policy, &mut controller,
                        detection.is_some() || retry_at.is_some() || busy_deadline.is_some(),
                        &mut output, &mut response_cancel, &output_runtime,
                    ).await;
                }
            },
            _ = auth_recheck.tick() => {
                // Refresh already queued with capture takes precedence over a
                // periodic check of the old token. This drain is also bounded.
                if !drain_pending(&mut stream, &mut window, &mut credentials, output.as_ref(), &mut capture_policy).await? { return Ok(()); }
                credentials.revalidate().await?;
            },
            _ = async { tokio::time::sleep_until(retry_at.expect("select guard")).await }, if retry_at.is_some() => {
                retry_at = None;
            }
            frame = tokio::time::timeout_at(credentials.last_frame_at + STREAM_IDLE, stream.next()) => {
                let frame = frame.map_err(|_| Status::deadline_exceeded("voice capture stalled"))?;
                let Some(frame) = frame else { return Ok(()); };
                if !accept_capture(frame?, &mut window, &mut credentials, output.as_ref(), &mut capture_policy).await? { return Ok(()); }
            }
            result = async { detection.as_mut().expect("select guard").await }, if detection.is_some() => {
                detection = None;
                if matches!(&result, Err(DetectionError::Busy)) {
                    let now = tokio::time::Instant::now();
                    let deadline = *busy_deadline.get_or_insert(now + Duration::from_secs(3));
                    if now >= deadline {
                        return Err(Status::resource_exhausted("turn model remained busy for 3 seconds"));
                    }
                    window.retry_after_busy();
                    retry_at = Some(now + Duration::from_millis(100));
                    continue;
                }
                busy_deadline = None;
                let observation = result.map_err(|error| match error {
                    DetectionError::Busy => Status::resource_exhausted("turn model busy"),
                    DetectionError::Timeout => Status::deadline_exceeded("turn model deadline"),
                    DetectionError::Cancelled => Status::cancelled("turn model cancelled"),
                    _ => Status::unavailable("turn model failed"),
                })?;
                // Give already-arrived speech a chance to invalidate this
                // snapshot before committing a response. The drain is bounded;
                // a producer that never yields must not monopolize the runtime.
                if !drain_pending(&mut stream, &mut window, &mut credentials, output.as_ref(), &mut capture_policy).await? { return Ok(()); }
                // Revalidation may revoke the original source but cannot replace
                // it. Drain again after that await before consuming freshness.
                if observation.state != TurnState::Incomplete {
                    credentials.revalidate().await?;
                    if !drain_pending(&mut stream, &mut window, &mut credentials, output.as_ref(), &mut capture_policy).await? { return Ok(()); }
                }
                if !controller.accept_evidence(&observation, window.revision()) { continue; }
                let owner = lease.slot.owner.lock().await;
                if lease.cancel.is_cancelled() || !owner.as_ref().is_some_and(|(id, _)| *id == lease.id) {
                    return Ok(());
                }
                let (response_enqueued, output_decision) = match semantic_request(&observation) {
                    SemanticRequest::Listen => (false, voice::OutputDecision::Unchanged),
                    SemanticRequest::Pause => {
                        let requested = output.as_ref().filter(|output| output.is_alive())
                            .map(|output| output.explicit_wait(observation.id.request_id, through_sequence))
                            .transpose().map_err(|_| Status::unavailable("original output pause queue unavailable"))?
                            .unwrap_or(false);
                        window.finish_utterance();
                        (false, if requested { voice::OutputDecision::PauseRequested } else { voice::OutputDecision::Unchanged })
                    },
                    SemanticRequest::Resume(kind) => {
                        let requested = output.as_ref().filter(|output| output.is_alive())
                            .map(|output| output.request_resume(ResumeAuthorization {
                                kind, detection_id: observation.id.request_id, through_sequence,
                            }))
                            .transpose().map_err(|_| Status::unavailable("original output resume queue unavailable"))?
                            .unwrap_or(false);
                        window.finish_utterance();
                        // This is only a request. The supervised original owner
                        // waits for the exact receipt before releasing production.
                        (false, if requested { voice::OutputDecision::ResumeRequested } else { voice::OutputDecision::Unchanged })
                    },
                    SemanticRequest::Cancel => {
                        if let Some(output) = output.take() { output.retire(); }
                        response_cancel.cancel();
                        let generation_epoch = controller.invalidate().map_err(|_| Status::out_of_range("voice epoch exhausted"))?;
                        fence_output(&channel,
                            VoiceScope { session_id: lease.id, device_boot_id: boot, generation_epoch },
                            false, "user_stopped_original_output").await?;
                        window.finish_utterance();
                        (false, voice::OutputDecision::Cancelled)
                    },
                    SemanticRequest::Respond(transcript) => {
                        if let Some(output) = output.take() { output.retire(); }
                        response_cancel.cancel();
                        let generation_epoch = controller.invalidate().map_err(|_| Status::out_of_range("voice epoch exhausted"))?;
                        let scope = VoiceScope { session_id: lease.id, device_boot_id: boot, generation_epoch };
                        fence_output(&channel, scope, false, "voice_turn_replaced").await?;
                        if lease.cancel.is_cancelled() { return Ok(()); }
                        response_cancel = lease.cancel.child_token();
                        let request_id = Uuid::new_v4();
                        let producer = output_runtime.delivery.begin_voice_output(
                            owner_user_id, request_id, scope, channel.clone(), response_cancel.clone(),
                            Some(output_runtime.hold_budget),
                        ).map_err(|_| Status::resource_exhausted("voice output supervision unavailable"))?;
                        let current_output = producer.output();
                        input.try_submit(RecognizedVoiceInput {
                            request_id, output: Some(RecognizedVoiceOutput { producer, channel: channel.clone() }),
                            channel_id: lease.channel.clone(), user_id: owner_user_id.to_string(), transcript,
                            session_id: lease.id, device_boot_id: boot, generation_epoch, cancel: response_cancel.clone(),
                            source_received_at: window.source_received_at()
                                .ok_or_else(|| Status::internal("complete utterance has no receiver timestamp"))?.into(),
                            source_privacy_epoch,
                            source_relationship_epoch,
                        }).map_err(|_| Status::resource_exhausted("voice processing capacity reached"))?;
                        // Retain only the control handle. The actual queue,
                        // pipeline and media owners determine output lifetime.
                        output = Some(current_output);
                        window.finish_utterance();
                        (true, voice::OutputDecision::Unchanged)
                    },
                };
                drop(owner);
                send_event(tx, lease, boot, controller.generation_epoch(), session_event::Payload::Evidence(voice::TurnEvidence {
                    request_id: observation.id.request_id.to_string(), through_sequence,
                    transcript: observation.transcript, state: match observation.state {
                        TurnState::Complete => voice::TurnState::Complete,
                        TurnState::Incomplete => voice::TurnState::Incomplete,
                        TurnState::Backchannel => voice::TurnState::Backchannel,
                        TurnState::Wait => voice::TurnState::Wait,
                    } as i32,
                    response_enqueued, inference_ms: observation.inference_ms, output_decision: output_decision as i32,
                })).await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::ChannelRegistry;
    use crate::modules::delivery::repository::DeliveryRepository;
    use crate::modules::delivery::transport::DeliveryTransport as _;
    use crate::modules::interaction::voice_input;
    use alia_core::voice::{DetectionId, ModelIdentity, PcmWindow};
    use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
    use std::panic::AssertUnwindSafe;
    use tokio::sync::oneshot;

    struct ModelCall(
        DetectionId,
        oneshot::Sender<Result<TurnObservation, DetectionError>>,
    );
    struct ControlledDetector(mpsc::Sender<ModelCall>);
    #[async_trait::async_trait]
    impl TurnDetector for ControlledDetector {
        async fn detect(
            &self,
            id: DetectionId,
            _audio: &PcmWindow,
            cancel: &CancellationToken,
        ) -> Result<TurnObservation, DetectionError> {
            let (tx, rx) = oneshot::channel();
            self.0
                .send(ModelCall(id, tx))
                .await
                .map_err(|_| DetectionError::Cancelled)?;
            tokio::select! {
                _ = cancel.cancelled() => Err(DetectionError::Cancelled),
                result = rx => result.map_err(|_| DetectionError::Cancelled)?,
            }
        }
    }
    impl ModelCall {
        fn answer(self, state: TurnState) {
            self.answer_text(state, "sample");
        }
        fn answer_text(self, state: TurnState, transcript: &str) {
            self.1
                .send(Ok(TurnObservation {
                    id: self.0,
                    state,
                    transcript: transcript.into(),
                    inference_ms: 1.0,
                    model: ModelIdentity {
                        source: "injected-test".into(),
                        revision: "1".into(),
                        device: "test".into(),
                    },
                }))
                .unwrap();
        }
    }

    fn evidence_payload(event: &SessionEvent) -> &voice::TurnEvidence {
        let Some(session_event::Payload::Evidence(evidence)) = &event.payload else {
            panic!("expected semantic evidence");
        };
        evidence
    }

    #[test]
    fn paired_v2_requires_explicit_bounded_software_hold_capabilities() {
        let mut open = voice::OpenSession {
            protocol_version: VOICE_CONTRACT_VERSION,
            software_hold: true,
            max_hold_ms: MAX_HOLD_MS,
            ..Default::default()
        };
        assert_eq!(negotiated_hold_limit(&open).unwrap(), MAX_HOLD_MS);
        open.protocol_version = 1;
        assert_eq!(
            negotiated_hold_limit(&open).unwrap_err().code(),
            tonic::Code::FailedPrecondition
        );
        open.protocol_version = VOICE_CONTRACT_VERSION;
        open.software_hold = false;
        assert!(negotiated_hold_limit(&open).is_err());
        open.software_hold = true;
        for budget in [0, MAX_HOLD_MS + 1, u64::MAX] {
            open.max_hold_ms = budget;
            assert!(negotiated_hold_limit(&open).is_err());
        }
        open.max_hold_ms = 1;
        assert_eq!(negotiated_hold_limit(&open).unwrap(), 1);
    }

    #[test]
    fn semantic_controls_match_whole_complete_utterances_not_substrings() {
        let observation = |state, transcript: &str| TurnObservation {
            id: DetectionId::new(1),
            state,
            transcript: transcript.into(),
            inference_ms: 1.0,
            model: ModelIdentity {
                source: "test".into(),
                revision: "1".into(),
                device: "test".into(),
            },
        };
        for phrase in [
            "继续",
            " 请继续！ ",
            "Continue.",
            "please   continue",
            "接着说",
        ] {
            assert_eq!(
                semantic_request(&observation(TurnState::Complete, phrase)),
                SemanticRequest::Resume(ResumeKind::ExplicitContinue)
            );
        }
        for phrase in [
            "继续讲这个问题",
            "为什么你说 continue",
            "\"continue\"",
            "stop sign 是什么意思",
        ] {
            assert_eq!(
                semantic_request(&observation(TurnState::Complete, phrase)),
                SemanticRequest::Respond(phrase.into())
            );
        }
        assert_eq!(
            semantic_request(&observation(TurnState::Complete, "不要继续")),
            SemanticRequest::Cancel
        );
        assert_eq!(
            semantic_request(&observation(TurnState::Incomplete, "继续")),
            SemanticRequest::Listen
        );
        assert_eq!(
            semantic_request(&observation(TurnState::Wait, "先停一下")),
            SemanticRequest::Pause
        );
        assert_eq!(
            semantic_request(&observation(TurnState::Backchannel, "嗯")),
            SemanticRequest::Resume(ResumeKind::FreshBackchannel)
        );
    }

    struct AuthChannel {
        revoked: std::sync::atomic::AtomicBool,
        privacy_epoch: std::sync::atomic::AtomicI64,
        relationship_epoch: std::sync::atomic::AtomicI64,
        required_token: std::sync::Mutex<Option<String>>,
        checked: std::sync::Mutex<Vec<String>>,
        fences: std::sync::Mutex<Vec<(VoiceScope, bool)>>,
        next_verification: std::sync::Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    }
    impl Default for AuthChannel {
        fn default() -> Self {
            Self {
                revoked: false.into(),
                privacy_epoch: 7.into(),
                relationship_epoch: 11.into(),
                required_token: Default::default(),
                checked: Default::default(),
                fences: Default::default(),
                next_verification: Default::default(),
            }
        }
    }
    impl crate::modules::platform::lifecycle::ModuleLifecycle for AuthChannel {
        fn name(&self) -> &'static str {
            "signed_test_device"
        }
    }
    impl AuthChannel {
        async fn retire_model_scope(
            &self,
            target: &crate::modules::delivery::transport::DeliveryTarget,
            _: &str,
        ) -> anyhow::Result<()> {
            assert!(
                target.voice.is_some_and(|scope| scope.is_valid()),
                "must stop only original output scope"
            );
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl crate::modules::delivery::transport::DeliveryTransport for AuthChannel {
        fn owner_user_id(&self) -> Option<Uuid> {
            Some(Uuid::from_u128(7))
        }
        fn channel_id(&self) -> &str {
            "device"
        }

        async fn retire_voice_output(
            &self,
            request: &alia_core::output_control::OutputRetirementRequest,
        ) -> anyhow::Result<alia_core::output_control::OutputRetirementReceipt> {
            request.validate()?;
            anyhow::ensure!(
                request.binding.is_none() && request.expected_backend.is_none(),
                "actor fixture never registers a native output"
            );
            anyhow::ensure!(
                self.fences
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(scope, close)| *scope == request.scope && !close),
                "unknown actor output scope"
            );
            self.retire_model_scope(
                &crate::modules::delivery::transport::DeliveryTarget {
                    user_id: Uuid::from_u128(7).to_string(),
                    voice: Some(request.scope),
                },
                "original actor output ended",
            )
            .await?;
            Ok(alia_core::output_control::OutputRetirementReceipt {
                scope: request.scope,
                binding: None,
                evidence: alia_core::output_control::OutputRetirementEvidence::NoNativeOwner,
            })
        }
    }

    #[async_trait::async_trait]
    impl crate::channels::adapter::ChannelAdapter for AuthChannel {
        async fn authenticated_source_epochs(
            &self,
        ) -> anyhow::Result<crate::modules::memory::source::SourceEpochs> {
            Ok(crate::modules::memory::source::SourceEpochs {
                privacy_epoch: self.privacy_epoch.load(std::sync::atomic::Ordering::SeqCst),
                relationship_epoch: self
                    .relationship_epoch
                    .load(std::sync::atomic::Ordering::SeqCst),
            })
        }

        async fn authenticated_owner(&self) -> anyhow::Result<Uuid> {
            Ok(Uuid::from_u128(7))
        }

        fn channel_type(&self) -> &'static str {
            "embedded"
        }
        fn capabilities(&self) -> crate::channels::adapter::ChannelCapabilities {
            Default::default()
        }

        async fn execute_action(
            &self,
            action: crate::channels::adapter::ChannelAction,
        ) -> anyhow::Result<serde_json::Value> {
            anyhow::ensure!(
                action.name == "verify_audio_ingress_auth",
                "unexpected action"
            );
            anyhow::ensure!(
                !self.revoked.load(std::sync::atomic::Ordering::SeqCst),
                "device binding revoked"
            );
            let token = action.params["token"].as_str().unwrap();
            let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
            alia_transport::verify_auth_token(
                token,
                key.verifying_key().as_bytes(),
                "device",
                alia_transport::EMBEDDED_INGRESS_SCOPE,
                alia_transport::DEFAULT_AUTH_MAX_SKEW,
            )?;
            if let Some(expected) = self.required_token.lock().unwrap().as_ref() {
                anyhow::ensure!(token == expected, "old test credential expired");
            }
            self.checked.lock().unwrap().push(token.to_owned());
            let pause = self.next_verification.lock().unwrap().take();
            if let Some((entered, release)) = pause {
                entered.send(()).unwrap();
                release.await?;
            }
            Ok(serde_json::json!({"authenticated": true}))
        }
        async fn fence_voice(&self, scope: VoiceScope, close: bool, _: &str) -> anyhow::Result<()> {
            self.fences.lock().unwrap().push((scope, close));
            Ok(())
        }
    }
    fn token(revision: u64) -> String {
        alia_transport::mint_auth_token(
            &ed25519_dalek::SigningKey::from_bytes(&[7; 32]),
            "device",
            alia_transport::EMBEDDED_INGRESS_SCOPE,
            Duration::from_secs(900 + revision),
        )
        .unwrap()
    }
    fn credentials(channels: Arc<ChannelRegistry>) -> SessionCredentials {
        let mut request = Request::new(());
        request.metadata_mut().insert(
            "authorization",
            format!("Bearer {}", token(0)).parse().unwrap(),
        );
        SessionCredentials {
            auth: VoiceIngressAuth::new(channels),
            request,
            channel: "device".into(),
            source: None,
            last_frame_at: tokio::time::Instant::now(),
        }
    }

    static DATABASE_CASES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);

    struct TestDatabase {
        db: DatabaseConnection,
        admin: DatabaseConnection,
        schema: String,
        _permit: tokio::sync::SemaphorePermit<'static>,
    }
    impl TestDatabase {
        async fn new() -> Self {
            let permit = DATABASE_CASES.acquire().await.unwrap();
            let url = std::env::var("ALIA_TEST_DATABASE_URL")
                .expect("voice output actor tests require disposable PostgreSQL");
            let mut options = ConnectOptions::new(url.clone());
            options.max_connections(1).sqlx_logging(false);
            let admin = Database::connect(options).await.unwrap();
            let schema = format!("voice_actor_{}", Uuid::new_v4().simple());
            admin
                .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
                .await
                .unwrap();
            let mut options = ConnectOptions::new(url);
            options
                .max_connections(4)
                .sqlx_logging(false)
                .set_schema_search_path(format!("{schema},public"));
            let db = Database::connect(options).await.unwrap();
            let database = Self {
                db,
                admin,
                schema,
                _permit: permit,
            };
            if let Err(error) = crate::modules::platform::migrations::run(&database.db).await {
                database.cleanup().await;
                panic!("voice actor migrations failed: {error:#}");
            }
            database
        }
        async fn cleanup(self) {
            self.db.close().await.unwrap();
            self.admin
                .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .await
                .unwrap();
            self.admin.close().await.unwrap();
        }
    }

    struct Harness {
        audio: mpsc::Sender<Result<DeviceFrame, Status>>,
        events: mpsc::Receiver<Result<SessionEvent, Status>>,
        calls: mpsc::Receiver<ModelCall>,
        input: mpsc::Receiver<RecognizedVoiceInput>,
        cancel: CancellationToken,
        task: Option<tokio::task::JoinHandle<Result<(), Status>>>,
        sequence: u64,
        channel: Arc<AuthChannel>,
        initiatives: Option<VoiceInitiativeHandle>,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            self.cancel.cancel();
            if let Some(task) = self.task.take() {
                task.abort();
            }
        }
    }

    async fn with_harness<F, Fut>(channel: Arc<AuthChannel>, test: F)
    where
        F: FnOnce(Harness) -> Fut,
        Fut: Future<Output = ()>,
    {
        with_harness_recheck(channel, AUTH_RECHECK, test).await;
    }

    async fn with_harness_recheck<F, Fut>(channel: Arc<AuthChannel>, interval: Duration, test: F)
    where
        F: FnOnce(Harness) -> Fut,
        Fut: Future<Output = ()>,
    {
        with_harness_runtime(channel, interval, false, test).await;
    }

    async fn with_harness_runtime<F, Fut>(
        channel: Arc<AuthChannel>,
        interval: Duration,
        initiatives_enabled: bool,
        test: F,
    ) where
        F: FnOnce(Harness) -> Fut,
        Fut: Future<Output = ()>,
    {
        // Existing interruption cases retain their original explicit contract.
        with_harness_policy(channel, interval, initiatives_enabled, true, test).await;
    }

    async fn with_harness_policy<F, Fut>(
        channel: Arc<AuthChannel>,
        interval: Duration,
        initiatives_enabled: bool,
        barge_in_enabled: bool,
        test: F,
    ) where
        F: FnOnce(Harness) -> Fut,
        Fut: Future<Output = ()>,
    {
        let database = TestDatabase::new().await;
        let cancel = CancellationToken::new();
        let delivery = DeliveryService::new(DeliveryRepository::new(database.db.clone()), &cancel);
        // All cases exercise the real recovery and supervisor path. Keep cleanup
        // outside the caught test future so panic cannot strand its schema.
        let result = AssertUnwindSafe(async {
            delivery.start().await.unwrap();
            let initiatives = initiatives_enabled.then(|| {
                (
                    VoiceInitiativeHandle::default(),
                    Arc::new(ContextService::new(
                        database.db.clone(),
                        Arc::new(crate::providers::embedding::NoopEmbedding),
                    )),
                )
            });
            test(
                Harness::with_channel(
                    channel,
                    cancel.clone(),
                    delivery.clone(),
                    interval,
                    initiatives,
                    barge_in_enabled,
                )
                .await,
            )
            .await;
        })
        .catch_unwind()
        .await;
        cancel.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(5), delivery.shutdown()).await;
        database.cleanup().await;
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
        stopped
            .expect("voice actor Delivery shutdown deadline")
            .unwrap();
    }

    impl Harness {
        /// Transport-only fixture for virtual clocks. The disconnected database
        /// fails any accidental SQL/output call; no Delivery worker is started.
        async fn idle_transport() -> Self {
            let cancel = CancellationToken::new();
            let delivery = DeliveryService::new(
                DeliveryRepository::new(DatabaseConnection::Disconnected),
                &cancel,
            );
            let context = Arc::new(ContextService::new(
                DatabaseConnection::Disconnected,
                Arc::new(crate::providers::embedding::NoopEmbedding),
            ));
            Self::with_channel(
                Arc::new(AuthChannel::default()),
                cancel,
                delivery,
                AUTH_RECHECK,
                Some((VoiceInitiativeHandle::default(), context)),
                true,
            )
            .await
        }

        async fn with_channel(
            channel: Arc<AuthChannel>,
            cancel: CancellationToken,
            delivery: Arc<DeliveryService>,
            auth_recheck_interval: Duration,
            initiatives: Option<(VoiceInitiativeHandle, Arc<ContextService>)>,
            barge_in_enabled: bool,
        ) -> Self {
            let lease = SessionLease::acquire(Arc::default(), "device".into(), &cancel)
                .await
                .unwrap();
            let (audio, rx) = mpsc::channel(32);
            let (tx, events) = mpsc::channel(8);
            let (call_tx, calls) = mpsc::channel(2);
            let detector = Arc::new(ControlledDetector(call_tx));
            let (input_handle, input) = voice_input::channel();
            let channels = Arc::new(ChannelRegistry::new());
            channels.register(channel.clone()).await.unwrap();
            let auth = credentials(channels.clone());
            let output_runtime = OutputRuntime {
                delivery: delivery.clone(),
                hold_budget: Duration::from_secs(60),
                barge_in_enabled,
                maximum_hold_ms: MAX_HOLD_MS,
                initiatives: initiatives.clone(),
                initiative_output_enabled: false,
                initiative_capture_gate_version:
                    alia_core::output_control::INITIATIVE_CAPTURE_GATE_VERSION,
            };
            let task = tokio::spawn(async move {
                run_session(
                    ReceiverStream::new(rx),
                    &tx,
                    &lease,
                    Uuid::new_v4(),
                    detector,
                    input_handle,
                    auth,
                    output_runtime,
                    auth_recheck_interval,
                )
                .await
            });
            let mut harness = Self {
                audio,
                events,
                calls,
                input,
                cancel,
                task: Some(task),
                sequence: 0,
                channel,
                initiatives: initiatives.map(|(handle, _)| handle),
            };
            assert!(matches!(
                harness.event().await.payload,
                Some(session_event::Payload::Ready(_))
            ));
            harness
        }
        async fn speech(&mut self) {
            for speech in [true, false, false, false, false, false, false, false] {
                self.audio
                    .send(Ok(DeviceFrame {
                        payload: Some(device_frame::Payload::Audio(voice::CaptureFrame {
                            sequence: self.sequence,
                            sample_offset: self.sequence * 1600,
                            pcm_s16le: vec![0; 3200],
                            speech_active: speech,
                        })),
                    }))
                    .await
                    .unwrap();
                self.sequence += 1;
            }
        }
        async fn silence(&mut self) {
            self.audio
                .send(Ok(DeviceFrame {
                    payload: Some(device_frame::Payload::Audio(voice::CaptureFrame {
                        sequence: self.sequence,
                        sample_offset: self.sequence * 1600,
                        pcm_s16le: vec![0; 3200],
                        speech_active: false,
                    })),
                }))
                .await
                .unwrap();
            self.sequence += 1;
        }
        async fn speech_frame(&mut self) {
            self.audio
                .send(Ok(DeviceFrame {
                    payload: Some(device_frame::Payload::Audio(voice::CaptureFrame {
                        sequence: self.sequence,
                        sample_offset: self.sequence * 1600,
                        pcm_s16le: vec![0; 3200],
                        speech_active: true,
                    })),
                }))
                .await
                .unwrap();
            self.sequence += 1;
        }
        async fn capture_barrier(&self, revision: u64) {
            // RefreshAuth shares the capture lane. Observing this distinct token
            // proves every preceding audio frame was consumed, without sleeps
            // or adding silence that would accidentally release suppression.
            let fresh = token(revision);
            self.refresh(fresh.clone()).await;
            tokio::time::timeout(Duration::from_secs(2), async {
                while !self.channel.checked.lock().unwrap().contains(&fresh) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("capture/auth barrier was not consumed");
        }
        async fn refresh(&self, token: String) {
            self.audio
                .send(Ok(DeviceFrame {
                    payload: Some(device_frame::Payload::RefreshAuth(voice::RefreshAuth {
                        bearer_token: token,
                    })),
                }))
                .await
                .unwrap();
        }
        async fn checked_token(&mut self, token: &str, count: usize) {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let checked = self
                        .channel
                        .checked
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|value| *value == token)
                        .count();
                    if checked >= count {
                        return;
                    }
                    self.silence().await;
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("refreshed credential was not revalidated");
        }
        async fn call(&mut self) -> ModelCall {
            tokio::time::timeout(Duration::from_secs(2), self.calls.recv())
                .await
                .unwrap()
                .unwrap()
        }
        async fn event(&mut self) -> SessionEvent {
            tokio::time::timeout(Duration::from_secs(2), self.events.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        }
        async fn join(&mut self) -> Result<(), Status> {
            tokio::time::timeout(Duration::from_secs(3), self.task.take().unwrap())
                .await
                .unwrap()
                .unwrap()
        }
        async fn finish(mut self) {
            self.cancel.cancel();
            self.join().await.unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn initiative_snapshot_polling_cannot_extend_capture_idle_deadline() {
        let mut h = Harness::idle_transport().await;
        let handle = h.initiatives.clone().unwrap();
        let owner = h.channel.owner_user_id().unwrap();
        assert!(handle.snapshot(owner, "device").await.is_err());
        let ready_at = tokio::time::Instant::now();
        // These mailbox requests reach the real capture actor, but no further
        // DeviceFrame arrives. Recreating timeout(STREAM_IDLE, next()) here
        // would postpone expiry until 18 seconds and fail the bounded join.
        for _ in 0..4 {
            tokio::time::advance(Duration::from_secs(2)).await;
            assert!(handle.snapshot(owner, "device").await.is_err());
            assert!(!h.task.as_ref().unwrap().is_finished());
        }
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            h.join().await.unwrap_err().code(),
            tonic::Code::DeadlineExceeded
        );
        assert_eq!(tokio::time::Instant::now() - ready_at, STREAM_IDLE);
        assert!(h.calls.try_recv().is_err());
        assert!(h.input.try_recv().is_err());
        assert!(handle.snapshot(owner, "device").await.is_err());
    }

    #[tokio::test]
    async fn initiative_closed_output_gate_cannot_advance_scope_or_read_context() {
        let mut h = Harness::idle_transport().await;
        let handle = h.initiatives.clone().unwrap();
        let owner = h.channel.owner_user_id().unwrap();
        h.silence().await;
        let opportunity = handle.snapshot(owner, "device").await.unwrap();
        let scope = opportunity.scope;
        let deadline = opportunity.expires_at;
        let cancel = CancellationToken::new();
        let error = handle
            .reserve(
                crate::modules::interaction::initiative::VoiceInitiativeRequest::from_permit(
                    crate::modules::companion::live::LiveAdmissionPermit::fixture(
                        opportunity,
                        crate::modules::memory::context::ContextBinding {
                            context_id: Uuid::new_v4(),
                            owner_id: owner,
                            request_id: Uuid::new_v4(),
                            source_event_id: None,
                            source_privacy_epoch: 7,
                        },
                        deadline,
                    ),
                    cancel.clone(),
                ),
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("initiative output admission is disabled")
        );
        assert!(cancel.is_cancelled());
        assert_eq!(handle.snapshot(owner, "device").await.unwrap().scope, scope);
        assert!(h.channel.fences.lock().unwrap().is_empty());
        assert!(h.calls.try_recv().is_err());
        assert!(h.input.try_recv().is_err());
        h.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn authenticated_refresh_frame_extends_transport_idle_but_not_capture_freshness() {
        let mut h = Harness::idle_transport().await;
        let handle = h.initiatives.clone().unwrap();
        let owner = h.channel.owner_user_id().unwrap();
        let ready_at = tokio::time::Instant::now();
        tokio::time::advance(Duration::from_secs(6)).await;
        let refreshed_token = token(1);
        h.refresh(refreshed_token.clone()).await;
        // The snapshot drains the real RefreshAuth frame and waits for its
        // authentication. It still cannot manufacture a capture opportunity.
        assert!(handle.snapshot(owner, "device").await.is_err());
        assert!(
            h.channel
                .checked
                .lock()
                .unwrap()
                .iter()
                .any(|value| *value == refreshed_token)
        );
        for elapsed in [5, 4] {
            tokio::time::advance(Duration::from_secs(elapsed)).await;
            assert!(handle.snapshot(owner, "device").await.is_err());
            assert!(!h.task.as_ref().unwrap().is_finished());
        }
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            h.join().await.unwrap_err().code(),
            tonic::Code::DeadlineExceeded
        );
        assert_eq!(
            tokio::time::Instant::now() - ready_at,
            Duration::from_secs(16)
        );
        assert!(h.calls.try_recv().is_err());
        assert!(h.input.try_recv().is_err());
    }

    #[tokio::test]
    async fn initiative_snapshot_requires_capture_and_never_allocates_output() {
        with_harness_runtime(
            Arc::new(AuthChannel::default()),
            AUTH_RECHECK,
            true,
            |mut h| async move {
                let handle = h.initiatives.clone().unwrap();
                let owner = h.channel.owner_user_id().unwrap();
                assert!(handle.snapshot(owner, "device").await.is_err());
                let before = chrono::Utc::now();
                h.silence().await;
                assert!(handle.snapshot(Uuid::new_v4(), "device").await.is_err());
                let snapshot = handle.snapshot(owner, "device").await.unwrap();
                assert_eq!(snapshot.scope.generation_epoch, 1);
                assert_eq!(snapshot.speech_revision, 0);
                assert_eq!(snapshot.through_sequence, 0);
                assert!(snapshot.last_capture_at >= before);
                assert!(snapshot.last_capture_at <= snapshot.observed_at);
                assert!(snapshot.expires_at - snapshot.observed_at <= chrono::Duration::seconds(5));
                assert!(h.channel.fences.lock().unwrap().is_empty());
                assert!(h.calls.try_recv().is_err());
                assert!(h.input.try_recv().is_err());
                h.finish().await;
                assert!(handle.snapshot(owner, "device").await.is_err());
            },
        )
        .await;
    }

    #[tokio::test]
    async fn initiative_snapshot_rechecks_speech_arriving_during_authentication() {
        with_harness_runtime(
            Arc::new(AuthChannel::default()),
            AUTH_RECHECK,
            true,
            |mut h| async move {
                let handle = h.initiatives.clone().unwrap();
                let owner = h.channel.owner_user_id().unwrap();
                h.silence().await;
                let (entered, entered_rx) = oneshot::channel();
                let (release, wait) = oneshot::channel();
                *h.channel.next_verification.lock().unwrap() = Some((entered, wait));
                let snapshot = tokio::spawn(async move { handle.snapshot(owner, "device").await });
                entered_rx.await.unwrap();
                h.speech().await;
                release.send(()).unwrap();
                assert!(snapshot.await.unwrap().is_err());
                assert!(h.input.try_recv().is_err());
                assert!(h.channel.fences.lock().unwrap().is_empty());
                h.finish().await;
            },
        )
        .await;
    }

    #[tokio::test]
    async fn initiative_snapshot_rejects_original_output_even_when_held() {
        with_harness_runtime(
            Arc::new(AuthChannel::default()),
            AUTH_RECHECK,
            true,
            |mut h| async move {
                let handle = h.initiatives.clone().unwrap();
                let owner = h.channel.owner_user_id().unwrap();
                h.speech().await;
                h.call().await.answer(TurnState::Complete);
                h.event().await;
                let original = h.input.recv().await.unwrap();
                h.silence().await;
                assert!(handle.snapshot(owner, "device").await.is_err());
                h.speech().await;
                h.call().await.answer(TurnState::Wait);
                h.event().await;
                assert!(
                    original
                        .output
                        .as_ref()
                        .unwrap()
                        .producer
                        .output()
                        .is_held()
                );
                assert!(handle.snapshot(owner, "device").await.is_err());
                assert!(!original.cancel.is_cancelled());
                h.finish().await;
            },
        )
        .await;
    }

    #[tokio::test]
    async fn disabled_barge_in_keeps_original_reply_and_accepts_the_next_fresh_turn() {
        with_harness_policy(
            Arc::new(AuthChannel::default()),
            AUTH_RECHECK,
            false,
            false,
            |mut h| async move {
                h.speech().await;
                h.call().await.answer(TurnState::Complete);
                assert_eq!(h.event().await.generation_epoch, 2);
                let original = h.input.recv().await.unwrap();
                let output = original.output.as_ref().unwrap().producer.output();
                let fences = h.channel.fences.lock().unwrap().clone();
                // No audio has been emitted: the original generation/producer owner
                // already blocks interruption, and keeps doing so across speech gaps.
                for revision in [1, 2] {
                    h.speech().await;
                    h.capture_barrier(revision).await;
                    assert!(output.is_alive() && !output.is_held());
                    assert!(!original.cancel.is_cancelled());
                    assert!(h.calls.try_recv().is_err() && h.input.try_recv().is_err());
                    assert!(h.events.try_recv().is_err());
                    assert_eq!(*h.channel.fences.lock().unwrap(), fences);
                }
                drop(original);
                assert!(!output.is_alive());
                h.speech().await;
                h.call().await.answer(TurnState::Complete);
                assert_eq!(h.event().await.generation_epoch, 3);
                let next = h.input.recv().await.unwrap();
                assert!(!next.cancel.is_cancelled());
                h.finish().await;
                assert!(next.cancel.is_cancelled());
            },
        )
        .await;
    }

    #[tokio::test]
    async fn disabled_barge_in_discards_speech_crossing_output_end_until_silence() {
        with_harness_policy(
            Arc::new(AuthChannel::default()),
            AUTH_RECHECK,
            false,
            false,
            |mut h| async move {
                h.speech().await;
                h.call().await.answer(TurnState::Complete);
                h.event().await;
                let original = h.input.recv().await.unwrap();
                h.speech_frame().await;
                h.capture_barrier(1).await;
                assert!(!original.cancel.is_cancelled());
                drop(original);
                // The same utterance continues after output finishes; its remaining
                // half must not become a new model request or generation.
                h.speech_frame().await;
                h.capture_barrier(2).await;
                assert!(h.calls.try_recv().is_err());
                for _ in 0..7 {
                    h.silence().await;
                }
                h.capture_barrier(3).await;
                assert!(h.calls.try_recv().is_err() && h.events.try_recv().is_err());
                h.speech().await;
                h.call().await.answer(TurnState::Complete);
                assert_eq!(h.event().await.generation_epoch, 3);
                assert!(!h.input.recv().await.unwrap().cancel.is_cancelled());
                h.finish().await;
            },
        )
        .await;
    }

    #[tokio::test]
    async fn disabled_barge_in_preserves_close_and_rejects_broken_capture_clocks() {
        for broken_clock in [false, true] {
            with_harness_policy(
                Arc::new(AuthChannel::default()),
                AUTH_RECHECK,
                false,
                false,
                |mut h| async move {
                    h.speech().await;
                    h.call().await.answer(TurnState::Complete);
                    h.event().await;
                    let original = h.input.recv().await.unwrap();
                    let payload = if broken_clock {
                        device_frame::Payload::Audio(voice::CaptureFrame {
                            sequence: h.sequence + 1,
                            sample_offset: h.sequence * 1600,
                            pcm_s16le: vec![0; 3200],
                            speech_active: true,
                        })
                    } else {
                        device_frame::Payload::Close(voice::CloseSession {})
                    };
                    h.audio
                        .send(Ok(DeviceFrame {
                            payload: Some(payload),
                        }))
                        .await
                        .unwrap();
                    let result = h.join().await;
                    if broken_clock {
                        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
                    } else {
                        result.unwrap();
                    }
                    assert!(original.cancel.is_cancelled());
                    assert!(h.channel.fences.lock().unwrap().last().unwrap().1);
                    assert!(h.calls.try_recv().is_err());
                },
            )
            .await;
        }
    }

    #[tokio::test]
    async fn recognized_speech_bypasses_stt_and_backchannel_preserves_generation() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            h.speech().await;
            h.call().await.answer(TurnState::Complete);
            assert_eq!(h.event().await.generation_epoch, 2);
            let first = h.input.recv().await.unwrap();
            assert!(!first.request_id.is_nil());
            let producer = &first.output.as_ref().unwrap().producer;
            assert_eq!(producer.request_id(), first.request_id);
            assert_eq!(producer.scope().generation_epoch, first.generation_epoch);
            let pinned: Arc<dyn crate::modules::delivery::transport::DeliveryTransport> =
                h.channel.clone();
            assert!(Arc::ptr_eq(&producer.channel(), &pinned));
            assert!(
                producer.output().response_id().is_none(),
                "actor alone cannot invent a Response"
            );
            assert_eq!(first.transcript, "sample");
            assert_eq!(first.source_privacy_epoch, 7);
            assert_eq!(first.source_relationship_epoch, 11);
            h.speech().await;
            h.call().await.answer(TurnState::Backchannel);
            let backchannel = h.event().await;
            assert_eq!(backchannel.generation_epoch, 2);
            assert_eq!(
                evidence_payload(&backchannel).output_decision,
                voice::OutputDecision::ResumeRequested as i32
            );
            tokio::time::timeout(Duration::from_secs(2), producer.wait_open())
                .await
                .unwrap()
                .unwrap();
            assert!(!first.cancel.is_cancelled());
            assert!(h.input.try_recv().is_err());
            h.speech().await;
            h.call().await.answer(TurnState::Wait);
            let wait = h.event().await;
            assert_eq!(wait.generation_epoch, 2);
            assert_eq!(
                evidence_payload(&wait).output_decision,
                voice::OutputDecision::PauseRequested as i32
            );
            assert!(!first.cancel.is_cancelled());
            assert_eq!(
                first
                    .output
                    .as_ref()
                    .unwrap()
                    .producer
                    .output()
                    .hold_reason(),
                Some(alia_core::playback_control::HoldReason::ExplicitWait)
            );
            h.finish().await;
        })
        .await;
    }

    #[tokio::test]
    async fn explicit_wait_needs_complete_explicit_continue_and_stop_never_resumes() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            h.speech().await;
            h.call().await.answer(TurnState::Complete);
            h.event().await;
            let original = h.input.recv().await.unwrap();
            let producer = &original.output.as_ref().unwrap().producer;
            h.speech().await;
            h.call().await.answer(TurnState::Wait);
            let paused = h.event().await;
            assert_eq!(paused.generation_epoch, original.generation_epoch);
            assert_eq!(
                evidence_payload(&paused).output_decision,
                voice::OutputDecision::PauseRequested as i32
            );
            h.speech().await;
            h.call().await.answer(TurnState::Backchannel);
            let backchannel = h.event().await;
            assert_eq!(
                evidence_payload(&backchannel).output_decision,
                voice::OutputDecision::Unchanged as i32
            );
            assert_eq!(
                producer.output().hold_reason(),
                Some(alia_core::playback_control::HoldReason::ExplicitWait)
            );
            h.speech().await;
            h.call().await.answer_text(TurnState::Complete, "请继续。");
            let resumed = h.event().await;
            assert_eq!(resumed.generation_epoch, original.generation_epoch);
            assert_eq!(
                evidence_payload(&resumed).output_decision,
                voice::OutputDecision::ResumeRequested as i32
            );
            tokio::time::timeout(Duration::from_secs(2), producer.wait_open())
                .await
                .unwrap()
                .unwrap();
            assert!(!original.cancel.is_cancelled());
            assert!(h.input.try_recv().is_err());
            h.speech().await;
            h.call().await.answer_text(TurnState::Complete, "别说了");
            let stopped = h.event().await;
            assert_eq!(stopped.generation_epoch, original.generation_epoch + 1);
            assert_eq!(
                evidence_payload(&stopped).output_decision,
                voice::OutputDecision::Cancelled as i32
            );
            assert!(original.cancel.is_cancelled());
            h.speech().await;
            h.call().await.answer_text(TurnState::Complete, "继续");
            let gone = h.event().await;
            assert_eq!(gone.generation_epoch, stopped.generation_epoch);
            assert_eq!(
                evidence_payload(&gone).output_decision,
                voice::OutputDecision::Unchanged as i32
            );
            assert!(h.input.try_recv().is_err());
            h.finish().await;
        })
        .await;
    }

    #[tokio::test]
    async fn newer_captured_speech_rejects_old_continue_and_new_topic_replaces_original_owner() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            h.speech().await;
            h.call().await.answer(TurnState::Complete);
            h.event().await;
            let original = h.input.recv().await.unwrap();
            h.speech().await;
            let stale = h.call().await;
            h.speech().await;
            stale.answer_text(TurnState::Complete, "继续");
            let fresh = h.call().await;
            assert!(h.events.try_recv().is_err());
            assert!(
                original
                    .output
                    .as_ref()
                    .unwrap()
                    .producer
                    .output()
                    .is_held()
            );
            fresh.answer_text(TurnState::Complete, "继续讲这个问题");
            let event = h.event().await;
            assert_eq!(event.generation_epoch, original.generation_epoch + 1);
            assert!(evidence_payload(&event).response_enqueued);
            assert_eq!(
                evidence_payload(&event).output_decision,
                voice::OutputDecision::Unchanged as i32
            );
            let replacement = h.input.recv().await.unwrap();
            assert_ne!(replacement.request_id, original.request_id);
            assert_eq!(replacement.transcript, "继续讲这个问题");
            assert!(original.cancel.is_cancelled());
            assert!(!replacement.cancel.is_cancelled());
            h.finish().await;
        })
        .await;
    }

    #[tokio::test]
    async fn speech_queued_during_resume_revalidation_invalidates_the_original_detection() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            h.speech().await;
            h.call().await.answer(TurnState::Complete);
            h.event().await;
            let original = h.input.recv().await.unwrap();
            h.speech().await;
            let old = h.call().await;
            let (entered, checking) = oneshot::channel();
            let (release, wait) = oneshot::channel();
            *h.channel.next_verification.lock().unwrap() = Some((entered, wait));
            old.answer_text(TurnState::Complete, "继续");
            tokio::time::timeout(Duration::from_secs(2), checking)
                .await
                .unwrap()
                .unwrap();
            h.speech().await;
            release.send(()).unwrap();
            let fresh = h.call().await;
            assert!(
                h.events.try_recv().is_err(),
                "old Continue must not emit semantic permission"
            );
            assert!(
                original
                    .output
                    .as_ref()
                    .unwrap()
                    .producer
                    .output()
                    .is_held()
            );
            fresh.answer(TurnState::Wait);
            assert_eq!(
                evidence_payload(&h.event().await).output_decision,
                voice::OutputDecision::PauseRequested as i32
            );
            assert!(!original.cancel.is_cancelled());
            assert!(h.input.try_recv().is_err());
            h.finish().await;
        })
        .await;
    }

    #[tokio::test]
    async fn resume_revalidates_original_privacy_relationship_and_binding_before_enqueue() {
        for domain in 0..3 {
            with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
                h.speech().await;
                h.call().await.answer(TurnState::Complete);
                h.event().await;
                let original = h.input.recv().await.unwrap();
                h.speech().await;
                h.call().await.answer(TurnState::Wait);
                h.event().await;
                h.speech().await;
                let continuing = h.call().await;
                match domain {
                    0 => h
                        .channel
                        .privacy_epoch
                        .store(8, std::sync::atomic::Ordering::SeqCst),
                    1 => h
                        .channel
                        .relationship_epoch
                        .store(12, std::sync::atomic::Ordering::SeqCst),
                    _ => h
                        .channel
                        .revoked
                        .store(true, std::sync::atomic::Ordering::SeqCst),
                }
                continuing.answer_text(TurnState::Complete, "继续");
                let error = h.join().await.unwrap_err();
                assert_eq!(
                    error.code(),
                    if domain == 2 {
                        tonic::Code::Unauthenticated
                    } else {
                        tonic::Code::PermissionDenied
                    }
                );
                assert!(original.cancel.is_cancelled());
                assert!(h.input.try_recv().is_err());
                assert!(
                    h.events.try_recv().is_err(),
                    "revoked source must not announce ResumeRequested"
                );
            })
            .await;
        }
    }

    #[tokio::test]
    async fn dropping_last_producer_ends_output_even_while_voice_session_stays_alive() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            h.speech().await;
            h.call().await.answer(TurnState::Complete);
            let admitted = h.event().await;
            let original = h.input.recv().await.unwrap();
            let control = original.output.as_ref().unwrap().producer.output();
            drop(original);
            assert!(!control.is_alive());
            h.speech().await;
            h.call().await.answer_text(TurnState::Complete, "继续");
            let no_output = h.event().await;
            assert_eq!(no_output.generation_epoch, admitted.generation_epoch);
            assert_eq!(
                evidence_payload(&no_output).output_decision,
                voice::OutputDecision::Unchanged as i32
            );
            assert!(h.input.try_recv().is_err());
            h.finish().await;
        })
        .await;
    }

    #[tokio::test]
    async fn saturated_recognized_queue_fails_closed_without_leaking_output_producer() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            for _ in 0..8 {
                h.speech().await;
                h.call().await.answer(TurnState::Complete);
                assert!(evidence_payload(&h.event().await).response_enqueued);
            }
            h.speech().await;
            h.call().await.answer(TurnState::Complete);
            assert_eq!(
                h.join().await.unwrap_err().code(),
                tonic::Code::ResourceExhausted
            );
            for _ in 0..8 {
                let queued = h.input.try_recv().unwrap();
                assert!(queued.cancel.is_cancelled());
                assert!(!queued.output.as_ref().unwrap().producer.output().is_alive());
            }
        })
        .await;
    }

    #[tokio::test]
    async fn speech_arriving_during_inference_rejects_old_snapshot_and_reevaluates() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            let received_after = chrono::Utc::now();
            h.speech().await;
            let old = h.call().await;
            let received_before = chrono::Utc::now();
            h.speech().await;
            tokio::time::timeout(Duration::from_secs(2), async {
                while h.audio.capacity() != h.audio.max_capacity() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            old.answer(TurnState::Complete);
            let current = h.call().await;
            assert!(h.input.try_recv().is_err());
            current.answer(TurnState::Complete);
            assert_eq!(h.event().await.generation_epoch, 2);
            let input = h.input.recv().await.unwrap();
            assert!(input.source_received_at >= received_after);
            assert!(input.source_received_at <= received_before);
            assert_eq!(input.source_privacy_epoch, 7);
            assert_eq!(input.source_relationship_epoch, 11);
            h.finish().await;
        })
        .await;
    }

    #[tokio::test]
    async fn capture_gap_fails_and_invalidates_already_enqueued_response() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            h.speech().await;
            h.call().await.answer(TurnState::Complete);
            h.event().await;
            let response = h.input.recv().await.unwrap();
            h.audio
                .send(Ok(DeviceFrame {
                    payload: Some(device_frame::Payload::Audio(voice::CaptureFrame {
                        sequence: 999,
                        sample_offset: 0,
                        pcm_s16le: vec![0; 2],
                        speech_active: true,
                    })),
                }))
                .await
                .unwrap();
            let error = h.join().await.unwrap_err();
            assert_eq!(error.code(), tonic::Code::InvalidArgument);
            assert!(response.cancel.is_cancelled());
        })
        .await;
    }

    #[tokio::test]
    async fn busy_model_retries_bounded_window_without_requiring_another_utterance() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            let received_after = chrono::Utc::now();
            h.speech().await;
            let busy = h.call().await;
            let received_before = chrono::Utc::now();
            busy.1.send(Err(DetectionError::Busy)).unwrap();
            h.call().await.answer(TurnState::Complete);
            assert_eq!(h.event().await.generation_epoch, 2);
            let input = h.input.recv().await.unwrap();
            assert!(input.source_received_at >= received_after);
            assert!(input.source_received_at <= received_before);
            assert_eq!(input.source_privacy_epoch, 7);
            assert_eq!(input.source_relationship_epoch, 11);
            h.finish().await;
        })
        .await;
    }

    #[tokio::test]
    async fn privacy_change_during_detection_retires_session_and_new_session_gets_new_epoch() {
        changed_epoch_during_detection_retires_session(true).await;
    }

    #[tokio::test]
    async fn relationship_change_during_detection_retires_session_and_new_session_gets_new_epoch() {
        changed_epoch_during_detection_retires_session(false).await;
    }

    async fn changed_epoch_during_detection_retires_session(privacy: bool) {
        let channel = Arc::new(AuthChannel::default());
        with_harness(channel.clone(), |mut old| async move {
            old.speech().await;
            let call = old.call().await;
            let channel = old.channel.clone();
            let (changed, domain, epoch) = if privacy {
                (&channel.privacy_epoch, "privacy", 8)
            } else {
                (&channel.relationship_epoch, "relationship", 12)
            };
            changed.store(epoch, std::sync::atomic::Ordering::SeqCst);
            call.answer(TurnState::Complete);
            let error = old.join().await.unwrap_err();
            assert_eq!(error.code(), tonic::Code::PermissionDenied);
            assert!(error.message().contains(&format!("{domain} epoch changed")));
            assert!(old.input.try_recv().is_err());
            assert!(channel.fences.lock().unwrap().last().unwrap().1);
        })
        .await;

        with_harness(channel, |mut fresh| async move {
            fresh.speech().await;
            fresh.call().await.answer(TurnState::Complete);
            fresh.event().await;
            let input = fresh.input.recv().await.unwrap();
            assert_eq!(input.source_privacy_epoch, if privacy { 8 } else { 7 });
            assert_eq!(
                input.source_relationship_epoch,
                if privacy { 11 } else { 12 }
            );
            fresh.finish().await;
        })
        .await;
    }

    #[tokio::test]
    async fn source_epoch_is_fixed_at_ready_before_first_audio_and_not_renewed_by_auth() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            h.channel
                .privacy_epoch
                .store(8, std::sync::atomic::Ordering::SeqCst);
            h.refresh(token(1)).await;
            let error = h.join().await.unwrap_err();
            assert_eq!(error.code(), tonic::Code::PermissionDenied);
            assert!(h.channel.fences.lock().unwrap().last().unwrap().1);
        })
        .await;
    }

    #[tokio::test]
    async fn privacy_change_retires_idle_session_and_cancels_queued_response() {
        with_harness_recheck(
            Arc::new(AuthChannel::default()),
            Duration::from_millis(100),
            |mut h| async move {
                h.speech().await;
                h.call().await.answer(TurnState::Complete);
                h.event().await;
                h.channel
                    .privacy_epoch
                    .store(8, std::sync::atomic::Ordering::SeqCst);
                let error = h.join().await.unwrap_err();
                assert_eq!(error.code(), tonic::Code::PermissionDenied);
                let queued = h.input.recv().await.unwrap();
                assert_eq!(queued.source_privacy_epoch, 7);
                assert!(queued.cancel.is_cancelled());
                assert!(h.channel.fences.lock().unwrap().last().unwrap().1);
            },
        )
        .await;
    }

    #[tokio::test]
    async fn continuously_ready_capture_does_not_starve_model_or_control() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            h.speech().await;
            let call = h.call().await;
            let audio = h.audio.clone();
            let mut sequence = h.sequence;
            let feeder = tokio::spawn(async move {
                loop {
                    let frame = DeviceFrame {
                        payload: Some(device_frame::Payload::Audio(voice::CaptureFrame {
                            sequence,
                            sample_offset: sequence * 1600,
                            pcm_s16le: vec![0; 3200],
                            speech_active: false,
                        })),
                    };
                    if audio.send(Ok(frame)).await.is_err() {
                        return;
                    }
                    sequence += 1;
                }
            });
            call.answer(TurnState::Backchannel);
            assert!(matches!(
                h.event().await.payload,
                Some(session_event::Payload::Evidence(_))
            ));
            h.finish().await;
            tokio::time::timeout(Duration::from_secs(2), feeder)
                .await
                .unwrap()
                .unwrap();
        })
        .await;
    }

    #[tokio::test]
    async fn replacing_a_session_cancels_the_old_owner_and_old_drop_preserves_new_owner() {
        let sessions = Arc::default();
        let cancel = CancellationToken::new();
        let old = SessionLease::acquire(Arc::clone(&sessions), "device".into(), &cancel)
            .await
            .unwrap();
        let new = SessionLease::acquire(Arc::clone(&sessions), "device".into(), &cancel)
            .await
            .unwrap();
        assert!(old.cancel.is_cancelled());
        drop(old);
        assert_eq!(new.slot.owner.lock().await.as_ref().unwrap().0, new.id);
        drop(new);
        assert!(sessions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn slow_control_blocks_same_channel_replacement_but_not_other_channels() {
        let sessions = Arc::default();
        let cancel = CancellationToken::new();
        let old = SessionLease::acquire(Arc::clone(&sessions), "a".into(), &cancel)
            .await
            .unwrap();
        let control = old.slot.owner.lock().await;
        let replacement = SessionLease::acquire(Arc::clone(&sessions), "a".into(), &cancel);
        tokio::pin!(replacement);
        assert!(futures::poll!(&mut replacement).is_pending());
        let other = SessionLease::acquire(Arc::clone(&sessions), "b".into(), &cancel)
            .await
            .unwrap();
        assert!(!other.cancel.is_cancelled());
        drop(control);
        let new = replacement.await.unwrap();
        assert!(old.cancel.is_cancelled());
        assert!(!new.cancel.is_cancelled());
    }
    #[tokio::test]
    async fn renewed_credentials_survive_repeated_periodic_revalidation() {
        with_harness_recheck(
            Arc::new(AuthChannel::default()),
            Duration::from_millis(100),
            |mut h| async move {
                for revision in 1..=3 {
                    let fresh = token(revision);
                    h.refresh(fresh.clone()).await;
                    // This is a short real-clock scheduling test. Signed token-age
                    // rejection is covered separately; do not advance live PG timers.
                    h.checked_token(&fresh, 1).await;
                    *h.channel.required_token.lock().unwrap() = Some(fresh.clone());
                    h.checked_token(&fresh, 5).await;
                    assert!(!h.task.as_ref().unwrap().is_finished());
                    // Allow the next explicitly authenticated credential to replace it.
                    *h.channel.required_token.lock().unwrap() = None;
                }
                h.finish().await;
            },
        )
        .await;
    }

    #[tokio::test]
    async fn binding_revocation_closes_on_periodic_revalidation() {
        with_harness_recheck(
            Arc::new(AuthChannel::default()),
            Duration::from_millis(100),
            |mut h| async move {
                let channel = h.channel.clone();
                channel
                    .revoked
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                let error = h.join().await.unwrap_err();
                assert_eq!(error.code(), tonic::Code::Unauthenticated);
                assert!(channel.fences.lock().unwrap().last().unwrap().1);
            },
        )
        .await;
    }

    #[tokio::test]
    async fn wrong_or_stale_signed_refresh_is_rejected_and_closes_scoped_output() {
        use base64::Engine;
        for invalid_case in 0..4 {
            with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
                let channel = h.channel.clone();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let claims = alia_transport::AuthTokenClaims {
                    version: 1,
                    server_id: if invalid_case == 2 {
                        "different-device"
                    } else {
                        "device"
                    }
                    .into(),
                    scope: if invalid_case == 3 {
                        "different-scope"
                    } else {
                        alia_transport::EMBEDDED_INGRESS_SCOPE
                    }
                    .into(),
                    iat: if invalid_case == 1 { now - 301 } else { now },
                    exp: now + 900,
                };
                let payload = serde_json::to_vec(&claims).unwrap();
                let key = ed25519_dalek::SigningKey::from_bytes(
                    &[if invalid_case == 0 { 9 } else { 7 }; 32],
                );
                let signature = alia_transport::sign_payload(&key, &payload);
                let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
                h.refresh(format!(
                    "{}.{}",
                    engine.encode(payload),
                    engine.encode(signature)
                ))
                .await;
                let error = h.join().await.unwrap_err();
                assert_eq!(error.code(), tonic::Code::Unauthenticated);
                assert!(channel.fences.lock().unwrap().last().unwrap().1);
            })
            .await;
        }
    }

    #[tokio::test]
    async fn complete_wait_keeps_owner_and_only_close_sends_terminal_fence() {
        with_harness(Arc::new(AuthChannel::default()), |mut h| async move {
            h.speech().await;
            h.call().await.answer(TurnState::Complete);
            let response = h.event().await;
            let input = h.input.recv().await.unwrap();
            h.speech().await;
            h.call().await.answer(TurnState::Wait);
            let wait = h.event().await;
            assert_eq!(wait.generation_epoch, response.generation_epoch);
            assert!(!input.cancel.is_cancelled());
            assert_eq!(
                evidence_payload(&wait).output_decision,
                voice::OutputDecision::PauseRequested as i32
            );
            let channel = h.channel.clone();
            h.finish().await;
            let fences = channel.fences.lock().unwrap();
            assert_eq!(fences.len(), 2);
            for (scope, _) in fences.iter() {
                assert_eq!(scope.session_id.to_string(), response.session_id);
                assert_eq!(scope.device_boot_id.to_string(), response.device_boot_id);
            }
            assert_eq!((fences[0].0.generation_epoch, fences[0].1), (2, false));
            assert!(fences[1].1);
            assert!(input.cancel.is_cancelled());
        })
        .await;
    }
}
