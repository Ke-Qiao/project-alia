//! Production SpeechPipeline -> signed VoiceSession -> Delivery -> PlaybackService.
//! Only PCM/VAD, turn results, response content and the native callback are fixtures.
//! Linux also runs the production R1 UART owner against a PTY; no hardware opens.
use super::*;
use alia_core::voice::{DetectionId, ModelIdentity, PcmWindow, TurnObservation, TurnState};
#[cfg(not(target_os = "linux"))]
use alia_embedded::module::traits::WakeWordEvent;
use alia_embedded::{
    config::{IdentityConfig, SpeechConfig},
    module::{
        context::{AudioBus, AudioFrame, EventBus as DeviceEvents, ModuleEvent},
        noop::NoopDirectResponse,
        speech::SpeechPipeline,
        traits::{VoiceActivityEngine, VoiceActivityState, WakeWordEngine, WakeWordSource},
    },
};
use alia_proto::voice::{
    self,
    voice_session_server::{VoiceSession, VoiceSessionServer},
};
use alia_server::{
    channels::ChannelRegistry,
    grpc::{voice_auth::VoiceIngressAuth, voice_session::VoiceSessionService},
    modules::{
        interaction::{
            turn_detector::{DetectionError, TurnDetector},
            voice_input::{self, RecognizedVoiceInput},
        },
        memory::context::ContextSource,
        platform::lifecycle::RuntimeTasks,
    },
};
use futures::{Stream, StreamExt};
use sea_orm::{DatabaseBackend, Statement};
use std::pin::Pin;
use tokio::sync::{mpsc, oneshot};

struct Detection {
    id: DetectionId,
    pcm: PcmWindow,
    reply: oneshot::Sender<TurnObservation>,
}
impl Detection {
    fn answer(self, state: TurnState, transcript: &str) {
        self.reply
            .send(TurnObservation {
                id: self.id,
                state,
                transcript: transcript.into(),
                inference_ms: 1.0,
                model: ModelIdentity {
                    source: "deterministic-speech-test".into(),
                    revision: "1".into(),
                    device: "fixture".into(),
                },
            })
            .unwrap();
    }
}
struct Detector(mpsc::Sender<Detection>);
#[tonic::async_trait]
impl TurnDetector for Detector {
    async fn detect(
        &self,
        id: DetectionId,
        pcm: &PcmWindow,
        cancel: &CancellationToken,
    ) -> Result<TurnObservation, DetectionError> {
        let (reply, receive) = oneshot::channel();
        self.0
            .send(Detection {
                id,
                pcm: pcm.clone(),
                reply,
            })
            .await
            .map_err(|_| DetectionError::Cancelled)?;
        tokio::select! {
            _ = cancel.cancelled() => Err(DetectionError::Cancelled),
            result = receive => result.map_err(|_| DetectionError::Cancelled),
        }
    }
}

struct Vad(Arc<Semaphore>);
#[tonic::async_trait]
impl DeviceLifecycle for Vad {
    fn name(&self) -> &'static str {
        "fixture-vad"
    }
}
impl VoiceActivityEngine for Vad {
    fn reset(&self) -> anyhow::Result<()> {
        Ok(())
    }
    fn accept(&self, pcm: &[i16], rate: u32) -> anyhow::Result<VoiceActivityState> {
        assert_eq!(rate, 16000);
        self.0.add_permits(1);
        Ok(if pcm.iter().any(|sample| *sample != 0) {
            VoiceActivityState::Speech
        } else {
            VoiceActivityState::Silence
        })
    }
}

#[cfg(not(target_os = "linux"))]
#[derive(Default)]
struct LocalWake(AtomicBool);
#[cfg(not(target_os = "linux"))]
#[tonic::async_trait]
impl DeviceLifecycle for LocalWake {
    fn name(&self) -> &'static str {
        "fixture-wake"
    }
}
#[cfg(not(target_os = "linux"))]
impl WakeWordEngine for LocalWake {
    fn reset(&self) -> anyhow::Result<()> {
        self.0.store(false, Ordering::Release);
        Ok(())
    }
    fn detect(&self, _: &[i16]) -> Option<WakeWordEvent> {
        self.0.swap(false, Ordering::AcqRel).then(|| WakeWordEvent {
            keyword: "fixture-wake".into(),
            timestamp_us: 1,
            source: WakeWordSource::Local,
        })
    }
}

#[cfg(target_os = "linux")]
#[path = "speech/r1.rs"]
mod r1;

#[path = "speech/system_capture.rs"]
mod system_capture;

struct Capture {
    bus: AudioBus,
    consumed: Arc<Semaphore>,
    offset: u64,
}
impl Capture {
    async fn frame(&mut self, value: i16) {
        self.bus.publish(AudioFrame {
            pcm: vec![value; 1600],
            sample_rate: 16000,
            timestamp_us: self.offset * 1_000_000 / 16000,
            timing: None,
        });
        self.offset += 1600;
        signal(&self.consumed).await;
        // Pace the synthetic device instead of overflowing the production upload queue.
        tokio::task::yield_now().await;
    }
    async fn silence(&mut self) {
        for _ in 0..7 {
            self.frame(0).await;
        }
    }
    async fn utterance(&mut self, value: i16) {
        self.frame(value).await;
        self.silence().await;
    }
}

/// A network failure drops the real service stream; no session event is fabricated.
#[derive(Clone)]
struct VoiceWire {
    inner: Arc<VoiceSessionService>,
    disconnect: Arc<Mutex<Option<CancellationToken>>>,
    events: Arc<Mutex<Vec<voice::SessionEvent>>>,
}
#[tonic::async_trait]
impl VoiceSession for VoiceWire {
    type StreamStream = Pin<Box<dyn Stream<Item = Result<voice::SessionEvent, Status>> + Send>>;
    async fn stream(
        &self,
        request: Request<tonic::Streaming<voice::DeviceFrame>>,
    ) -> Result<Response<Self::StreamStream>, Status> {
        let mut stream = self.inner.stream(request).await?.into_inner();
        let disconnect = CancellationToken::new();
        *self.disconnect.lock().unwrap() = Some(disconnect.clone());
        let events = self.events.clone();
        Ok(Response::new(Box::pin(async_stream::stream! {
            loop {
                tokio::select! {
                    biased;
                    _ = disconnect.cancelled() => { yield Err(Status::unavailable("injected voice transport disconnect")); break; },
                    event = stream.next() => match event {
                        Some(event) => {
                            if let Ok(event) = &event { events.lock().unwrap().push(event.clone()); }
                            yield event;
                        },
                        None => break,
                    },
                }
            }
        })))
    }
}

#[derive(Default)]
struct SpeechFixture {
    paired: Option<Runtime>,
    speech: Option<SpeechPipeline>,
    voice: Option<RpcServer>,
    wake: Option<Arc<dyn WakeWordEngine>>,
    stop: CancellationToken,
    tasks: Arc<RuntimeTasks>,
    deliveries: Vec<tokio::task::JoinHandle<DeliveryResult<DeliveryRecord>>>,
    stage: &'static str,
}
impl SpeechFixture {
    async fn close(mut self) {
        if self.stage != "body complete" {
            eprintln!("speech fixture cleanup at {}", self.stage);
        }
        self.stop.cancel();
        let mut failed = Vec::new();
        if let Some(speech) = self.speech.take() {
            if speech.stop().await.is_err() {
                failed.push("speech");
            }
        }
        if let Some(wake) = self.wake.take() {
            if wake.stop().await.is_err() {
                failed.push("wake");
            }
        }
        self.tasks.close();
        if let Some(voice) = self.voice.take() {
            if AssertUnwindSafe(voice.close())
                .catch_unwind()
                .await
                .is_err()
            {
                failed.push("voice listener");
            }
        }
        if self.tasks.wait().await.is_err() {
            failed.push("voice tasks");
        }
        if let Some(paired) = self.paired.take() {
            if AssertUnwindSafe(paired.close())
                .catch_unwind()
                .await
                .is_err()
            {
                failed.push("paired owner");
            }
        }
        for task in self.deliveries {
            if task.await.is_err() {
                failed.push("media join");
            }
        }
        assert!(
            failed.is_empty(),
            "speech cleanup at {}: {failed:?}",
            self.stage
        );
    }
}

async fn receive<T>(receiver: &mut mpsc::Receiver<T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), receiver.recv())
        .await
        .unwrap()
        .unwrap()
}

pub(super) async fn bind_response(
    f: &Fixture,
    service: &Arc<DeliveryService>,
    input: &RecognizedVoiceInput,
) -> ResponseRecord {
    let conversation = Uuid::new_v4();
    let source = Uuid::new_v4();
    f.db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,$2,$3)",
        vec![
            conversation.into(),
            input.channel_id.clone().into(),
            f.user.to_string().into(),
        ],
    ))
    .await
    .unwrap();
    let source_time = f.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,$5,$6,$7) RETURNING source_received_at",
        vec![source.into(), conversation.into(), f.user.to_string().into(), input.transcript.clone().into(), input.source_received_at.into(), input.source_privacy_epoch.into(), input.source_relationship_epoch.into()])).await.unwrap().unwrap().try_get("", "source_received_at").unwrap();
    let context = f
        .context()
        .materialize(
            ContextSource {
                owner_id: f.user,
                request_id: input.request_id,
                channel_id: input.channel_id.clone(),
                conversation_id: Some(conversation),
                source_event_id: Some(source),
                source_received_at: source_time,
                source_privacy_epoch: input.source_privacy_epoch,
            },
            &input.transcript,
        )
        .await
        .unwrap();
    let response = service
        .prepare(NewResponse {
            key: ResponseKey {
                user_id: f.user,
                request_id: input.request_id,
                output_key: "answer".into(),
            },
            context_id: context.lease().id(),
            conversation_id: Some(conversation),
            source_event_id: Some(source),
            source_received_at: source_time,
            source_privacy_epoch: Some(input.source_privacy_epoch),
            channel_id: input.channel_id.clone(),
            kind: "conversation".into(),
            content: json!({"intent":input.transcript,"voice_scope":input.output.as_ref().unwrap().producer.scope()}),
        })
        .await
        .unwrap();
    input
        .output
        .as_ref()
        .unwrap()
        .producer
        .bind(response.clone())
        .await
        .unwrap();
    response
}

async fn send_media(
    owner: &mut SpeechFixture,
    s: &Scene,
    response: &ResponseRecord,
    samples: &[i16],
) {
    let input = s.audio(response, samples).await;
    let service = s.service.clone();
    owner.deliveries.push(tokio::spawn(
        async move { service.deliver_audio(input).await },
    ));
    media_signal(&s.device.ended_event, owner.deliveries.last_mut().unwrap()).await;
}

#[tokio::test]
async fn speech_upload_and_real_server_playback_share_pause_cancel_and_reconnect_ownership() {
    with_db_context(SpeechFixture::default(), |f, owner| Box::pin(async move {
        owner.stage = "initialize signed playback pair";
        Runtime::initialize(&mut owner.paired, f, 4096).await;
        let paired = owner.paired.as_ref().unwrap();
        let mut s = paired.scene.clone();
        s.session.revoke_all().unwrap();
        let channels = Arc::new(ChannelRegistry::new());
        channels.register(s.channel.clone()).await.unwrap();
        s.channel.binding_client().await.unwrap();
        let (input, mut recognized) = voice_input::channel();
        let (detector, mut detections) = mpsc::channel(2);
        let wire = VoiceWire {
            inner: Arc::new(VoiceSessionService::new(
                VoiceIngressAuth::new(channels), Some(Arc::new(Detector(detector))), input,
                s.service.clone(), Duration::from_secs(30), owner.stop.clone(), owner.tasks.clone(),
            )),
            disconnect: Arc::default(), events: Arc::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        // Binding stores the HTTP base; the production voice client uses its next port.
        let address = listener.local_addr().unwrap();
        let endpoint = format!("http://127.0.0.1:{}", address.port() - 1);
        let stop = owner.stop.clone();
        let service = wire.clone();
        owner.voice = Some(RpcServer { stop: stop.clone(), task: Some(tokio::spawn(async move {
            let incoming = async_stream::stream! { loop { yield listener.accept().await.map(|(stream, _)| stream); } };
            tonic::transport::Server::builder().add_service(VoiceSessionServer::new(service))
                .serve_with_incoming_shutdown(incoming, stop.cancelled()).await.unwrap();
        })) });
        let binding = paired.binding.clone();
        let key = alia_server::auth::ensure_server_keypair(&paired._directory.0.join("server.key")).unwrap();
        binding.unbind("server").await.unwrap();
        let issued = alia_transport::current_unix_timestamp().unwrap();
        let public = key.public_key_bytes();
        let payload = alia_transport::bind_payload("server", "server", &endpoint, &public, issued);
        binding.bind("server", "server", &endpoint, &public, &alia_transport::sign_payload(key.signing_key(), &payload), issued).await.unwrap();
        let identity = IdentityConfig { device_id: "companion-paired-device".into(), device_name: "speech fixture".into(), keypair_path: paired._directory.0.join("device.key") };
        std::fs::write(&identity.keypair_path, [19u8; 32]).unwrap();
        alia_embedded::identity::load_device_keypair(&identity.keypair_path).unwrap();
        assert!(!binding.status().await.dev_skip_auth);

        #[cfg(target_os = "linux")]
        let (mut board, wake) = r1::Board::new();
        #[cfg(not(target_os = "linux"))]
        let wake = Arc::new(LocalWake::default());
        owner.wake = Some(wake.clone());
        let bus = AudioBus::new(32);
        let consumed = Arc::new(Semaphore::new(0));
        let events = DeviceEvents::new(32);
        let mut lifecycle = events.subscribe();
        owner.speech = Some(SpeechPipeline::new(
            bus.clone(), events, wake.clone(), Arc::new(Vad(consumed.clone())),
            Arc::new(NoopDirectResponse), s.session.clone(), Uuid::new_v4(), binding,
            &SpeechConfig { channel_id: "test-device".into(), pre_roll_ms: 0, upload_queue_frames: 32, ..Default::default() },
            &identity, owner.stop.clone(),
        ));
        owner.speech.as_ref().unwrap().start().await.unwrap();
        let mut capture = Capture { bus, consumed, offset: 0 };
        capture.frame(0).await;
        #[cfg(target_os = "linux")]
        board.verify(wake.as_ref()).await;

        let mut retired: Option<(Scene, ResponseRecord, Arc<AudioSubmission>)> = None;
        for cycle in 0..2 {
            owner.stage = "natural wake and authenticated Ready";
            #[cfg(target_os = "linux")]
            board.reset_applied(wake.as_ref()).await;
            #[cfg(target_os = "linux")]
            board.wake(0x1200 + cycle);
            #[cfg(not(target_os = "linux"))]
            wake.0.store(true, Ordering::Release);
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    capture.frame(0).await;
                    if let Ok(ModuleEvent::WakeWordDetected(event)) = lifecycle.try_recv() {
                        #[cfg(target_os = "linux")]
                        assert_eq!(event.source, WakeWordSource::R1 { message_id: 0x1200 + cycle, angle_deg: Some(344.0), beam: Some(0) });
                        #[cfg(not(target_os = "linux"))]
                        assert_eq!(event.source, WakeWordSource::Local);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }).await.unwrap_or_else(|_| panic!("natural wake timeout in cycle {cycle}"));
            eventually(|| async { s.session.active_scope().unwrap().is_some() }).await;
            let ready = s.session.active_scope().unwrap().unwrap();
            assert_eq!(ready.generation_epoch, 1);
            if let Some((old, _, _)) = &retired {
                assert_ne!(ready.session_id, old.scope.session_id);
                assert_eq!(ready.device_boot_id, old.scope.device_boot_id);
            }
            assert_eq!(wire.events.lock().unwrap().iter().filter(|event| matches!(event.payload, Some(voice::session_event::Payload::Ready(_)))).count(), cycle as usize + 1);

            owner.stage = "continuous PCM reaches real server detector";
            let active_frames = if cycle == 0 { 200 } else { 1 };
            let mut expected = Vec::new();
            for frame in 1..=active_frames {
                capture.frame(frame as i16).await;
                expected.extend(std::iter::repeat_n(frame as i16, 1600));
                if frame == 150 || frame == 200 {
                    assert!(detections.try_recv().is_err(), "continuous active PCM must not synthesize an endpoint");
                    assert_eq!(s.session.active_scope().unwrap(), Some(ready));
                }
            }
            capture.silence().await;
            let detected = receive(&mut detections).await;
            let pcm: Vec<_> = detected.pcm.as_bytes().chunks_exact(2).map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]])).collect();
            let start = pcm.iter().position(|sample| *sample != 0).unwrap();
            assert!(start <= 4800, "only bounded server preroll precedes speech");
            assert_eq!(&pcm[start..start + expected.len()], expected);
            assert_eq!(&pcm[start + expected.len()..], vec![0; 11200]);
            detected.answer(TurnState::Complete, "deterministic response please");
            let input = receive(&mut recognized).await;
            assert_eq!(input.session_id, ready.session_id);
            assert_eq!(input.device_boot_id, ready.device_boot_id);
            assert_eq!(input.user_id, s.f.user.to_string());
            assert_eq!(input.channel_id, "test-device");
            s.scope = input.output.as_ref().unwrap().producer.scope();
            eventually(|| async { s.session.active_scope().unwrap() == Some(s.scope) }).await;
            let response = bind_response(&s.f, &s.service, &input).await;
            send_media(owner, &s, &response, &[17; 1600]).await;
            let original = s.device.submissions.lock().unwrap().values().find(|submission| submission.identity.scope == s.scope).unwrap().clone();
            s.device.submit_frames(&original.identity.playback_id.to_string(), 40);
            assert_eq!(original.snapshot(0).submitted_source_frames, Some(40));
            assert_eq!(original.snapshot(0).presented_source_frames, None);

            if cycle == 1 {
                owner.stage = "replacement excludes retired media";
                let (old, old_response, old_submission) = retired.take().unwrap();
                let mut late = old.audio(&old_response, &[99; 16]).await;
                late.output_key = "late-after-replacement".into();
                let result = s.service.deliver_audio(late).await;
                assert!(result.is_err() || result.is_ok_and(|record| record.status == DeliveryStatus::Cancelled));
                assert!(!s.device.submit_audio(&old_submission, 1600, &[99]).unwrap());
                assert_eq!(s.session.active_scope().unwrap(), Some(s.scope));
                assert_eq!(s.drain(), vec![17; 1560]);
                submitted(&owner.deliveries.pop().unwrap().await.unwrap().unwrap(), 1600);
                drop(input);
                break;
            }

            owner.stage = "simultaneous upload pauses original native output";
            capture.utterance(301).await;
            let backchannel = receive(&mut detections).await;
            s.command_count(&response, 1).await;
            assert_eq!(original.try_begin_output().unwrap_err(), OutputBlocked::Held);
            assert_eq!(s.session.active_scope().unwrap(), Some(s.scope));
            backchannel.answer(TurnState::Backchannel, "嗯");
            s.command_count(&response, 2).await;
            eventually(|| async { s.session.observe_control(&s.scope).unwrap().phase == ControlPhase::Running }).await;
            s.device.submit_frames(&original.identity.playback_id.to_string(), 40);
            assert_eq!(original.snapshot(1).submitted_source_frames, Some(80));
            assert!(recognized.try_recv().is_err(), "backchannel must not create a new response");

            owner.stage = "semantic cancel fences original media while capture survives";
            capture.utterance(302).await;
            receive(&mut detections).await.answer(TurnState::Complete, "停止");
            eventually(|| async { input.cancel.is_cancelled() && s.session.active_scope().unwrap().is_some_and(|scope| scope.session_id == ready.session_id && scope.generation_epoch == s.scope.generation_epoch + 1) }).await;
            assert!(s.device.state.lock().unwrap().1.is_empty());
            assert!(!s.device.submit_audio(&original, 1600, &[99]).unwrap());
            assert_eq!(owner.deliveries.pop().unwrap().await.unwrap().unwrap().status, DeliveryStatus::Cancelled);
            assert!(recognized.try_recv().is_err());

            // A new utterance in this same capture proves output cancellation did not stop upload.
            capture.utterance(303).await;
            receive(&mut detections).await.answer(TurnState::Complete, "another response before disconnect");
            let pending_input = receive(&mut recognized).await;
            assert_eq!(pending_input.session_id, ready.session_id);
            s.scope = pending_input.output.as_ref().unwrap().producer.scope();
            eventually(|| async { s.session.active_scope().unwrap() == Some(s.scope) }).await;
            let pending_response = bind_response(&s.f, &s.service, &pending_input).await;
            send_media(owner, &s, &pending_response, &[23; 1600]).await;
            let pending_submission = s.device.submissions.lock().unwrap().values().find(|submission| submission.identity.scope == s.scope).unwrap().clone();
            retired = Some((s.clone(), pending_response.clone(), pending_submission));
            let disconnected_scope = s.scope;
            owner.stage = "transport disconnect drains owners and requires fresh wake";
            wire.disconnect.lock().unwrap().as_ref().unwrap().cancel();
            eventually(|| async { pending_input.cancel.is_cancelled() && s.session.active_scope().unwrap().is_none() && owner.tasks.active() == 0 }).await;
            tokio::time::timeout(Duration::from_secs(3), async {
                while !matches!(lifecycle.recv().await.unwrap(), ModuleEvent::SpeechEnd) {}
            }).await.unwrap();
            assert_eq!(owner.deliveries.pop().unwrap().await.unwrap().unwrap().status, DeliveryStatus::Cancelled);
            assert!(s.device.state.lock().unwrap().1.is_empty());
            let mut late = s.audio(&pending_response, &[99; 16]).await;
            late.output_key = "late-after-disconnect".into();
            let result = s.service.deliver_audio(late).await;
            assert!(result.is_err() || result.is_ok_and(|record| record.status == DeliveryStatus::Cancelled));
            capture.frame(0).await;
            assert!(s.session.active_scope().unwrap().is_none(), "old audio cannot silently reconnect");
            assert_eq!(disconnected_scope.session_id, ready.session_id);
            assert!(s.device.state.lock().unwrap().1.is_empty());
            drop(pending_input);
            drop(input);
            // Drain the completed session's lifecycle messages before the next wake assertion.
            while lifecycle.try_recv().is_ok() {}
        }
        assert_eq!(s.f.count("delivery_responses").await, 3);
        assert_eq!(s.wire.pushes.load(Ordering::SeqCst), 3);
        owner.stage = "body complete";
    }), SpeechFixture::close).await;
}
