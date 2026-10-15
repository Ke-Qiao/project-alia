//! Complete scanner -> live actor -> fixed TTS -> paired PlaybackService path.
//! PostgreSQL, both gRPC services and ownership are real. Detector/TTS/native
//! samples are controlled fixtures; this is not acoustic or model validation.
use super::*;
use alia_core::voice::{DetectionId, ModelIdentity, PcmWindow, TurnObservation, TurnState};
use alia_proto::voice::{
    self, device_frame, session_event, voice_session_client::VoiceSessionClient,
    voice_session_server::VoiceSessionServer,
};
use alia_server::modules::interaction::conversation::ConversationStore;
use alia_server::{
    bootstrap::ServerBuilder,
    grpc::{voice_auth::VoiceIngressAuth, voice_session::VoiceSessionService},
    modules::{
        companion::types::{CompanionMode, CompanionPolicy},
        delivery::audio::EncodedAudio,
        interaction::turn_detector::{DetectionError, TurnDetector},
        memory::{context::ContextSource, source::capture_source_epochs},
    },
    providers::adapter::{TtsAdapter, TtsAudioStream, TtsRequest},
};
use sea_orm::{DatabaseBackend, Statement};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;

pub(super) fn binding_service(
    binding: Arc<BindingManager>,
    directory: &std::path::Path,
) -> alia_embedded::grpc::DeviceBindingService {
    let mut config = alia_embedded::config::EmbeddedConfig::default();
    config.identity.device_id = "companion-paired-device".into();
    config.config_path = directory.join("embedded.toml");
    alia_embedded::grpc::DeviceBindingService::new(
        binding,
        config.identity.clone(),
        Arc::new(alia_embedded::module::noop::NoopDiscovery),
        Arc::new(alia_embedded::plugin::PluginRegistry::new(
            directory.join("plugins"),
        )),
        Arc::new(alia_embedded::module::motion::MotionRuntime::disabled(
            config.motion.clone(),
        )),
        Arc::new(alia_embedded::settings::EmbeddedSettingsManager::new(
            &config,
        )),
        ed25519_dalek::SigningKey::from_bytes(&[19; 32])
            .verifying_key()
            .to_bytes()
            .to_vec(),
    )
}

#[derive(Clone)]
struct FixedTts {
    calls: Arc<AtomicUsize>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
#[tonic::async_trait]
impl ServerLifecycle for FixedTts {
    fn name(&self) -> &'static str {
        "fixed-companion-tts"
    }
}
#[tonic::async_trait]
impl TtsAdapter for FixedTts {
    async fn synthesize_request(
        &self,
        request: &TtsRequest,
        budget: ResponseAudioBudget,
    ) -> anyhow::Result<EncodedAudio> {
        assert_eq!(request.text, "如果你愿意，我们可以再聊一会儿。");
        assert_eq!(
            self.calls.fetch_add(1, Ordering::SeqCst),
            0,
            "invitation must never synthesize twice"
        );
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        // A small valid mono PCM16 WAV; the real channel decodes this header.
        let samples = [17i16; 160];
        let data: Vec<_> = samples.into_iter().flat_map(i16::to_le_bytes).collect();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&32000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        budget.copy_from_slice(&wav).await
    }
    async fn synthesize_stream_request(
        &self,
        request: &TtsRequest,
        budget: ResponseAudioBudget,
    ) -> anyhow::Result<TtsAudioStream> {
        let audio = self.synthesize_request(request, budget).await?;
        Ok(Box::pin(futures::stream::iter([Ok(audio)])))
    }
}
struct ModelCall(DetectionId, oneshot::Sender<TurnObservation>);
impl ModelCall {
    fn answer(self, state: TurnState, transcript: &str) {
        self.1
            .send(TurnObservation {
                id: self.0,
                state,
                transcript: transcript.into(),
                inference_ms: 1.0,
                model: ModelIdentity {
                    source: "paired-test".into(),
                    revision: "1".into(),
                    device: "test".into(),
                },
            })
            .unwrap();
    }
}
struct ControlledDetector(mpsc::Sender<ModelCall>);
#[tonic::async_trait]
impl TurnDetector for ControlledDetector {
    async fn detect(
        &self,
        id: DetectionId,
        _: &PcmWindow,
        cancel: &CancellationToken,
    ) -> Result<TurnObservation, DetectionError> {
        let (reply, receive) = oneshot::channel();
        self.0
            .send(ModelCall(id, reply))
            .await
            .map_err(|_| DetectionError::Cancelled)?;
        tokio::select! {
            _=cancel.cancelled()=>Err(DetectionError::Cancelled),
            result=receive=>result.map_err(|_|DetectionError::Cancelled),
        }
    }
}
enum CaptureCommand {
    Speech {
        upload: bool,
        reply: oneshot::Sender<u64>,
    },
    Flush(oneshot::Sender<u64>),
}
async fn speech(capture: &mpsc::Sender<CaptureCommand>, upload: bool) -> u64 {
    let (reply, receive) = oneshot::channel();
    capture
        .send(CaptureCommand::Speech { upload, reply })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), receive)
        .await
        .unwrap()
        .unwrap()
}
async fn next_detection(calls: &mut mpsc::Receiver<ModelCall>) -> ModelCall {
    tokio::time::timeout(Duration::from_secs(2), calls.recv())
        .await
        .unwrap()
        .unwrap()
}
#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    Complete,
    Revoke,
    ResumeRace,
}

async fn resume_race(
    s: &Scene,
    response: &ResponseRecord,
    capture: &mpsc::Sender<CaptureCommand>,
    calls: &mut mpsc::Receiver<ModelCall>,
) {
    let original = s.submission().unwrap();
    let generation = original.generation;
    s.device
        .submit_frames(&original.identity.playback_id.to_string(), 40);
    assert_eq!(original.snapshot(1).submitted_source_frames, Some(40));
    speech(capture, true).await;
    s.command_count(response, 1).await;
    let first = next_detection(calls).await;
    s.wire.hold_resume_ack.store(true, Ordering::SeqCst);
    first.answer(TurnState::Backchannel, "嗯");
    signal(&s.wire.resume_applied).await;
    let newer = speech(capture, false).await;
    let local = s.session.observe_control(&s.scope).unwrap();
    assert_eq!(local.phase, ControlPhase::Held);
    assert_eq!(
        local.hold_id, None,
        "old formal H1 really completed on the device"
    );
    assert_eq!(local.local_capture_hold.unwrap().through_sequence, newer);
    assert_eq!(
        original.try_begin_output().unwrap_err(),
        OutputBlocked::Held
    );
    s.wire.release_resume.add_permits(1);
    s.command_count(response, 3).await;
    assert_eq!(
        s.wire.observations.load(Ordering::SeqCst),
        1,
        "lost ACK is observed, not replayed"
    );
    let commands = s.wire.controls.lock().unwrap().clone();
    assert_eq!(commands.len(), 3);
    assert!(
        commands.iter().all(|c| c.binding == commands[0].binding
            && c.expected_backend == commands[0].expected_backend)
    );
    assert_eq!(commands[0].command.sequence, 1);
    assert_eq!(commands[1].command.sequence, 2);
    assert_eq!(commands[2].command.sequence, 3);
    assert_eq!(commands[0].command.hold_id, commands[1].command.hold_id);
    assert_ne!(commands[0].command.hold_id, commands[2].command.hold_id);
    assert!(
        matches!(commands[2].command.action,ControlAction::Pause{reason:HoldReason::AcousticCandidate,through_sequence,..} if through_sequence==newer)
    );
    assert_eq!(commands[2].binding.scope, s.scope);
    let held = s.session.observe_control(&s.scope).unwrap();
    assert_eq!(held.phase, ControlPhase::Held);
    assert_eq!(held.hold_id, Some(commands[2].command.hold_id));
    assert_eq!(original.snapshot(2).submitted_source_frames, Some(40));
    assert_eq!(original.generation, generation);
    let (reply, receive) = oneshot::channel();
    capture.send(CaptureCommand::Flush(reply)).await.unwrap();
    assert_eq!(receive.await.unwrap(), newer);
    next_detection(calls)
        .await
        .answer(TurnState::Complete, "继续");
    s.command_count(response, 4).await;
    let running = s.session.observe_control(&s.scope).unwrap();
    assert_eq!(running.phase, ControlPhase::Running);
    assert_eq!(running.local_capture_hold, None);
    let commands = s.wire.controls.lock().unwrap();
    assert_eq!(commands.len(), 4);
    assert_eq!(commands[3].command.hold_id, commands[2].command.hold_id);
    assert_eq!(commands[3].binding, commands[0].binding);
    assert!(matches!(
        commands[3].command.action,
        ControlAction::Resume {
            authorization: ResumeAuthorization {
                kind: ResumeKind::ExplicitContinue,
                ..
            }
        }
    ));
    assert_eq!(original.snapshot(3).submitted_source_frames, Some(40));
}
async fn count(s: &Scene, table: &str) -> i64 {
    s.f.db
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("SELECT count(*) AS n FROM {table}"),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}

/// Failure-only evidence before shutdown changes candidate states. Never print
/// identities, source text, private grants or raw database errors.
async fn failure_state(
    db: &sea_orm::DatabaseConnection,
    shadow_scan: impl std::fmt::Debug,
) -> String {
    let started = std::time::Instant::now();
    let rows = tokio::time::timeout(Duration::from_secs(1), async {
        let row =
            db
                .query_one(Statement::from_string(
                    DatabaseBackend::Postgres,
                    "SELECT jsonb_build_object(
                'appraisals', (SELECT count(*) FROM appraisal_records),
                'responses', (SELECT count(*) FROM delivery_responses),
                'source_claims', (SELECT coalesce(jsonb_agg(v), '[]'::jsonb) FROM (
                    SELECT origin_mode, count(*) AS count
                    FROM companion_source_claims GROUP BY origin_mode ORDER BY origin_mode) v),
                'rejected_sources', (SELECT count(*) FROM companion_rejected_sources),
                'pending', (SELECT coalesce(jsonb_agg(v), '[]'::jsonb) FROM (
                    SELECT state, retirement_reason, count(*) AS count
                    FROM companion_live_pending GROUP BY state, retirement_reason
                    ORDER BY state, retirement_reason) v),
                'admissions', (SELECT coalesce(jsonb_agg(v), '[]'::jsonb) FROM (
                    SELECT state, retirement_reason, count(*) AS count
                    FROM companion_admissions GROUP BY state, retirement_reason
                    ORDER BY state, retirement_reason) v),
                'task_db_waits', (SELECT coalesce(jsonb_agg(v), '[]'::jsonb) FROM (
                    SELECT state, wait_event_type, count(*) AS count
                    FROM pg_stat_activity WHERE datname=current_database()
                    GROUP BY state, wait_event_type ORDER BY state, wait_event_type) v)) AS summary"
                        .to_string(),
                ))
                .await?
                .ok_or_else(|| anyhow::anyhow!("missing diagnostic row"))?;
        Ok::<serde_json::Value, anyhow::Error>(row.try_get("", "summary")?)
    })
    .await;
    let rows = match rows {
        Ok(Ok(rows)) => rows.to_string(),
        Ok(Err(_)) => "unavailable".into(),
        Err(_) => "diagnostic deadline exceeded".into(),
    };
    format!(
        "shadow_scan={shadow_scan:?}; diagnostic_wait_ms={} (test_process_monotonic, pool/query combined); rows={rows}",
        started.elapsed().as_millis()
    )
}

async fn seed(state: &alia_server::state::AppState, owner: Uuid) -> Uuid {
    let conversation = state
        .modules
        .conversation
        .get_or_create_active_session("test-device", &owner.to_string())
        .await
        .unwrap();
    let stamps = capture_source_epochs(&state.db, owner).await.unwrap();
    let text = "thank you";
    let source = state
        .modules
        .conversation
        .append_sourced_message(
            owner,
            conversation,
            alia_server::providers::adapter::ChatMessage::new("user", text),
            chrono::Utc::now(),
            stamps.privacy_epoch,
            stamps.relationship_epoch,
        )
        .await
        .unwrap();
    let received = state
        .db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT source_received_at FROM chat_messages WHERE id=$1",
            [source.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "source_received_at")
        .unwrap();
    let context = state
        .modules
        .context
        .materialize(
            ContextSource {
                owner_id: owner,
                request_id: Uuid::new_v4(),
                channel_id: "test-device".into(),
                conversation_id: Some(conversation),
                source_event_id: Some(source),
                source_received_at: received,
                source_privacy_epoch: stamps.privacy_epoch,
            },
            text,
        )
        .await
        .unwrap();
    state
        .modules
        .expression
        .observe_context_user(&state.modules.context, &context)
        .await
        .unwrap();
    source
}

async fn complete_invitation(scenario: Scenario) {
    // Only fixed, identity-free admission diagnostics are enabled. Never
    // inherit a broad RUST_LOG filter that could include requests or grants.
    let _ = tracing_subscriber::fmt()
        .with_env_filter("off,alia_server::modules::companion::runner=debug,alia_server::modules::companion::service=warn")
        .without_time()
        .with_ansi(false)
        .with_test_writer()
        .try_init();
    let revoke_during_tts = scenario == Scenario::Revoke;
    with_db(|f| async move {
        let paired = Runtime::new(f,4096).await;
        let mut s = paired.scene.clone();
        // This case uses the builder-owned Delivery/Companion lifecycle.
        s.service.shutdown().await.unwrap();
        let mut config: alia_server::config::ServerConfig = toml::from_str(include_str!("../../../../config/server.example.toml")).unwrap();
        config.config_dir = paired._directory.0.clone();
        config.auth.server_keypair_path = paired._directory.0.join("server.key");
        config.server.id = "server".into();
        config.server.host = "127.0.0.1".into(); config.server.port = 0; config.server.grpc_port = None;
        config.server.discovery_enabled = false; config.server.shutdown_grace_ms = 5000;
        config.channels.clear(); config.modules.stt="noop".into(); config.modules.tts="noop".into();
        config.llm.roles.minor="noop".into(); config.llm.roles.major="noop".into(); config.llm.roles.default="noop".into(); config.llm.roles.embedding="noop".into();
        config.llm.roles.memory_capture=None;config.llm.roles.sentence_expression=None;
        config.policies.insights.enabled=false;config.plugins.home_assistant.enabled=false;config.providers.easy_turn.enabled=false;
        let tts=FixedTts {calls:Arc::new(AtomicUsize::new(0)),entered:Arc::new(Semaphore::new(0)),release:Arc::new(Semaphore::new(0))};
        let runtime = ServerBuilder::new(Arc::new(config),s.f.db.clone(),EventBus::new(64),CancellationToken::new()).with_tts(tts.clone()).build().await.unwrap();
        let state=runtime.state().clone();
        state.channels.register(s.channel.clone()).await.unwrap();
        // The real connection hydrates its bound peer from DeviceBindingService;
        // a configured pin alone cannot replace that runtime identity.
        s.channel.binding_client().await.unwrap();
        s.service = state.delivery.clone();
        let stop=CancellationToken::new();
        let auth=VoiceIngressAuth::new(state.channels.clone());
        let (model_tx,mut model_calls)=mpsc::channel(2);
        let voice=VoiceSessionService::new(auth,Some(Arc::new(ControlledDetector(model_tx))),state.voice_input.clone(),state.delivery.clone(),Duration::from_secs(30),stop.clone(),state.runtime_tasks.clone())
            .with_initiatives(state.voice_initiatives.clone(),state.modules.context.clone());
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint=format!("http://{}",listener.local_addr().unwrap());
        let shutdown=stop.clone();
        let task=tokio::spawn(async move {
            let incoming=async_stream::stream! {loop {yield listener.accept().await.map(|(stream,_)|stream);}};
            tonic::transport::Server::builder().add_service(VoiceSessionServer::new(voice)).serve_with_incoming_shutdown(incoming,shutdown.cancelled()).await.unwrap();
        });
        let voice_server=RpcServer{stop:stop.clone(),task:Some(task)};
        let mut heartbeat=None;
        let mut stage="connect voice client";
        let result=AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(25),async {
            let (tx,rx)=mpsc::channel(8);
            let boot=Uuid::new_v4();
            tx.send(voice::DeviceFrame{payload:Some(device_frame::Payload::Open(voice::OpenSession{
                protocol_version:VOICE_CONTRACT_VERSION,device_boot_id:boot.to_string(),channel_id:"test-device".into(),software_hold:true,max_hold_ms:MAX_HOLD_MS,initiative_admission:true,initiative_capture_gate_version:INITIATIVE_CAPTURE_GATE_VERSION,
            }))}).await.unwrap();
            let key=ed25519_dalek::SigningKey::from_bytes(&[19;32]);
            let subject=alia_transport::embedded_device_auth_subject("companion-paired-device",key.verifying_key().as_bytes());
            let token=alia_transport::mint_auth_token(&key,&subject,alia_transport::EMBEDDED_INGRESS_SCOPE,Duration::from_secs(60)).unwrap();
            let mut request=Request::new(ReceiverStream::new(rx));
            request.metadata_mut().insert("authorization",format!("Bearer {token}").parse().unwrap());
            request.metadata_mut().insert("x-alia-channel-id", "test-device".parse().unwrap());
            let mut client=VoiceSessionClient::connect(endpoint).await.unwrap();
            stage="open voice stream";
            let mut events=client.stream(request).await.unwrap().into_inner();
            stage="receive Ready";
            let ready=events.message().await.unwrap().unwrap();
            assert!(matches!(ready.payload,Some(session_event::Payload::Ready(ref v)) if v.initiative_capture_gate_version==1));
            s.scope=VoiceScope{session_id:ready.session_id.parse().unwrap(),device_boot_id:boot,generation_epoch:ready.generation_epoch};
            s.capture=CaptureControlBarrier::default();
            s.session.activate_with_capture(&s.scope,s.capture.clone(),MAX_HOLD_MS).unwrap();
            let capture=s.capture.clone();let session=s.session.clone();let shutdown=stop.clone();
            let (capture_tx,mut capture_rx)=mpsc::channel(2);
            heartbeat=Some(tokio::spawn(async move {
                let mut sequence=0;let mut tick=tokio::time::interval(Duration::from_millis(100));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut withheld:Option<(voice::DeviceFrame,u64)>=None;
                loop {
                    let command=tokio::select! { biased;
                        _=shutdown.cancelled()=>break,
                        command=capture_rx.recv()=>match command {Some(command)=>Some(command),None=>break},
                        _=tick.tick(),if withheld.is_none()=>None,
                    };
                    let (speech_active,upload,reply)=match command {
                        Some(CaptureCommand::Flush(reply))=>{
                            let (frame,n)=withheld.take().expect("original local frame must be retained");
                            if tx.send(frame).await.is_err(){break;}
                            let _=reply.send(n);continue;
                        },
                        Some(CaptureCommand::Speech{upload,reply})=>(true,upload,Some(reply)),
                        None=>(false,true,None),
                    };
                    assert!(withheld.is_none(),"capture may not skip its original queued frame");
                    let frame=voice::DeviceFrame{payload:Some(device_frame::Payload::Audio(voice::CaptureFrame{
                        sequence,sample_offset:sequence*1600,pcm_s16le:vec![0;3200],speech_active,
                    }))};
                    if session.accept_capture(&capture,sequence,speech_active,||Ok(())).is_err(){break;}
                    if upload { if tx.send(frame).await.is_err(){break;} }
                    else {withheld=Some((frame,sequence));}
                    if let Some(reply)=reply {let _=reply.send(sequence);}
                    sequence+=1;
                }
            }));
            stage="enable companion policy";
            let policy=state.modules.companion.policy(s.f.user).await.unwrap();
            assert_eq!(policy.policy.mode,CompanionMode::Shadow);
            state.modules.companion.replace_policy(s.f.user,policy.revision,CompanionPolicy{mode:CompanionMode::Enabled,..policy.policy}).await.unwrap();
            stage="seed source and appraisal";
            let source=seed(&state,s.f.user).await;
            stage="receive InitiativeAdmitted";
            let event=events.message().await.unwrap().unwrap();
            let admission=match event.payload {Some(session_event::Payload::InitiativeAdmitted(v))=>v,other=>panic!("expected initiative: {other:?}")};
            s.scope.generation_epoch=event.generation_epoch;
            s.session.admit_initiative_event(&s.scope,InitiativeCapture{initiative_id:admission.initiative_id.parse().unwrap(),request_id:admission.request_id.parse().unwrap(),through_sequence:admission.through_sequence},admission.context_id.parse().unwrap()).unwrap();
            stage="enter original TTS";
            signal(&tts.entered).await;
            stage="verify original admission";
            assert_eq!(count(&s,"companion_admissions").await,1);
            assert_eq!(count(&s,"delivery_responses").await,1);
            assert_eq!(s.wire.registrations.lock().unwrap().len(),1);
            let row=s.f.db.query_one(Statement::from_string(DatabaseBackend::Postgres,"SELECT response_id,request_id,source_event_id,context_id FROM delivery_responses".to_string())).await.unwrap().unwrap();
            let response_id:Uuid=row.try_get("","response_id").unwrap();
            assert_eq!(row.try_get::<Uuid>("","source_event_id").unwrap(),source);
            assert_eq!(row.try_get::<Uuid>("","request_id").unwrap().to_string(),admission.request_id);
            let registered=s.wire.registrations.lock().unwrap()[0].clone();
            assert_eq!(registered.binding.scope,s.scope);
            assert_eq!(registered.binding.response_id,response_id);
            assert_eq!(registered.binding.context_id,row.try_get::<Uuid>("","context_id").unwrap());
            assert_eq!(registered.binding.context_id.to_string(),admission.context_id);
            assert_eq!(registered.binding.initiative_capture.unwrap().through_sequence,admission.through_sequence);
            if revoke_during_tts {
                stage="revoke original source";
                state.modules.context.clear_relationship(s.f.user,Uuid::new_v4(),0).await.unwrap();
            }
            tts.release.add_permits(1);
            if revoke_during_tts {
                stage="observe retired admission";
                eventually(||async {
                    let row=s.f.db.query_one(Statement::from_string(DatabaseBackend::Postgres,"SELECT state FROM companion_admissions".to_string())).await.unwrap().unwrap();
                    row.try_get::<String>("","state").unwrap()=="retired"
                }).await;
                assert_eq!(s.wire.pushes.load(Ordering::SeqCst),0);
                assert_eq!(count(&s,"companion_admissions").await,1,"erasure cannot refund charge");
            } else {
                stage="accept original media end";
                eventually(||async {s.submission().is_some_and(|v|v.snapshot(1).end_accepted)}).await;
                if scenario == Scenario::ResumeRace {
                    stage="complete H1 to H2 handoff";
                    let response=s.service.get_response(s.f.user,response_id).await.unwrap();
                    resume_race(&s,&response,&capture_tx,&mut model_calls).await;
                    assert_eq!(s.drain(),vec![17;120],"resume must keep exactly the original unsubmitted suffix");
                } else {
                    assert!(model_calls.try_recv().is_err(),"silent invitation cannot invoke turn inference");
                    assert_eq!(s.drain(),vec![17;160]);
                }
                stage="observe original submission";
                eventually(||async {
                    let records=s.service.list_deliveries(s.f.user,response_id).await.unwrap();
                    records.len()==1 && records[0].status==DeliveryStatus::Submitted
                }).await;
                let records=s.service.list_deliveries(s.f.user,response_id).await.unwrap();
                submitted(&records[0],160);
                assert_eq!(count(&s,"chat_messages").await,2);
                assert_eq!(s.wire.pushes.load(Ordering::SeqCst),1);
                assert_eq!(s.device.submissions.lock().unwrap().len(),1);
            }
            stage="verify completed output";
            assert_eq!(tts.calls.load(Ordering::SeqCst),1);
            assert_eq!(count(&s,"delivery_responses").await,1,"H2 must retain the sole original Response");
            assert_eq!(count(&s,"companion_admissions").await,1,"control recovery cannot charge another initiative");
            assert_eq!(count(&s,"execution_effects").await,0);
        })).catch_unwind().await;
        let failure_diagnostic = if !matches!(&result, Ok(Ok(()))) {
            let diagnostic = failure_state(&s.f.db, state.modules.companion.scan_progress()).await;
            eprintln!("complete companion path failed before cleanup at {stage}; {diagnostic}");
            Some(diagnostic)
        } else {
            None
        };
        stop.cancel();
        if let Some(task)=heartbeat {task.await.unwrap();}
        tts.release.add_permits(1);
        s.wire.release_resume.add_permits(8);
        let shutdown=runtime.shutdown().await;
        voice_server.close().await;
        paired.close().await;
        shutdown.unwrap();
        if revoke_during_tts {
            // Retirement is recorded immediately by erasure. Only completed
            // shutdown proves the original runner/native owners cannot later
            // publish the synthesis that was just released above.
            assert_eq!(s.wire.pushes.load(Ordering::SeqCst), 0);
        }
        match result {Ok(Ok(()))=>(),Ok(Err(_))=>panic!("complete companion path exceeded 25 seconds at {stage}; {}",failure_diagnostic.unwrap()),Err(error)=>std::panic::resume_unwind(error)}
    }).await;
}
#[tokio::test]
async fn scanner_commits_one_fixed_invitation_through_real_voice_and_paired_playback() {
    complete_invitation(Scenario::Complete).await;
}
#[tokio::test]
async fn source_erasure_during_original_tts_prevents_first_media_and_preserves_charge() {
    complete_invitation(Scenario::Revoke).await;
}

#[tokio::test]
async fn applied_resume_lost_ack_and_new_local_speech_handoff_to_h2_on_original_output() {
    complete_invitation(Scenario::ResumeRace).await;
}

#[tokio::test]
async fn failure_diagnostic_reports_live_claims_without_private_values() {
    with_db(|f| async move {
        let source = Uuid::new_v4();
        let appraisal = Uuid::new_v4();
        let generator = "private-diagnostic-generator";
        f.db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO companion_source_claims(owner_id,source_event_id,kind,generator_version,origin_mode,policy_revision) VALUES($1,$2,'follow_up_invitation',$3,'enabled',1)",
            vec![f.user.into(), source.into(), generator.into()],
        )).await.unwrap();
        f.db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO companion_rejected_sources(owner_id,appraisal_id) VALUES($1,$2)",
            vec![f.user.into(), appraisal.into()],
        )).await.unwrap();
        let companion = alia_server::modules::companion::CompanionService::new(
            f.db.clone(),
            alia_server::modules::interaction::initiative::VoiceInitiativeHandle::default(),
            CancellationToken::new(),
        );
        let progress = companion.scan_progress();
        assert_eq!((progress.batches_started, progress.batches_completed), (0, 0));
        let summary = failure_state(&f.db, progress).await;
        assert!(summary.starts_with("shadow_scan="));
        let rows: serde_json::Value =
            serde_json::from_str(summary.split_once("; rows=").unwrap().1).unwrap();
        assert_eq!(rows["source_claims"], json!([{"origin_mode":"enabled","count":1}]));
        assert_eq!(rows["rejected_sources"], 1);
        for private in [f.user.to_string(), source.to_string(), appraisal.to_string(), generator.into()] {
            assert!(!summary.contains(&private), "diagnostic leaked a private value");
        }

        f.db.execute_unprepared("ALTER TABLE companion_source_claims RENAME COLUMN origin_mode TO private_missing_origin_mode")
            .await.unwrap();
        let unavailable = failure_state(&f.db, companion.scan_progress()).await;
        assert_eq!(unavailable.split_once("; rows=").unwrap().1, "unavailable");
    }).await;
}
