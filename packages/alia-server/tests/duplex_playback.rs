//! Real server Delivery owner and Embedded PlaybackService over loopback gRPC.
//! Only the bounded native queue is simulated. No Presented or acoustic claim.
#![cfg(feature = "channel-embedded")]
mod delivery_support;
use alia_core::{delivery::*, output_control::*, playback_control::*};
use alia_embedded::{
    binding::BindingManager,
    grpc::PlaybackService,
    module::{
        lifecycle::ModuleLifecycle as DeviceLifecycle,
        playback_session::{
            AudioSubmission, CaptureControlBarrier, OutputBlocked, PlaybackControl, PlaybackSession,
        },
        traits::AudioPlaybackBackend,
    },
    storage::EmbeddedStore,
};
use alia_proto::playback::{
    self as proto,
    playback_server::{Playback, PlaybackServer},
};
use alia_server::{
    channels::{
        adapter::*,
        embedded::{EmbeddedChannel, EmbeddedChannelConfig},
    },
    event::EventBus,
    module::{ModuleLifecycle as ServerLifecycle, lifecycle::ModuleContext},
    modules::delivery::{AudioDelivery, DeliveryService, OutputProducer, types::*},
    plugins::PluginStorageService,
    providers::{sensor::SensorRegistry, tts::audio_budget::ResponseAudioBudget},
    registry::ArtifactRegistry,
    settings::SettingsRegistry,
};
use delivery_support::{Fixture, with_db};
use futures::FutureExt;
use sea_orm::ConnectionTrait;
use serde_json::json;
use std::{
    collections::HashMap,
    future::Future,
    panic::AssertUnwindSafe,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
use uuid::Uuid;

struct Device {
    state: Mutex<(u64, Vec<i16>)>,
    control: Mutex<Option<Arc<PlaybackControl>>>,
    backend: alia_core::delivery::SubmissionBackend,
    changed: Condvar,
    capacity: usize,
    submissions: Mutex<HashMap<PlaybackIdentity, Arc<AudioSubmission>>>,
    queued: Mutex<Vec<(PlaybackIdentity, Vec<i16>)>>,
    blocked: AtomicUsize,
    blocked_event: tokio::sync::Semaphore,
    ended_event: tokio::sync::Semaphore,
    hold_end: AtomicBool,
    end_entered: AtomicBool,
}
#[tonic::async_trait]
impl DeviceLifecycle for Device {
    fn name(&self) -> &'static str {
        "test-playback"
    }
}
impl AudioPlaybackBackend for Device {
    fn playback_control(
        &self,
        scope: &VoiceScope,
        generation: u64,
    ) -> anyhow::Result<Arc<PlaybackControl>> {
        anyhow::ensure!(self.is_playback_current(generation), "native owner gone");
        let mut control = self.control.lock().unwrap();
        if let Some(control) = control.as_ref() {
            anyhow::ensure!(
                control.scope == *scope && control.generation == generation,
                "scope changed"
            );
            return Ok(control.clone());
        }
        let created = PlaybackControl::new(*scope, generation, self.backend.clone())?;
        *control = Some(created.clone());
        Ok(created)
    }
    fn apply_playback_control(
        &self,
        control: &Arc<PlaybackControl>,
        command: &ControlCommand,
    ) -> anyhow::Result<ControlReceipt> {
        anyhow::ensure!(
            self.is_playback_current(control.generation),
            "native owner gone"
        );
        let receipt = control.apply(command);
        self.changed.notify_all();
        Ok(receipt)
    }
    fn open_submission(
        &self,
        identity: PlaybackIdentity,
        rate: u32,
        generation: u64,
    ) -> anyhow::Result<Arc<AudioSubmission>> {
        let control = self.playback_control(&identity.scope, generation)?;
        let submission = AudioSubmission::with_control(
            identity,
            generation,
            rate,
            self.backend.clone(),
            Some(control),
        )?;
        self.submissions
            .lock()
            .unwrap()
            .insert(identity, submission.clone());
        Ok(submission)
    }
    fn submit_audio(
        &self,
        submission: &Arc<AudioSubmission>,
        offset: u64,
        pcm: &[i16],
    ) -> anyhow::Result<bool> {
        let mut state = self.state.lock().unwrap();
        while state.0 == submission.generation
            && submission.is_live()
            && state.1.len() >= self.capacity
        {
            self.blocked.fetch_add(1, Ordering::SeqCst);
            self.blocked_event.add_permits(1);
            state = self.changed.wait(state).unwrap();
            self.blocked.fetch_sub(1, Ordering::SeqCst);
        }
        if state.0 != submission.generation || !submission.is_live() {
            return Ok(false);
        }
        submission.accept(offset, pcm.len() as u64)?;
        self.queued
            .lock()
            .unwrap()
            .push((submission.identity, pcm.to_vec()));
        state.1.extend_from_slice(pcm);
        Ok(true)
    }
    fn end_submission(
        &self,
        submission: &Arc<AudioSubmission>,
        total: u64,
    ) -> anyhow::Result<bool> {
        let accepted = submission.accept_end(total)?;
        self.end_entered.store(true, Ordering::Release);
        self.ended_event.add_permits(1);
        let mut state = self.state.lock().unwrap();
        while self.hold_end.load(Ordering::Acquire) {
            state = self.changed.wait(state).unwrap();
        }
        Ok(accepted)
    }
    fn cancel_submission(
        &self,
        submission: &Arc<AudioSubmission>,
        reason: &str,
    ) -> anyhow::Result<bool> {
        let mut state = self.state.lock().unwrap();
        if state.0 != submission.generation {
            return Ok(false);
        }
        self.hold_end.store(false, Ordering::Release);
        submission.terminate(ObservationEnd::Cancelled {
            reason: reason.into(),
        });
        let mut queued = self.queued.lock().unwrap();
        queued.retain(|(identity, _)| *identity != submission.identity);
        state.1 = queued
            .iter()
            .flat_map(|(_, pcm)| pcm.iter().copied())
            .collect();
        self.changed.notify_all();
        Ok(true)
    }
    fn play(&self, _: &[i16], _: u32) -> anyhow::Result<()> {
        unreachable!()
    }
    fn stop_playback(&self) -> anyhow::Result<()> {
        unreachable!()
    }
    fn is_available(&self) -> bool {
        true
    }
    fn begin_scoped_playback(&self) -> anyhow::Result<u64> {
        let mut state = self.state.lock().unwrap();
        state.0 += 1;
        if let Some(control) = self.control.lock().unwrap().take() {
            control.retire(RetiredReason::Cancelled);
        }
        state.1.clear();
        self.queued.lock().unwrap().clear();
        self.changed.notify_all();
        Ok(state.0)
    }
    fn play_if_current(&self, pcm: &[i16], _: u32, generation: u64) -> anyhow::Result<bool> {
        let mut state = self.state.lock().unwrap();
        while state.0 == generation && state.1.len() >= self.capacity {
            state = self.changed.wait(state).unwrap();
        }
        if state.0 != generation {
            return Ok(false);
        }
        state.1.extend_from_slice(pcm);
        Ok(true)
    }
    fn stop_if_current(&self, generation: u64) -> anyhow::Result<bool> {
        let mut state = self.state.lock().unwrap();
        if state.0 != generation {
            return Ok(false);
        }
        state.0 += 1;
        if let Some(control) = self.control.lock().unwrap().take() {
            control.retire(RetiredReason::Cancelled);
        }
        state.1.clear();
        self.queued.lock().unwrap().clear();
        self.changed.notify_all();
        Ok(true)
    }
    fn is_playback_current(&self, generation: u64) -> bool {
        self.state.lock().unwrap().0 == generation
    }
}

impl Device {
    /// Explicit simulated callback submission, never a sleep-based completion.
    fn submit_frames(&self, id: &str, frames: usize) {
        let mut state = self.state.lock().unwrap();
        let mut queued = self.queued.lock().unwrap();
        let submission = self
            .submissions
            .lock()
            .unwrap()
            .values()
            .find(|s| s.identity.playback_id.to_string() == id)
            .unwrap()
            .clone();
        let write = submission
            .try_begin_output()
            .expect("simulated callback must respect the original gate");
        let mut remaining = frames;
        for (identity, pcm) in queued.iter_mut() {
            if identity.playback_id.to_string() == id {
                let count = remaining.min(pcm.len());
                pcm.drain(..count);
                remaining -= count;
            }
        }
        assert_eq!(remaining, 0);
        state.1 = queued
            .iter()
            .flat_map(|(_, pcm)| pcm.iter().copied())
            .collect();
        write.finish(frames as u64);
        self.changed.notify_all();
    }
}

/// Faults happen after the production service has executed the authenticated
/// command. The proxy never constructs a receipt or executes native state.
struct Wire {
    inner: PlaybackService,
    lose_control_ack: AtomicBool,
    hold_registration: AtomicBool,
    registered: Semaphore,
    release_registration: Semaphore,
    controls: Mutex<Vec<BoundControlCommand>>,
    observations: AtomicUsize,
    pushes: AtomicUsize,
}
#[derive(Clone)]
struct Proxy(Arc<Wire>);
#[tonic::async_trait]
impl Playback for Proxy {
    type PushStream = <PlaybackService as Playback>::PushStream;
    async fn register_output(
        &self,
        request: Request<proto::RegisterOutputRequest>,
    ) -> std::result::Result<Response<proto::RegisterOutputReceipt>, Status> {
        let result = self.0.inner.register_output(request).await?;
        if self.0.hold_registration.swap(false, Ordering::SeqCst) {
            self.0.registered.add_permits(1);
            self.0
                .release_registration
                .acquire()
                .await
                .unwrap()
                .forget();
        }
        Ok(result)
    }
    async fn control(
        &self,
        request: Request<proto::ControlRequest>,
    ) -> std::result::Result<Response<proto::ControlResponse>, Status> {
        self.0
            .controls
            .lock()
            .unwrap()
            .push(alia_proto::output_control::decode_control_request(request.get_ref()).unwrap());
        let result = self.0.inner.control(request).await?;
        if self.0.lose_control_ack.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable(
                "injected loss after native command execution",
            ));
        }
        Ok(result)
    }
    async fn observe_control(
        &self,
        request: Request<proto::ControlRequest>,
    ) -> std::result::Result<Response<proto::ControlResponse>, Status> {
        self.0.observations.fetch_add(1, Ordering::SeqCst);
        self.0.inner.observe_control(request).await
    }
    async fn push(
        &self,
        request: Request<tonic::Streaming<proto::PlaybackFrame>>,
    ) -> std::result::Result<Response<Self::PushStream>, Status> {
        self.0.pushes.fetch_add(1, Ordering::SeqCst);
        self.0.inner.push(request).await
    }
    async fn fence(
        &self,
        request: Request<proto::FenceRequest>,
    ) -> std::result::Result<Response<proto::FenceReceipt>, Status> {
        self.0.inner.fence(request).await
    }
}
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct RpcServer {
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for RpcServer {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
impl RpcServer {
    async fn close(mut self) {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}
#[derive(Clone)]
struct Scene {
    f: Fixture,
    service: Arc<DeliveryService>,
    channel: Arc<EmbeddedChannel>,
    session: Arc<PlaybackSession>,
    device: Arc<Device>,
    wire: Arc<Wire>,
    scope: VoiceScope,
    capture: CaptureControlBarrier,
}
struct Runtime {
    scene: Scene,
    server: RpcServer,
    _directory: Directory,
}
impl Runtime {
    async fn new(f: Fixture, capacity: usize) -> Self {
        let directory =
            Directory(std::env::temp_dir().join(format!("alia-duplex-pair-{}", Uuid::new_v4())));
        std::fs::create_dir(&directory.0).unwrap();
        let key_path = directory.0.join("server.key");
        let keypair = alia_server::auth::ensure_server_keypair(&key_path).unwrap();
        let store = EmbeddedStore::open(directory.0.join("device.sqlite"))
            .await
            .unwrap();
        let binding = Arc::new(BindingManager::new(store, 3, 60));
        let issued = alia_transport::current_unix_timestamp().unwrap();
        let public = keypair.public_key_bytes();
        let payload = alia_transport::bind_payload(
            "server",
            "server",
            "http://localhost:1234",
            &public,
            issued,
        );
        binding
            .bind(
                "server",
                "server",
                "http://localhost:1234",
                &public,
                &alia_transport::sign_payload(keypair.signing_key(), &payload),
                issued,
            )
            .await
            .unwrap();
        let device = Arc::new(Device {
            state: Mutex::new((0, Vec::new())),
            control: Mutex::new(None),
            backend: SubmissionBackend {
                instance_id: Uuid::new_v4(),
                boundary: SubmissionBoundary::CpalCallbackBuffer,
                output_sample_rate_hz: 16000,
                output_channels: 1,
            },
            changed: Condvar::new(),
            capacity,
            submissions: Mutex::new(HashMap::new()),
            queued: Mutex::new(Vec::new()),
            blocked: AtomicUsize::new(0),
            blocked_event: Semaphore::new(0),
            ended_event: Semaphore::new(0),
            hold_end: AtomicBool::new(false),
            end_entered: AtomicBool::new(false),
        });
        let session = Arc::new(PlaybackSession::new(device.clone()));
        let scope = VoiceScope {
            device_boot_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            generation_epoch: 1,
        };
        let capture = CaptureControlBarrier::default();
        session
            .accept_capture(&capture, 0, true, || Ok(()))
            .unwrap();
        session
            .activate_with_capture(&scope, capture.clone(), MAX_HOLD_MS)
            .unwrap();
        let revoked = session.clone();
        binding
            .install_output_revocation(Arc::new(move || revoked.revoke_all().unwrap()))
            .unwrap();
        let wire = Arc::new(Wire {
            inner: PlaybackService::new(session.clone(), binding),
            lose_control_ack: AtomicBool::new(false),
            hold_registration: AtomicBool::new(false),
            registered: Semaphore::new(0),
            release_registration: Semaphore::new(0),
            controls: Mutex::new(Vec::new()),
            observations: AtomicUsize::new(0),
            pushes: AtomicUsize::new(0),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let proxy = Proxy(wire.clone());
        let server = tokio::spawn(async move {
            let incoming = async_stream::stream! { loop { yield listener.accept().await.map(|(stream, _)| stream); } };
            tonic::transport::Server::builder()
                .add_service(PlaybackServer::new(proxy))
                .serve_with_incoming_shutdown(incoming, shutdown.cancelled())
                .await
                .unwrap();
        });
        // This guard exists before fallible channel initialization so a setup
        // panic cannot detach a live test listener.
        let server = RpcServer {
            stop,
            task: Some(server),
        };
        let mut config: alia_server::config::ServerConfig =
            toml::from_str(include_str!("../../../config/server.example.toml")).unwrap();
        config.server.id = "server".into();
        config.auth.server_keypair_path = key_path;
        let events = EventBus::new(16);
        let ctx = ModuleContext {
            db: f.db.clone(),
            config: Arc::new(config),
            event_bus: events.clone(),
            cancel_token: CancellationToken::new(),
            settings: SettingsRegistry::new(),
            artifacts: Arc::new(ArtifactRegistry::new(f.db.clone())),
            plugin_storage: Arc::new(PluginStorageService::new(f.db.clone())),
            sensors: Arc::new(SensorRegistry::new(Default::default())),
            scheduler: None,
            conditions: None,
            actions: None,
            conversation: None,
            memory: None,
            llm: None,
            minor_llm: None,
            major_llm: None,
            transcript_correction_llm: None,
            memory_capture_llm: None,
            sentence_expression_llm: None,
            embedding: None,
            cache: None,
            tools: None,
            expression: None,
        };
        let mut channel = EmbeddedChannel::new(
            EmbeddedChannelConfig {
                id: "test-device".into(),
                owner_user_id: Some(f.user),
                expected_device_id: None,
                expected_device_public_key: None,
                endpoint,
                discovery_service_name: "_alia-paired-test._tcp".into(),
                discovery_timeout_sec: 1,
                reconnect_interval_sec: 1,
                reconnect_max_backoff_sec: 1,
                grpc_timeout_sec: 30,
                voice_turn_interrupt_enabled: true,
                cancel_streaming_tts_on_interrupt: true,
                discard_interrupted_turn_history: true,
            },
            events,
        );
        channel.init(&ctx).await.unwrap();
        let service = DeliveryService::new(f.repo(), &CancellationToken::new());
        service.start().await.unwrap();
        Self {
            scene: Scene {
                f,
                service,
                channel: Arc::new(channel),
                session,
                device,
                wire,
                scope,
                capture,
            },
            server,
            _directory: directory,
        }
    }
    async fn close(self) {
        self.scene.wire.release_registration.add_permits(8);
        // Free native queue waiters before joining either production supervisor.
        self.scene.session.revoke_all().unwrap();
        let delivery = self.scene.service.shutdown().await;
        assert!(
            !self.scene.service.shutdown_pending(),
            "Delivery did not join: {delivery:?}"
        );
        let embedded = self.scene.session.stop().await;
        self.scene.channel.stop().await.unwrap();
        self.server.close().await;
        embedded.unwrap();
    }
}
impl Scene {
    fn target(&self) -> ChannelTarget {
        ChannelTarget {
            user_id: self.f.user.to_string(),
            voice: Some(self.scope),
        }
    }
    async fn output(&self) -> (Arc<OutputProducer>, ResponseRecord) {
        let mut input = self.f.response().await;
        input.content["voice_scope"] = json!(self.scope);
        let producer = self
            .service
            .begin_voice_output(
                self.f.user,
                input.key.request_id,
                self.scope,
                self.channel.clone(),
                CancellationToken::new(),
                Some(Duration::from_secs(30)),
            )
            .unwrap();
        let response = self.service.prepare(input).await.unwrap();
        producer.bind(response.clone()).await.unwrap();
        (producer, response)
    }
    async fn audio(&self, response: &ResponseRecord, pcm: &[i16]) -> AudioDelivery {
        let bytes: Vec<_> = pcm.iter().flat_map(|v| v.to_le_bytes()).collect();
        let data = ResponseAudioBudget::for_response(response.response_id)
            .unwrap()
            .copy_from_slice(&bytes)
            .await
            .unwrap()
            .into_buffer();
        AudioDelivery {
            response_id: response.response_id,
            output_key: "speech:0".into(),
            content: json!({"text":"paired speech"}),
            channel: self.channel.clone(),
            target: self.target(),
            audio: Box::pin(futures::stream::iter([Ok(AudioPayload {
                data,
                sample_rate: 16000,
                channels: 1,
                format: AudioFormat::Pcm16,
            })])),
            cancel: CancellationToken::new(),
        }
    }
    async fn command_count(&self, response: &ResponseRecord, count: usize) {
        eventually(|| async {
            let rows = self.f.db.query_one(sea_orm::Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
                "SELECT COUNT(*) AS n FROM output_control_commands WHERE response_id=$1 AND status='receipted'", [response.response_id.into()])).await.unwrap().unwrap();
            rows.try_get::<i64>("", "n").unwrap() == count as i64
        }).await;
    }
    fn continue_output(&self, producer: &OutputProducer) {
        self.session
            .accept_capture(&self.capture, 1, true, || Ok(()))
            .unwrap();
        assert!(
            producer
                .output()
                .request_resume(ResumeAuthorization {
                    kind: ResumeKind::ExplicitContinue,
                    detection_id: Uuid::new_v4(),
                    through_sequence: 1
                })
                .unwrap()
        );
    }
    fn drain(&self) -> Vec<i16> {
        let queued: Vec<_> = self
            .device
            .queued
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, pcm)| !pcm.is_empty())
            .map(|(id, pcm)| (id.playback_id.to_string(), pcm.clone()))
            .collect();
        let samples = queued
            .iter()
            .flat_map(|(_, pcm)| pcm.iter().copied())
            .collect();
        for (id, pcm) in queued {
            self.device.submit_frames(&id, pcm.len());
        }
        samples
    }

    fn submission(&self) -> Option<Arc<AudioSubmission>> {
        self.device
            .submissions
            .lock()
            .unwrap()
            .values()
            .next()
            .cloned()
    }
}
async fn eventually(mut predicate: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !predicate().await {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("bounded state observation");
}
async fn signal(semaphore: &Semaphore) {
    tokio::time::timeout(Duration::from_secs(3), semaphore.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}
async fn with_scene<F, Fut>(capacity: usize, test: F)
where
    F: FnOnce(Scene) -> Fut,
    Fut: Future<Output = ()>,
{
    with_db(|f| async move {
        let runtime = Runtime::new(f, capacity).await;
        let result = AssertUnwindSafe(tokio::time::timeout(
            Duration::from_secs(32),
            test(runtime.scene.clone()),
        ))
        .catch_unwind()
        .await;
        runtime.close().await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(_)) => panic!("paired playback exceeded 32 seconds"),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
    .await;
}
fn submitted(record: &DeliveryRecord, count: u64) {
    assert_eq!(record.status, DeliveryStatus::Submitted, "{record:#?}");
    assert_eq!(record.submitted_frames, Some(count));
    assert_eq!(record.presented_frames, None);
}

#[tokio::test]
async fn held_before_first_start_survives_handshake_idle_and_resumes_one_original_output() {
    with_scene(4096, |s| async move {
        let (producer, response) = s.output().await;
        assert!(producer.output().explicit_wait(Uuid::new_v4(), 0).unwrap());
        s.command_count(&response, 1).await;
        let input = s.audio(&response, &[11, 22, 33, 44]).await;
        let budget = ResponseAudioBudget::for_response(response.response_id).unwrap();
        let service = s.service.clone();
        let task = tokio::spawn(async move { service.deliver_audio(input).await });
        eventually(|| async {
            !s.service
                .list_deliveries(s.f.user, response.response_id)
                .await
                .unwrap()
                .is_empty()
        })
        .await;
        // Deliberately exceed the real five-second handshake budget. This tests
        // elapsed time, not race scheduling: admission was observed above.
        tokio::time::sleep(Duration::from_millis(5500)).await;
        assert!(!task.is_finished());
        assert_eq!(s.wire.pushes.load(Ordering::SeqCst), 0);
        assert_eq!(budget.reserved_bytes(), 8);
        assert!(s.submission().is_none());
        s.continue_output(&producer);
        s.command_count(&response, 2).await;
        signal(&s.device.ended_event).await;
        assert_eq!(s.drain(), [11, 22, 33, 44]);
        submitted(&task.await.unwrap().unwrap(), 4);
        assert_eq!(s.wire.pushes.load(Ordering::SeqCst), 1);
        assert_eq!(budget.reserved_bytes(), 0);
        drop(producer);
    })
    .await;
}

#[tokio::test]
async fn held_full_native_queue_keeps_original_pcm_and_control_lane_available() {
    with_scene(1600, |s| async move {
        let (producer, response) = s.output().await;
        let pcm: Vec<i16> = (0..6400).map(|n| n as i16).collect();
        let input = s.audio(&response, &pcm).await;
        let service = s.service.clone();
        let task = tokio::spawn(async move { service.deliver_audio(input).await });
        signal(&s.device.blocked_event).await;
        let first = s.device.state.lock().unwrap().1.clone();
        assert_eq!(first, &pcm[..1600]);
        assert!(producer.output().explicit_wait(Uuid::new_v4(), 0).unwrap());
        s.command_count(&response, 1).await;
        let original = s.submission().unwrap();
        assert_eq!(
            original.try_begin_output().unwrap_err(),
            OutputBlocked::Held
        );
        assert_eq!(s.device.state.lock().unwrap().1, first);
        assert!(!task.is_finished());
        s.continue_output(&producer);
        s.command_count(&response, 2).await;
        let native = Mutex::new(Vec::new());
        eventually(|| async {
            native.lock().unwrap().extend(s.drain());
            task.is_finished()
        })
        .await;
        submitted(&task.await.unwrap().unwrap(), 6400);
        assert_eq!(*native.lock().unwrap(), pcm);
        assert_eq!(s.device.submissions.lock().unwrap().len(), 1);
        drop(producer);
    })
    .await;
}

#[tokio::test]
async fn held_after_end_preserves_unsubmitted_tail_beyond_media_idle() {
    with_scene(4096, |s| async move {
        let (producer, response) = s.output().await;
        let input = s.audio(&response, &[8, 7, 6, 5]).await;
        let service = s.service.clone();
        let task = tokio::spawn(async move { service.deliver_audio(input).await });
        signal(&s.device.ended_event).await;
        let original = s.submission().unwrap();
        assert!(original.end_accepted());
        assert_eq!(original.snapshot(0).submitted_source_frames, Some(0));
        assert!(producer.output().explicit_wait(Uuid::new_v4(), 0).unwrap());
        s.command_count(&response, 1).await;
        tokio::time::sleep(Duration::from_millis(5500)).await;
        assert!(!task.is_finished());
        assert_eq!(
            original.try_begin_output().unwrap_err(),
            OutputBlocked::Held
        );
        assert_eq!(s.device.state.lock().unwrap().1, [8, 7, 6, 5]);
        s.continue_output(&producer);
        s.command_count(&response, 2).await;
        assert_eq!(s.drain(), [8, 7, 6, 5]);
        submitted(&task.await.unwrap().unwrap(), 4);
        assert_eq!(s.device.submissions.lock().unwrap().len(), 1);
        drop(producer);
    })
    .await;
}

#[tokio::test]
async fn control_ack_loss_is_observed_from_original_device_command() {
    with_scene(4096, |s| async move {
        let (producer, response) = s.output().await;
        s.wire.lose_control_ack.store(true, Ordering::SeqCst);
        assert!(producer.output().explicit_wait(Uuid::new_v4(), 0).unwrap());
        s.command_count(&response, 1).await;
        assert_eq!(s.wire.controls.lock().unwrap().len(), 1);
        assert!(s.wire.observations.load(Ordering::SeqCst) >= 1);
        assert_eq!(
            s.session.observe_control(&s.scope).unwrap().phase,
            ControlPhase::Held
        );
        assert!(producer.output().is_held());
        let original = s.wire.controls.lock().unwrap()[0].clone();
        let observed = s.channel.observe_output_control(&original).await.unwrap();
        assert_eq!(observed.command, original);
        assert_eq!(observed.receipt.outcome, CommandOutcome::Applied);
        assert_eq!(observed.receipt.snapshot.phase, ControlPhase::Held);
        s.continue_output(&producer);
        s.command_count(&response, 2).await;
        assert!(!producer.output().is_held());
        drop(producer);
    })
    .await;
}

#[tokio::test]
async fn registration_reply_after_fence_never_repopulates_obsolete_channel_binding() {
    with_scene(4096, |s| async move {
        let response = s.service.prepare(s.f.response().await).await.unwrap();
        let binding = OutputBinding {
            scope: s.scope,
            response_id: response.response_id,
            context_id: response.context_id.unwrap(),
            server_instance_id: Uuid::new_v4(),
        };
        s.wire.hold_registration.store(true, Ordering::SeqCst);
        let channel = s.channel.clone();
        let task = tokio::spawn(async move { channel.register_output(binding).await });
        signal(&s.wire.registered).await;
        s.channel
            .stop_current_playback(
                &s.target(),
                "fence while original registration ACK is delayed",
            )
            .await
            .unwrap();
        s.wire.release_registration.add_permits(1);
        assert!(
            task.await.unwrap().is_err(),
            "late acknowledgement resurrected old local admission"
        );
        assert!(
            s.channel.register_output(binding).await.is_err(),
            "obsolete registration served from late-filled cache"
        );
        assert!(s.session.register_output(&binding).is_err());
    })
    .await;
}
