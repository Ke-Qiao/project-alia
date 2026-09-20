//! Authenticated continuous capture. One live session owns each device channel.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use alia_core::capture::{CaptureWindow, MAX_FRAME_SAMPLES};
use alia_core::delivery::VoiceScope;
use alia_core::turn::{TurnController, TurnDecision};
use alia_core::voice::{TurnObservation, TurnState};
use alia_proto::voice::voice_session_server::VoiceSession;
use alia_proto::voice::{self, DeviceFrame, SessionEvent, device_frame, session_event};
use futures::{FutureExt, Stream, StreamExt};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use super::audio_ingress::{AdmittedSource, ServerAudioIngressService};
use crate::channels::ChannelRegistry;
use crate::modules::interaction::turn_detector::{DetectionError, TurnDetector};
use crate::modules::interaction::voice_input::{RecognizedVoiceInput, VoiceInputHandle};

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
    auth: ServerAudioIngressService,
    channels: Arc<ChannelRegistry>,
    detector: Option<Arc<dyn TurnDetector>>,
    input: VoiceInputHandle,
    shutdown: CancellationToken,
    runtime_tasks: Arc<crate::module::lifecycle::RuntimeTasks>,
    sessions: Sessions,
}

impl VoiceSessionService {
    pub fn new(
        auth: ServerAudioIngressService,
        channels: Arc<ChannelRegistry>,
        detector: Option<Arc<dyn TurnDetector>>,
        input: VoiceInputHandle,
        shutdown: CancellationToken,
        runtime_tasks: Arc<crate::module::lifecycle::RuntimeTasks>,
    ) -> Self {
        Self {
            auth,
            channels,
            detector,
            input,
            shutdown,
            runtime_tasks,
            sessions: Arc::default(),
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
        if open.protocol_version != 1 {
            return Err(Status::failed_precondition(
                "unsupported voice protocol version",
            ));
        }
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
        let channels = self.channels.clone();
        let input = self.input.clone();
        self.runtime_tasks
            .try_spawn("voice_session", async move {
                let result = run_session(
                    stream,
                    &tx,
                    &lease,
                    boot,
                    detector,
                    input,
                    channels.clone(),
                    SessionCredentials {
                        auth,
                        request: auth_request,
                        channel: lease.channel.clone(),
                        source: None,
                    },
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
    auth: ServerAudioIngressService,
    request: Request<()>,
    channel: String,
    /// Captured once before Ready. Refresh may invalidate, never replace it.
    source: Option<AdmittedSource>,
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
    channels: &ChannelRegistry,
    channel_id: &str,
    scope: VoiceScope,
    close_session: bool,
    reason: &str,
) -> Result<(), Status> {
    let channel = channels
        .get(channel_id)
        .await
        .ok_or_else(|| Status::unavailable("voice output channel disappeared"))?;
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
) -> Result<bool, Status> {
    match frame.payload {
        Some(device_frame::Payload::Audio(frame)) => {
            window
                .push(
                    frame.sequence,
                    frame.sample_offset,
                    &frame.pcm_s16le,
                    frame.speech_active,
                    std::time::SystemTime::now(),
                )
                .map_err(|error| Status::invalid_argument(error.to_string()))?;
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
) -> Result<bool, Status>
where
    S: Stream<Item = Result<DeviceFrame, Status>> + Unpin,
{
    for drained in 0..MAX_PENDING_FRAMES {
        match stream.next().now_or_never() {
            None => break,
            Some(None) => return Ok(false),
            Some(Some(frame)) => {
                if !accept_capture(frame?, window, credentials).await? {
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
    channels: Arc<ChannelRegistry>,
    credentials: SessionCredentials,
) -> Result<(), Status>
where
    S: Stream<Item = Result<DeviceFrame, Status>> + Unpin,
{
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
        channels.clone(),
        credentials,
    )
    => result,
    };
    lease.cancel.cancel();
    let closed = fence_output(
        &channels,
        &lease.channel,
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
    channels: Arc<ChannelRegistry>,
    mut credentials: SessionCredentials,
) -> Result<(), Status>
where
    S: Stream<Item = Result<DeviceFrame, Status>> + Unpin,
{
    let source = credentials.auth.admit_source(&lease.channel).await?;
    let owner_user_id = source.user_id;
    let source_privacy_epoch = source.privacy_epoch;
    let source_relationship_epoch = source.relationship_epoch;
    credentials.source = Some(source);
    // A channel can be replaced between the initial handshake authentication
    // and admission. Verify the credential and pinned source together before
    // allowing capture or model work.
    credentials.revalidate().await?;
    send_event(
        tx,
        lease,
        boot,
        1,
        session_event::Payload::Ready(voice::SessionReady {
            protocol_version: 1,
            sample_rate_hz: 16_000,
            channels: 1,
            max_frame_samples: MAX_FRAME_SAMPLES as u32,
        }),
    )
    .await?;
    let mut window = CaptureWindow::default();
    let mut controller = TurnController::default();
    let mut detection: Option<Detection> = None;
    let mut through_sequence = 0;
    let mut response_cancel = lease.cancel.child_token();
    let mut retry_at: Option<tokio::time::Instant> = None;
    let mut busy_deadline: Option<tokio::time::Instant> = None;
    let mut auth_recheck =
        tokio::time::interval_at(tokio::time::Instant::now() + AUTH_RECHECK, AUTH_RECHECK);
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
            _ = auth_recheck.tick() => {
                // Refresh already queued with capture takes precedence over a
                // periodic check of the old token. This drain is also bounded.
                if !drain_pending(&mut stream, &mut window, &mut credentials).await? { return Ok(()); }
                credentials.revalidate().await?;
            },
            _ = async { tokio::time::sleep_until(retry_at.expect("select guard")).await }, if retry_at.is_some() => {
                retry_at = None;
            }
            frame = tokio::time::timeout(STREAM_IDLE, stream.next()) => {
                let frame = frame.map_err(|_| Status::deadline_exceeded("voice capture stalled"))?;
                let Some(frame) = frame else { return Ok(()); };
                if !accept_capture(frame?, &mut window, &mut credentials).await? { return Ok(()); }
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
                if !drain_pending(&mut stream, &mut window, &mut credentials).await? { return Ok(()); }
                let decision = controller.observe(&observation, window.revision()).map_err(|_| Status::out_of_range("voice epoch exhausted"))?;
                let response_enqueued = match decision {
                    TurnDecision::Stale => continue,
                    TurnDecision::Respond { generation_epoch, transcript } => {
                        credentials.revalidate().await?;
                        let owner = lease.slot.owner.lock().await;
                        if lease.cancel.is_cancelled() || !owner.as_ref().is_some_and(|(id, _)| *id == lease.id) {
                            return Ok(());
                        }
                        response_cancel.cancel();
                        fence_output(&channels, &lease.channel,
                            VoiceScope { session_id: lease.id, device_boot_id: boot, generation_epoch },
                            false, "voice_turn_replaced").await?;
                        if lease.cancel.is_cancelled() { return Ok(()); }
                        response_cancel = lease.cancel.child_token();
                        input.try_submit(RecognizedVoiceInput {
                            channel_id: lease.channel.clone(), user_id: owner_user_id.to_string(), transcript,
                            session_id: lease.id, device_boot_id: boot, generation_epoch, cancel: response_cancel.clone(),
                            source_received_at: window.source_received_at()
                                .ok_or_else(|| Status::internal("complete utterance has no receiver timestamp"))?.into(),
                            source_privacy_epoch,
                            source_relationship_epoch,
                        }).map_err(|_| Status::resource_exhausted("voice processing capacity reached"))?;
                        drop(owner);
                        window.finish_utterance();
                        true
                    },
                    TurnDecision::ContinueListening => false,
                    TurnDecision::Backchannel => { window.finish_utterance(); false },
                    TurnDecision::Wait => {
                        // Explicit hold: stop current generation. Resuming from a
                        // presented playback cursor needs the delivery contract.
                        response_cancel.cancel();
                        fence_output(&channels, &lease.channel,
                            VoiceScope { session_id: lease.id, device_boot_id: boot, generation_epoch: controller.generation_epoch() },
                            false, "user_requested_wait").await?;
                        window.finish_utterance();
                        false
                    },
                };
                send_event(tx, lease, boot, controller.generation_epoch(), session_event::Payload::Evidence(voice::TurnEvidence {
                    request_id: observation.id.request_id.to_string(), through_sequence,
                    transcript: observation.transcript, state: match observation.state {
                        TurnState::Complete => voice::TurnState::Complete,
                        TurnState::Incomplete => voice::TurnState::Incomplete,
                        TurnState::Backchannel => voice::TurnState::Backchannel,
                        TurnState::Wait => voice::TurnState::Wait,
                    } as i32,
                    response_enqueued, inference_ms: observation.inference_ms,
                })).await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::interaction::voice_input;
    use alia_core::voice::{DetectionId, ModelIdentity, PcmWindow};
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
            self.1
                .send(Ok(TurnObservation {
                    id: self.0,
                    state,
                    transcript: "sample".into(),
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

    struct AuthChannel {
        revoked: std::sync::atomic::AtomicBool,
        privacy_epoch: std::sync::atomic::AtomicI64,
        relationship_epoch: std::sync::atomic::AtomicI64,
        required_token: std::sync::Mutex<Option<String>>,
        checked: std::sync::Mutex<Vec<String>>,
        fences: std::sync::Mutex<Vec<(VoiceScope, bool)>>,
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
            }
        }
    }
    impl crate::module::ModuleLifecycle for AuthChannel {
        fn name(&self) -> &'static str {
            "signed_test_device"
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

        fn owner_user_id(&self) -> Option<Uuid> {
            Some(Uuid::from_u128(7))
        }

        fn channel_type(&self) -> &'static str {
            "embedded"
        }
        fn channel_id(&self) -> &str {
            "device"
        }
        fn capabilities(&self) -> crate::channels::adapter::ChannelCapabilities {
            Default::default()
        }
        async fn send_message(
            &self,
            _: &crate::channels::adapter::ChannelTarget,
            _: &str,
        ) -> anyhow::Result<()> {
            Ok(())
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
            Ok(serde_json::json!({"authenticated": true}))
        }
        async fn fence_voice(&self, scope: VoiceScope, close: bool, _: &str) -> anyhow::Result<()> {
            self.fences.lock().unwrap().push((scope, close));
            Ok(())
        }
        async fn stop_current_playback(
            &self,
            _: &crate::channels::adapter::ChannelTarget,
            _: &str,
        ) -> anyhow::Result<()> {
            panic!("voice session must never dispatch an unscoped stop");
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
            auth: ServerAudioIngressService::new(
                crate::event::EventBus::new(8),
                channels,
                "device",
            ),
            request,
            channel: "device".into(),
            source: None,
        }
    }

    struct Harness {
        audio: mpsc::Sender<Result<DeviceFrame, Status>>,
        events: mpsc::Receiver<Result<SessionEvent, Status>>,
        calls: mpsc::Receiver<ModelCall>,
        input: mpsc::Receiver<RecognizedVoiceInput>,
        cancel: CancellationToken,
        task: tokio::task::JoinHandle<Result<(), Status>>,
        sequence: u64,
        channel: Arc<AuthChannel>,
    }

    impl Harness {
        async fn new() -> Self {
            Self::with_channel(Arc::new(AuthChannel::default())).await
        }

        async fn with_channel(channel: Arc<AuthChannel>) -> Self {
            let cancel = CancellationToken::new();
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
            let task = tokio::spawn(async move {
                run_session(
                    ReceiverStream::new(rx),
                    &tx,
                    &lease,
                    Uuid::new_v4(),
                    detector,
                    input_handle,
                    channels,
                    auth,
                )
                .await
            });
            let mut harness = Self {
                audio,
                events,
                calls,
                input,
                cancel,
                task,
                sequence: 0,
                channel,
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
        async fn finish(self) {
            self.cancel.cancel();
            tokio::time::timeout(Duration::from_secs(2), self.task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn recognized_speech_bypasses_stt_and_backchannel_preserves_generation() {
        let mut h = Harness::new().await;
        h.speech().await;
        h.call().await.answer(TurnState::Complete);
        assert_eq!(h.event().await.generation_epoch, 2);
        let first = h.input.recv().await.unwrap();
        assert_eq!(first.transcript, "sample");
        assert_eq!(first.source_privacy_epoch, 7);
        assert_eq!(first.source_relationship_epoch, 11);
        h.speech().await;
        h.call().await.answer(TurnState::Backchannel);
        assert_eq!(h.event().await.generation_epoch, 2);
        assert!(!first.cancel.is_cancelled());
        assert!(h.input.try_recv().is_err());
        h.speech().await;
        h.call().await.answer(TurnState::Wait);
        assert_eq!(h.event().await.generation_epoch, 3);
        assert!(first.cancel.is_cancelled());
        h.finish().await;
    }

    #[tokio::test]
    async fn speech_arriving_during_inference_rejects_old_snapshot_and_reevaluates() {
        let mut h = Harness::new().await;
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
    }

    #[tokio::test]
    async fn capture_gap_fails_and_invalidates_already_enqueued_response() {
        let mut h = Harness::new().await;
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
        let error = tokio::time::timeout(Duration::from_secs(2), h.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
        assert!(response.cancel.is_cancelled());
    }

    #[tokio::test]
    async fn busy_model_retries_bounded_window_without_requiring_another_utterance() {
        let mut h = Harness::new().await;
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
        let mut old = Harness::new().await;
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
        let error = tokio::time::timeout(Duration::from_secs(2), old.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::PermissionDenied);
        assert!(error.message().contains(&format!("{domain} epoch changed")));
        assert!(old.input.try_recv().is_err());
        assert!(channel.fences.lock().unwrap().last().unwrap().1);

        let mut fresh = Harness::with_channel(channel).await;
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
    }

    #[tokio::test]
    async fn source_epoch_is_fixed_at_ready_before_first_audio_and_not_renewed_by_auth() {
        let h = Harness::new().await;
        h.channel
            .privacy_epoch
            .store(8, std::sync::atomic::Ordering::SeqCst);
        h.refresh(token(1)).await;
        let error = tokio::time::timeout(Duration::from_secs(2), h.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::PermissionDenied);
        assert!(h.channel.fences.lock().unwrap().last().unwrap().1);
    }

    #[tokio::test(start_paused = true)]
    async fn privacy_change_retires_idle_session_and_cancels_queued_response() {
        let mut h = Harness::new().await;
        h.speech().await;
        h.call().await.answer(TurnState::Complete);
        h.event().await;
        h.channel
            .privacy_epoch
            .store(8, std::sync::atomic::Ordering::SeqCst);
        for _ in 0..6 {
            h.silence().await;
            tokio::time::advance(Duration::from_secs(5)).await;
            tokio::task::yield_now().await;
        }
        let error = h.task.await.unwrap().unwrap_err();
        assert_eq!(error.code(), tonic::Code::PermissionDenied);
        let queued = h.input.recv().await.unwrap();
        assert_eq!(queued.source_privacy_epoch, 7);
        assert!(queued.cancel.is_cancelled());
        assert!(h.channel.fences.lock().unwrap().last().unwrap().1);
    }

    #[tokio::test]
    async fn continuously_ready_capture_does_not_starve_model_or_control() {
        let mut h = Harness::new().await;
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
    #[tokio::test(start_paused = true)]
    async fn renewed_credentials_keep_session_alive_beyond_five_minutes() {
        let mut h = Harness::new().await;
        let mut issued = Vec::new();
        for seconds in (5..=365).step_by(5) {
            h.silence().await;
            if seconds % 120 == 0 {
                let fresh = token(seconds);
                issued.push(fresh.clone());
                *h.channel.required_token.lock().unwrap() = Some(fresh.clone());
                // This refresh and the 30-second check become ready together.
                h.refresh(fresh).await;
            }
            tokio::time::advance(Duration::from_secs(5)).await;
            tokio::task::yield_now().await;
            assert!(
                !h.task.is_finished(),
                "session ended at virtual second {seconds}"
            );
        }
        let checked = h.channel.checked.lock().unwrap().clone();
        assert!(checked.len() >= 12, "30-second revalidation was skipped");
        assert_eq!(issued.len(), 3);
        assert!(issued.iter().all(|token| checked.contains(token)));
        h.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn binding_revocation_closes_on_periodic_revalidation() {
        let mut h = Harness::new().await;
        let channel = h.channel.clone();
        channel
            .revoked
            .store(true, std::sync::atomic::Ordering::SeqCst);
        for _ in 0..6 {
            h.silence().await;
            tokio::time::advance(Duration::from_secs(5)).await;
            tokio::task::yield_now().await;
        }
        let error = h.task.await.unwrap().unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unauthenticated);
        assert!(channel.fences.lock().unwrap().last().unwrap().1);
    }

    #[tokio::test]
    async fn wrong_or_stale_signed_refresh_is_rejected_and_closes_scoped_output() {
        use base64::Engine;
        for invalid_case in 0..4 {
            let h = Harness::new().await;
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
            let key =
                ed25519_dalek::SigningKey::from_bytes(&[if invalid_case == 0 { 9 } else { 7 }; 32]);
            let signature = alia_transport::sign_payload(&key, &payload);
            let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            h.refresh(format!(
                "{}.{}",
                engine.encode(payload),
                engine.encode(signature)
            ))
            .await;
            let error = h.task.await.unwrap().unwrap_err();
            assert_eq!(error.code(), tonic::Code::Unauthenticated);
            assert!(channel.fences.lock().unwrap().last().unwrap().1);
        }
    }

    #[tokio::test]
    async fn complete_wait_and_close_send_immutable_scoped_fences() {
        let mut h = Harness::new().await;
        h.speech().await;
        h.call().await.answer(TurnState::Complete);
        let response = h.event().await;
        let input = h.input.recv().await.unwrap();
        h.speech().await;
        h.call().await.answer(TurnState::Wait);
        let wait = h.event().await;
        assert_eq!(wait.generation_epoch, response.generation_epoch + 1);
        assert!(input.cancel.is_cancelled());
        let channel = h.channel.clone();
        h.finish().await;
        let fences = channel.fences.lock().unwrap();
        assert_eq!(fences.len(), 3);
        for (scope, _) in fences.iter() {
            assert_eq!(scope.session_id.to_string(), response.session_id);
            assert_eq!(scope.device_boot_id.to_string(), response.device_boot_id);
        }
        assert_eq!((fences[0].0.generation_epoch, fences[0].1), (2, false));
        assert_eq!((fences[1].0.generation_epoch, fences[1].1), (3, false));
        assert!(fences[2].1);
    }
}
