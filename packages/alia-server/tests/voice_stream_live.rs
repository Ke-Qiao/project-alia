//! Opt-in gRPC -> production Rust detector -> real pinned Easy-Turn smoke.
//! Only channel authentication/output are injected. This does not validate
//! production device binding, a physical microphone, VAD, LLM/TTS, or playback.
//!
//! ALIA_EASY_TURN_PCM_DIR must be an absolute directory containing PCM16 mono
//! 16 kHz conversions of the pinned official complete_real_1, backchannel_real_1,
//! and wait_real_1 WAV samples. Conversion: alia_easy_turn.smoke.pcm16.
#![cfg(all(feature = "channel-embedded", feature = "turn-easy"))]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alia_core::delivery::VoiceScope;
use alia_proto::voice::voice_session_client::VoiceSessionClient;
use alia_proto::voice::voice_session_server::VoiceSessionServer;
use alia_proto::voice::{
    CaptureFrame, CloseSession, DeviceFrame, OpenSession, SessionEvent, TurnState, device_frame,
    session_event,
};
use alia_server::adapters::easy_turn::{EasyTurnConfig, EasyTurnDetector};
use alia_server::channels::ChannelRegistry;
use alia_server::channels::adapter::{
    ChannelAction, ChannelAdapter, ChannelCapabilities, ChannelTarget,
};
use alia_server::event::EventBus;
use alia_server::grpc::audio_ingress::ServerAudioIngressService;
use alia_server::grpc::voice_session::VoiceSessionService;
use alia_server::module::lifecycle::ModuleLifecycle;
use alia_server::modules::interaction::voice_input;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Streaming};
use uuid::Uuid;

const CHANNEL: &str = "official-audio-test";
const TEST_TOKEN: &str = "injected-test-credential";
const MODEL_REVISION: &str = "7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98";

struct TestChannel {
    fences: Arc<Mutex<Vec<(VoiceScope, bool)>>>,
}

#[async_trait]
impl ModuleLifecycle for TestChannel {
    fn name(&self) -> &'static str {
        "injected-live-test-channel"
    }
}

#[async_trait]
impl ChannelAdapter for TestChannel {
    async fn authenticated_source_epoch(&self) -> anyhow::Result<i64> {
        Ok(0) // Explicit test authority; this smoke does not exercise a database.
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
        CHANNEL
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities::default()
    }

    async fn send_message(&self, _: &ChannelTarget, _: &str) -> anyhow::Result<()> {
        anyhow::bail!("live smoke must stop at recognized input")
    }

    async fn stop_current_playback(&self, _: &ChannelTarget, _: &str) -> anyhow::Result<()> {
        panic!("live voice session must never use an unscoped stop")
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
#[ignore = "requires real Easy-Turn weights/service and absolute ALIA_EASY_TURN_PCM_DIR"]
async fn official_audio_over_grpc_preserves_backchannel_and_cancels_on_wait() {
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
    channels
        .register(Arc::new(TestChannel {
            fences: fences.clone(),
        }))
        .await
        .unwrap();
    let auth = ServerAudioIngressService::new(EventBus::new(8), channels.clone(), CHANNEL);
    let (input, mut recognized) = voice_input::channel();
    let shutdown = CancellationToken::new();
    let _cancel_on_drop = shutdown.clone().drop_guard();
    let service = VoiceSessionService::new(
        auth,
        channels,
        Some(detector),
        input,
        shutdown.clone(),
        std::sync::Arc::new(alia_server::module::lifecycle::RuntimeTasks::default()),
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
            protocol_version: 1,
            device_boot_id: boot.to_string(),
            channel_id: CHANNEL.into(),
        })),
    };
    let missing_auth = client
        .stream(tokio_stream::iter([open.clone()]))
        .await
        .unwrap_err();
    assert_eq!(missing_auth.code(), tonic::Code::Unauthenticated);

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
    assert!(matches!(
        ready.payload,
        Some(session_event::Payload::Ready(_))
    ));
    assert_eq!(ready.device_boot_id, boot.to_string());
    assert_eq!(ready.generation_epoch, 1);
    let session_id = Uuid::parse_str(&ready.session_id).unwrap();
    let mut sequence = 0;
    let mut sample_offset = 0;
    let mut active_response = None;

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
        utterance(&tx, &pcm, &mut sequence, &mut sample_offset).await;
        let result = event(&mut responses).await;
        assert_eq!(result.session_id, session_id.to_string());
        assert_eq!(result.device_boot_id, boot.to_string());
        assert_eq!(
            result.generation_epoch,
            if expected == TurnState::Wait { 3 } else { 2 }
        );
        let Some(session_event::Payload::Evidence(evidence)) = result.payload else {
            panic!("expected turn evidence");
        };
        assert_eq!(evidence.state, expected as i32, "{name}");
        assert_eq!(evidence.through_sequence, sequence - 1);
        assert!(!evidence.transcript.trim().is_empty());
        assert!(!Uuid::parse_str(&evidence.request_id).unwrap().is_nil());
        assert!(evidence.inference_ms.is_finite() && evidence.inference_ms > 0.0);
        assert_eq!(evidence.response_enqueued, expected == TurnState::Complete);
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
            assert_eq!(input.user_id, Uuid::from_u128(7).to_string());
            assert_eq!(input.transcript, evidence.transcript.trim());
            active_response = Some(input.cancel);
        } else {
            assert!(recognized.try_recv().is_err());
            assert_eq!(
                active_response.as_ref().unwrap().is_cancelled(),
                expected == TurnState::Wait
            );
        }
    }
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
        [(2, false), (3, false), (1, true)].map(|(generation_epoch, closed)| {
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
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
