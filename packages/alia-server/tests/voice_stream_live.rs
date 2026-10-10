//! Opt-in gRPC -> production Rust detector -> pinned Easy-Turn, with real PG
//! source/Context/Response/control admissions. Device auth and software output
//! are explicit fixtures; no physical microphone, VAD, LLM/TTS or speaker claim.
//!
//! ALIA_EASY_TURN_PCM_DIR must be an absolute directory containing PCM16 mono
//! 16 kHz conversions of the pinned official complete_real_1, backchannel_real_1,
//! and wait_real_1 WAV samples. Conversion: alia_easy_turn.smoke.pcm16.
#![cfg(all(feature = "channel-embedded", feature = "turn-easy"))]
use alia_server::modules::delivery::transport::DeliveryTarget;
mod delivery_support;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alia_core::delivery::VoiceScope;
use alia_core::{
    delivery::{SubmissionBackend, SubmissionBoundary},
    output_control::{
        BoundControlCommand, BoundControlReceipt, ControlMode, OutputBinding, OutputRegistration,
        VOICE_CONTRACT_VERSION,
    },
    playback_control::{
        CommandOutcome, ControlAction, ControlPhase, ControlReceipt, HoldReason, MAX_HOLD_MS,
        PlaybackControlState, RetiredReason,
    },
};
use alia_proto::voice::voice_session_client::VoiceSessionClient;
use alia_proto::voice::voice_session_server::VoiceSessionServer;
use alia_proto::voice::{
    CaptureFrame, CloseSession, DeviceFrame, OpenSession, OutputDecision, SessionEvent, TurnState,
    device_frame, session_event,
};
use alia_server::adapters::easy_turn::{EasyTurnConfig, EasyTurnDetector};
use alia_server::channels::ChannelRegistry;
use alia_server::channels::adapter::{ChannelAction, ChannelAdapter, ChannelCapabilities};
use alia_server::grpc::voice_auth::VoiceIngressAuth;
use alia_server::grpc::voice_session::VoiceSessionService;
use alia_server::modules::interaction::voice_input;
use alia_server::modules::platform::lifecycle::ModuleLifecycle;
use alia_server::modules::{
    delivery::{
        DeliveryService,
        repository::control::{ControlCommandStatus, OutputControlView},
        types::{NewResponse, ResponseKey},
    },
    memory::{context::ContextSource, source::capture_source_epochs},
};
use async_trait::async_trait;
use delivery_support::{Fixture, with_db};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Streaming};
use uuid::Uuid;

const CHANNEL: &str = "official-audio-test";
const TEST_TOKEN: &str = "injected-test-credential";
const MODEL_REVISION: &str = "7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98";
const NEGOTIATED_HOLD_MS: u64 = 30_000;

struct TestChannel {
    fences: Arc<Mutex<Vec<(VoiceScope, bool)>>>,
    owner: Uuid,
    db: DatabaseConnection,
    output: Mutex<LocalOutput>,
}

#[derive(Default)]
struct LocalOutput {
    through: Option<u64>,
    speech: Option<u64>,
    registered: Option<RegisteredOutput>,
}
struct RegisteredOutput {
    registration: OutputRegistration,
    control: PlaybackControlState,
    started: std::time::Instant,
    observation: u64,
}
impl TestChannel {
    fn capture(&self, sequence: u64, speech: bool) {
        let mut output = self.output.lock().unwrap();
        assert!(
            output
                .through
                .is_none_or(|previous| previous + 1 == sequence)
        );
        output.through = Some(sequence);
        if speech {
            output.speech = Some(sequence);
        }
        if let Some(registered) = &mut output.registered {
            registered
                .control
                .observe_capture(sequence, speech, 0)
                .unwrap();
        }
    }
    fn receipt(
        &self,
        command: BoundControlCommand,
        apply: bool,
    ) -> anyhow::Result<BoundControlReceipt> {
        command.validate()?;
        let mut output = self.output.lock().unwrap();
        let registered = output
            .registered
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("output not registered"))?;
        anyhow::ensure!(
            command.binding == registered.registration.binding
                && command.expected_backend == registered.registration.backend,
            "original output changed"
        );
        let elapsed = registered.started.elapsed().as_millis() as u64;
        let receipt = if apply {
            registered.control.apply(&command.command, elapsed, true)
        } else {
            let outcome = registered
                .control
                .recorded_outcome(&command.command)
                .ok_or_else(|| anyhow::anyhow!("unknown command"))?;
            ControlReceipt {
                command_id: command.command.command_id,
                outcome,
                snapshot: registered.control.snapshot(elapsed, true),
            }
        };
        registered.observation += 1;
        Ok(BoundControlReceipt {
            intent_digest: command.intent_digest(),
            command,
            observation_sequence: registered.observation,
            backend: registered.registration.backend.clone(),
            mode: ControlMode::SoftwareGate,
            native_writes_quiescent: true,
            receipt,
            progress: vec![],
        })
    }
}

#[async_trait]
impl ModuleLifecycle for TestChannel {
    fn name(&self) -> &'static str {
        "injected-live-test-channel"
    }
}

impl TestChannel {
    async fn retire_model_scope(&self, target: &DeliveryTarget, _: &str) -> anyhow::Result<()> {
        let scope = target.voice.expect("stop must name the original scope");
        let mut output = self.output.lock().unwrap();
        if let Some(registered) = &mut output.registered {
            anyhow::ensure!(
                registered.registration.binding.scope == scope,
                "wrong stop scope"
            );
            registered.control.retire(RetiredReason::Cancelled);
        }
        Ok(())
    }
}

#[async_trait]
impl alia_server::modules::delivery::transport::DeliveryTransport for TestChannel {
    fn owner_user_id(&self) -> Option<Uuid> {
        Some(self.owner)
    }

    fn channel_id(&self) -> &str {
        CHANNEL
    }

    async fn retire_voice_output(
        &self,
        request: &alia_core::output_control::OutputRetirementRequest,
    ) -> anyhow::Result<alia_core::output_control::OutputRetirementReceipt> {
        use alia_core::output_control::{OutputRetirementEvidence, OutputRetirementReceipt};
        request.validate()?;
        anyhow::ensure!(
            self.fences
                .lock()
                .unwrap()
                .iter()
                .any(|(scope, close)| *scope == request.scope && !close),
            "unknown live output scope"
        );
        let registered = self
            .output
            .lock()
            .unwrap()
            .registered
            .as_ref()
            .map(|output| output.registration.clone());
        let evidence = if let Some(registered) = registered {
            anyhow::ensure!(
                request.binding == Some(registered.binding)
                    && request
                        .expected_backend
                        .as_ref()
                        .is_none_or(|backend| *backend == registered.backend),
                "original live output or backend changed"
            );
            self.retire_model_scope(
                &DeliveryTarget {
                    user_id: self.owner.to_string(),
                    voice: Some(request.scope),
                },
                "original live output ended",
            )
            .await?;
            // This fixture registers a software control owner but never starts
            // PCM or native writes, so there are no playback facts to settle.
            OutputRetirementEvidence::Settled {
                backend: registered.backend,
                progress: vec![],
            }
        } else {
            anyhow::ensure!(
                request.expected_backend.is_none(),
                "registered backend disappeared"
            );
            OutputRetirementEvidence::NoNativeOwner
        };
        let receipt = OutputRetirementReceipt {
            scope: request.scope,
            binding: request.binding,
            evidence,
        };
        receipt.validate(request)?;
        Ok(receipt)
    }

    async fn register_output(&self, binding: OutputBinding) -> anyhow::Result<OutputRegistration> {
        anyhow::ensure!(binding.is_valid(), "invalid output binding");
        let mut output = self.output.lock().unwrap();
        if let Some(current) = &output.registered {
            anyhow::ensure!(
                current.registration.binding == binding,
                "cannot replace original registration"
            );
            return Ok(current.registration.clone());
        }
        let registration = OutputRegistration {
            binding,
            backend: SubmissionBackend {
                instance_id: Uuid::new_v4(),
                boundary: SubmissionBoundary::CpalCallbackBuffer,
                output_sample_rate_hz: 48000,
                output_channels: 2,
            },
            maximum_hold_ms: MAX_HOLD_MS,
            initiative_capture_gate_version: 0,
        };
        registration.validate()?;
        let mut control = PlaybackControlState::new(binding.scope).unwrap();
        control
            .initialize_capture(output.through, output.speech)
            .unwrap();
        output.registered = Some(RegisteredOutput {
            registration: registration.clone(),
            control,
            started: std::time::Instant::now(),
            observation: 0,
        });
        Ok(registration)
    }
    async fn control_output(
        &self,
        command: &BoundControlCommand,
    ) -> anyhow::Result<BoundControlReceipt> {
        self.receipt(command.clone(), true)
    }
    async fn observe_output_control(
        &self,
        command: &BoundControlCommand,
    ) -> anyhow::Result<BoundControlReceipt> {
        self.receipt(command.clone(), false)
    }
}

#[async_trait::async_trait]
impl ChannelAdapter for TestChannel {
    async fn authenticated_source_epochs(
        &self,
    ) -> anyhow::Result<alia_server::modules::memory::source::SourceEpochs> {
        capture_source_epochs(&self.db, self.owner).await
    }

    async fn authenticated_owner(&self) -> anyhow::Result<Uuid> {
        let row = self
            .db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT status FROM users WHERE id=$1",
                vec![self.owner.into()],
            ))
            .await?
            .ok_or_else(|| anyhow::anyhow!("owner unavailable"))?;
        anyhow::ensure!(
            row.try_get::<String>("", "status")? == "active",
            "owner inactive"
        );
        Ok(self.owner)
    }

    fn channel_type(&self) -> &'static str {
        "embedded"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities::default()
    }

    async fn fence_voice(
        &self,
        scope: VoiceScope,
        close_session: bool,
        _: &str,
    ) -> anyhow::Result<()> {
        assert!(scope.is_valid());
        self.fences.lock().unwrap().push((scope, close_session));
        Ok(()) // Injected output; no device playback acceptance is claimed.
    }

    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
        anyhow::ensure!(
            action.name == "verify_audio_ingress_auth",
            "unexpected action"
        );
        anyhow::ensure!(
            action.params["token"] == TEST_TOKEN,
            "invalid test credential"
        );
        Ok(serde_json::json!({"ok": true}))
    }
}

async fn event(stream: &mut Streaming<SessionEvent>) -> SessionEvent {
    tokio::time::timeout(Duration::from_secs(10), stream.message())
        .await
        .expect("voice evidence deadline")
        .expect("gRPC voice stream failed")
        .expect("voice stream ended before evidence")
}

async fn utterance(
    tx: &mpsc::Sender<DeviceFrame>,
    device: &TestChannel,
    bytes: &[u8],
    sequence: &mut u64,
    sample_offset: &mut u64,
) {
    assert!(!bytes.is_empty() && bytes.len() % 2 == 0);
    // Explicit VAD input: the official clip is marked speech, then 700 ms of
    // silence triggers server evaluation. No microphone/VAD behavior is faked.
    let silence = [0; 3200];
    for (pcm, speech_active) in bytes
        .chunks(3200)
        .map(|pcm| (pcm, true))
        .chain(std::iter::repeat_n((silence.as_slice(), false), 7))
    {
        device.capture(*sequence, speech_active);
        tx.send(DeviceFrame {
            payload: Some(device_frame::Payload::Audio(CaptureFrame {
                sequence: *sequence,
                sample_offset: *sample_offset,
                pcm_s16le: pcm.to_vec(),
                speech_active,
            })),
        })
        .await
        .unwrap();
        *sequence += 1;
        *sample_offset += pcm.len() as u64 / 2;
    }
}

#[tokio::test]
#[ignore = "requires real Easy-Turn service, ALIA_EASY_TURN_PCM_DIR and disposable ALIA_TEST_DATABASE_URL"]
async fn official_audio_over_grpc_preserves_original_response_across_backchannel_and_wait() {
    with_db(|fixture| async move { official_audio(fixture).await }).await;
}

async fn official_audio(fixture: Fixture) {
    let pcm_dir =
        PathBuf::from(std::env::var("ALIA_EASY_TURN_PCM_DIR").expect("set fixture directory"));
    assert!(pcm_dir.is_absolute(), "fixture directory must be absolute");
    let endpoint =
        std::env::var("ALIA_EASY_TURN_URL").unwrap_or_else(|_| "http://127.0.0.1:8091".into());
    let capabilities: serde_json::Value = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .get(format!(
            "{}/v1/capabilities",
            endpoint.trim_end_matches('/')
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(capabilities["model"]["source"], "ASLP-lab/Easy-Turn");
    assert_eq!(capabilities["model"]["revision"], MODEL_REVISION);
    assert_eq!(capabilities["model"]["device"], "cpu");
    assert_eq!(capabilities["max_concurrent_requests"], 1);

    let detector = Arc::new(
        EasyTurnDetector::new(&EasyTurnConfig {
            enabled: true,
            endpoint,
            timeout_ms: 8_000,
            max_concurrency: 1,
        })
        .unwrap(),
    );
    let channels = Arc::new(ChannelRegistry::new());
    let fences = Arc::new(Mutex::new(Vec::new()));
    let device = Arc::new(TestChannel {
        fences: fences.clone(),
        owner: fixture.user,
        db: fixture.db.clone(),
        output: Mutex::new(LocalOutput::default()),
    });
    channels.register(device.clone()).await.unwrap();
    let auth = VoiceIngressAuth::new(channels.clone());
    let (input, mut recognized) = voice_input::channel();
    let shutdown = CancellationToken::new();
    let _cancel_on_drop = shutdown.clone().drop_guard();
    let delivery = DeliveryService::new(fixture.repo(), &shutdown);
    delivery.start().await.unwrap();
    let service = VoiceSessionService::new(
        auth,
        Some(detector),
        input,
        delivery.clone(),
        Duration::from_secs(60),
        shutdown.clone(),
        std::sync::Arc::new(alia_server::modules::platform::lifecycle::RuntimeTasks::default()),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let grpc_endpoint = format!("http://{}", listener.local_addr().unwrap());
    let incoming = async_stream::stream! {
        loop {
            yield listener.accept().await.map(|(stream, _)| stream);
        }
    };
    let server_shutdown = shutdown.clone();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(VoiceSessionServer::new(service).max_decoding_message_size(64 * 1024))
            .serve_with_incoming_shutdown(incoming, server_shutdown.cancelled_owned())
            .await
    });
    let mut client = VoiceSessionClient::connect(grpc_endpoint).await.unwrap();
    let boot = Uuid::new_v4();
    let open = DeviceFrame {
        payload: Some(device_frame::Payload::Open(OpenSession {
            protocol_version: VOICE_CONTRACT_VERSION,
            software_hold: true,
            max_hold_ms: NEGOTIATED_HOLD_MS,
            initiative_admission: false,
            initiative_capture_gate_version: 0,
            device_boot_id: boot.to_string(),
            channel_id: CHANNEL.into(),
        })),
    };
    let missing_auth = client
        .stream(tokio_stream::iter([open.clone()]))
        .await
        .unwrap_err();
    assert_eq!(missing_auth.code(), tonic::Code::Unauthenticated);

    let mut old_open = open.clone();
    if let Some(device_frame::Payload::Open(open)) = old_open.payload.as_mut() {
        open.protocol_version = 1;
    }
    let mut old_request = Request::new(tokio_stream::iter([old_open]));
    old_request.metadata_mut().insert(
        "authorization",
        format!("Bearer {TEST_TOKEN}").parse().unwrap(),
    );
    old_request
        .metadata_mut()
        .insert("x-alia-channel-id", CHANNEL.parse().unwrap());
    assert_eq!(
        client.stream(old_request).await.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );

    let (tx, rx) = mpsc::channel(8);
    tx.send(open).await.unwrap();
    let mut request = Request::new(ReceiverStream::new(rx));
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {TEST_TOKEN}").parse().unwrap(),
    );
    request
        .metadata_mut()
        .insert("x-alia-channel-id", CHANNEL.parse().unwrap());
    let mut responses = client.stream(request).await.unwrap().into_inner();
    let ready = event(&mut responses).await;
    let Some(session_event::Payload::Ready(capabilities)) = &ready.payload else {
        panic!("expected v2 Ready");
    };
    assert_eq!(capabilities.protocol_version, VOICE_CONTRACT_VERSION);
    assert!(capabilities.software_hold);
    assert_eq!(capabilities.max_hold_ms, NEGOTIATED_HOLD_MS);
    assert_eq!(ready.device_boot_id, boot.to_string());
    assert_eq!(ready.generation_epoch, 1);
    let session_id = Uuid::parse_str(&ready.session_id).unwrap();
    let mut sequence = 0;
    let mut sample_offset = 0;
    let mut active_input: Option<voice_input::RecognizedVoiceInput> = None;
    let mut response_id = None;
    let mut original_registration = None;

    for (name, expected, pcm_sha256) in [
        (
            "complete_real_1",
            TurnState::Complete,
            "99d723b782fa39adf1e2a80e1e8ae6e8909cf75dd27007ee3d4cafcdaf32570d",
        ),
        (
            "backchannel_real_1",
            TurnState::Backchannel,
            "6511d724d7618a84fe044c3bbc931ba240ba557e11bf138f273d31f3da4477d8",
        ),
        (
            "wait_real_1",
            TurnState::Wait,
            "cf2152f26f0e0a0768b368ac924a071516de37de273b70c133eed206880bcdb7",
        ),
    ] {
        let pcm = std::fs::read(pcm_dir.join(format!("{name}.pcm"))).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&pcm)),
            pcm_sha256,
            "{name}: wrong official PCM fixture"
        );
        utterance(&tx, &device, &pcm, &mut sequence, &mut sample_offset).await;
        let result = event(&mut responses).await;
        assert_eq!(result.session_id, session_id.to_string());
        assert_eq!(result.device_boot_id, boot.to_string());
        assert_eq!(result.generation_epoch, 2);
        let Some(session_event::Payload::Evidence(evidence)) = result.payload else {
            panic!("expected turn evidence");
        };
        assert_eq!(evidence.state, expected as i32, "{name}");
        assert_eq!(evidence.through_sequence, sequence - 1);
        assert!(!evidence.transcript.trim().is_empty());
        assert!(!Uuid::parse_str(&evidence.request_id).unwrap().is_nil());
        assert!(evidence.inference_ms.is_finite() && evidence.inference_ms > 0.0);
        assert_eq!(evidence.response_enqueued, expected == TurnState::Complete);
        assert_eq!(
            evidence.output_decision,
            match expected {
                TurnState::Complete => OutputDecision::Unchanged,
                TurnState::Backchannel => OutputDecision::ResumeRequested,
                TurnState::Wait => OutputDecision::PauseRequested,
                _ => unreachable!(),
            } as i32
        );
        eprintln!(
            "real gRPC -> Easy-Turn {name}: {expected:?}, {} ms",
            evidence.inference_ms
        );
        if expected == TurnState::Complete {
            let input = recognized.recv().await.unwrap();
            assert_eq!(input.session_id, session_id);
            assert_eq!(input.device_boot_id, boot);
            assert_eq!(input.generation_epoch, 2);
            assert_eq!(input.channel_id, CHANNEL);
            assert_eq!(input.user_id, fixture.user.to_string());
            assert_eq!(input.transcript, evidence.transcript.trim());
            let response = canonical_response(&fixture, &delivery, &input).await;
            let producer = &input.output.as_ref().unwrap().producer;
            producer.bind(response.clone()).await.unwrap();
            assert_eq!(producer.request_id(), input.request_id);
            assert_eq!(producer.output().response_id(), Some(response.response_id));
            let registration = device
                .output
                .lock()
                .unwrap()
                .registered
                .as_ref()
                .unwrap()
                .registration
                .clone();
            assert_eq!(Some(registration.binding.context_id), response.context_id);
            assert_eq!(registration.binding.response_id, response.response_id);
            original_registration = Some(registration);
            response_id = Some(response.response_id);
            active_input = Some(input);
        } else {
            assert!(recognized.try_recv().is_err());
            let original = active_input.as_ref().unwrap();
            assert!(!original.cancel.is_cancelled());
            let producer = &original.output.as_ref().unwrap().producer;
            assert_eq!(producer.output().response_id(), response_id);
            if expected == TurnState::Backchannel {
                tokio::time::timeout(Duration::from_secs(3), producer.wait_open())
                    .await
                    .unwrap()
                    .unwrap();
                assert!(!producer.output().is_held());
            } else {
                assert_eq!(
                    producer.output().hold_reason(),
                    Some(HoldReason::ExplicitWait)
                );
            }
            let registration = original_registration.as_ref().unwrap();
            let view = settled_controls(
                &fixture,
                registration,
                if expected == TurnState::Backchannel {
                    2
                } else {
                    4
                },
            )
            .await;
            let latest = view.scope.latest_receipt.as_ref().unwrap();
            assert_eq!(
                latest.receipt.snapshot.phase,
                if expected == TurnState::Backchannel {
                    ControlPhase::Running
                } else {
                    ControlPhase::Held
                }
            );
            if expected == TurnState::Backchannel {
                assert!(matches!(
                    view.commands[1].command.command.action,
                    ControlAction::Resume { .. }
                ));
            } else {
                assert_eq!(
                    latest.receipt.snapshot.reason,
                    Some(HoldReason::ExplicitWait)
                );
                assert!(matches!(
                    view.commands[3].command.command.action,
                    ControlAction::ConfirmExplicitWait { .. }
                ));
                assert_eq!(
                    view.commands[2].deadline_at, view.commands[3].deadline_at,
                    "WAIT must not renew the acoustic hold budget"
                );
            }
        }
    }
    assert_eq!(fixture.count("delivery_responses").await, 1);
    assert_eq!(
        fixture.assistant_count().await,
        0,
        "no assistant commit or TTS was exercised"
    );
    tx.send(DeviceFrame {
        payload: Some(device_frame::Payload::Close(CloseSession {})),
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), responses.message())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert_eq!(
        *fences.lock().unwrap(),
        [(2, false), (1, true)].map(|(generation_epoch, closed)| {
            (
                VoiceScope {
                    device_boot_id: boot,
                    session_id,
                    generation_epoch,
                },
                closed,
            )
        })
    );
    drop(responses);
    drop(tx);
    drop(client);
    shutdown.cancel();
    assert!(active_input.as_ref().unwrap().cancel.is_cancelled());
    delivery.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

async fn settled_controls(
    fixture: &Fixture,
    registration: &OutputRegistration,
    count: usize,
) -> OutputControlView {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let view = fixture
                .repo()
                .query_control(fixture.user, &registration.binding)
                .await
                .unwrap();
            assert_eq!(
                &view.scope.registration, registration,
                "original output identity or backend changed"
            );
            if view.commands.len() == count
                && view
                    .commands
                    .iter()
                    .all(|record| record.status == ControlCommandStatus::Receipted)
            {
                for record in &view.commands {
                    assert_eq!(record.command.binding, registration.binding);
                    assert_eq!(record.command.expected_backend, registration.backend);
                    assert_eq!(record.intent_digest, record.command.intent_digest());
                    let receipt = record.latest_receipt.as_ref().unwrap();
                    receipt.validate(&record.command).unwrap();
                    assert_eq!(receipt.receipt.outcome, CommandOutcome::Applied);
                    if let ControlAction::Pause { budget_ms, .. } = record.command.command.action {
                        assert!(
                            (1..=NEGOTIATED_HOLD_MS).contains(&budget_ms),
                            "negotiated limit must cap the configured 60000ms hold"
                        );
                    }
                }
                return view;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("original controls were not receipted in the real ledger")
}

async fn canonical_response(
    fixture: &Fixture,
    delivery: &Arc<DeliveryService>,
    input: &voice_input::RecognizedVoiceInput,
) -> alia_server::modules::delivery::types::ResponseRecord {
    let session = Uuid::new_v4();
    let event = Uuid::new_v4();
    fixture
        .db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,$2,$3)",
            vec![
                session.into(),
                CHANNEL.into(),
                fixture.user.to_string().into(),
            ],
        ))
        .await
        .unwrap();
    let source_time = fixture.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,$5,$6,$7) RETURNING source_received_at",
        vec![event.into(), session.into(), fixture.user.to_string().into(), input.transcript.clone().into(), input.source_received_at.into(), input.source_privacy_epoch.into(), input.source_relationship_epoch.into()]))
        .await.unwrap().unwrap().try_get("", "source_received_at").unwrap();
    let source = ContextSource {
        owner_id: fixture.user,
        request_id: input.request_id,
        channel_id: CHANNEL.into(),
        conversation_id: Some(session),
        source_event_id: Some(event),
        source_received_at: source_time,
        source_privacy_epoch: input.source_privacy_epoch,
    };
    let context = fixture
        .context()
        .materialize(source, &input.transcript)
        .await
        .unwrap();
    delivery.prepare(NewResponse {
        key: ResponseKey { user_id: fixture.user, request_id: input.request_id, output_key: "answer".into() },
        context_id: context.lease().id(), conversation_id: Some(session), source_event_id: Some(event),
        source_received_at: source_time, source_privacy_epoch: Some(input.source_privacy_epoch),
        channel_id: CHANNEL.into(), kind: "conversation".into(),
        content: serde_json::json!({"intent":input.transcript,"voice_scope":input.output.as_ref().unwrap().producer.scope()}),
    }).await.unwrap()
}
