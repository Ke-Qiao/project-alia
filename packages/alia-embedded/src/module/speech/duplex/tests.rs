use super::*;
use crate::module::context::AudioBus;
use crate::module::lifecycle::ModuleLifecycle;
use crate::module::traits::WakeWordEvent;
use crate::storage::EmbeddedStore;
use alia_proto::voice::{
    SessionReady, TurnEvidence,
    voice_session_server::{VoiceSession, VoiceSessionServer},
};
use alia_transport::{
    DEFAULT_AUTH_MAX_SKEW, EMBEDDED_INGRESS_SCOPE, embedded_device_auth_subject, verify_auth_token,
};
use std::pin::Pin;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

#[derive(Default)]
struct LocalAudio {
    stops: AtomicU64,
}

#[tonic::async_trait]
impl ModuleLifecycle for LocalAudio {
    fn name(&self) -> &'static str {
        "test_audio"
    }
}
impl AudioPlaybackBackend for LocalAudio {
    fn play(&self, _: &[i16], _: u32) -> Result<()> {
        Ok(())
    }
    fn stop_playback(&self) -> Result<()> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn is_available(&self) -> bool {
        true
    }
}
impl VoiceActivityEngine for LocalAudio {
    fn reset(&self) -> Result<()> {
        Ok(())
    }
    fn accept(&self, pcm: &[i16], _: u32) -> Result<VoiceActivityState> {
        Ok(if pcm.iter().any(|sample| *sample != 0) {
            VoiceActivityState::Speech
        } else {
            VoiceActivityState::Silence
        })
    }
}
impl WakeWordEngine for LocalAudio {
    fn detect(&self, pcm: &[i16]) -> Option<WakeWordEvent> {
        (pcm.first() == Some(&99)).then(|| WakeWordEvent {
            keyword: "wake".into(),
            confidence: 1.0,
            timestamp_us: 0,
        })
    }
}

fn ready(boot: &str, session: &str) -> SessionEvent {
    SessionEvent {
        device_boot_id: boot.into(),
        session_id: session.into(),
        generation_epoch: 1,
        payload: Some(session_event::Payload::Ready(SessionReady {
            protocol_version: 1,
            sample_rate_hz: SAMPLE_RATE,
            channels: 1,
            max_frame_samples: MAX_FRAME_SAMPLES as u32,
        })),
    }
}

fn evidence(boot: &str, session: &str, sequence: u64, state: TurnState) -> SessionEvent {
    SessionEvent {
        device_boot_id: boot.into(),
        session_id: session.into(),
        generation_epoch: 1,
        payload: Some(session_event::Payload::Evidence(TurnEvidence {
            request_id: Uuid::new_v4().to_string(),
            through_sequence: sequence,
            transcript: "test".into(),
            state: state as i32,
            response_enqueued: state == TurnState::Complete,
            inference_ms: 1.0,
        })),
    }
}

enum Seen {
    Opened {
        boot: String,
        session: String,
        output: mpsc::Sender<Result<SessionEvent, Status>>,
    },
    Audio(CaptureFrame),
}

struct FakeServer {
    seen: mpsc::Sender<Seen>,
    public_key: Vec<u8>,
    subject: String,
}

#[tonic::async_trait]
impl VoiceSession for FakeServer {
    type StreamStream = Pin<Box<dyn Stream<Item = Result<SessionEvent, Status>> + Send>>;
    async fn stream(
        &self,
        request: Request<tonic::Streaming<DeviceFrame>>,
    ) -> Result<Response<Self::StreamStream>, Status> {
        let token = request
            .metadata()
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .strip_prefix("Bearer ")
            .unwrap();
        verify_auth_token(
            token,
            &self.public_key,
            &self.subject,
            EMBEDDED_INGRESS_SCOPE,
            DEFAULT_AUTH_MAX_SKEW,
        )
        .unwrap();
        assert_eq!(
            request.metadata().get("x-alia-channel-id").unwrap(),
            CHANNEL_ID
        );
        let mut incoming = request.into_inner();
        let open = incoming.message().await?.unwrap();
        let Some(device_frame::Payload::Open(open)) = open.payload else {
            panic!("expected Open")
        };
        assert_eq!(open.protocol_version, 1);
        assert_eq!(open.channel_id, CHANNEL_ID);
        let session = Uuid::new_v4().to_string();
        let boot = open.device_boot_id;
        let (output, mut replies) = mpsc::channel(8);
        self.seen
            .send(Seen::Opened {
                boot: boot.clone(),
                session: session.clone(),
                output,
            })
            .await
            .unwrap();
        let seen = self.seen.clone();
        let response = async_stream::stream! {
            yield Ok(ready(&boot, &session));
            let mut sequence = 0;
            let mut offset = 0;
            loop {
                tokio::select! {
                    frame = incoming.message() => match frame {
                        Ok(Some(DeviceFrame { payload: Some(device_frame::Payload::Audio(frame)) })) => {
                            assert_eq!(frame.sequence, sequence);
                            assert_eq!(frame.sample_offset, offset);
                            assert!(!frame.pcm_s16le.is_empty() && frame.pcm_s16le.len() <= 3200);
                            sequence += 1;
                            offset += frame.pcm_s16le.len() as u64 / 2;
                            seen.send(Seen::Audio(frame)).await.unwrap();
                        }
                        _ => break,
                    },
                    reply = replies.recv() => match reply {
                        Some(reply) => { let done = reply.is_err(); yield reply; if done { break; } },
                        None => break,
                    },
                }
            }
        };
        Ok(Response::new(Box::pin(response)))
    }
}

struct Harness {
    runtime: Runtime,
    audio: AudioBus,
    events: EventBus,
    local: Arc<LocalAudio>,
    seen: mpsc::Receiver<Seen>,
    shutdown: CancellationToken,
    server: JoinHandle<()>,
    _directory: tempfile::TempDir,
}

async fn harness(audio_capacity: usize) -> Harness {
    let directory = tempfile::tempdir().unwrap();
    let keypair_path = directory.path().join("device.key");
    let keypair = crate::identity::ensure_device_keypair(&keypair_path).unwrap();
    let store = EmbeddedStore::open(directory.path().join("state.sqlite"))
        .await
        .unwrap();
    let binding = Arc::new(BindingManager::new(store, 3, 60));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    binding
        .seed_dev_binding("test", "test", &format!("http://127.0.0.1:{}", port - 1))
        .await
        .unwrap();
    let (seen_tx, seen) = mpsc::channel(32);
    let public_key = keypair.public_key_bytes().to_vec();
    let subject = embedded_device_auth_subject("test-device", &public_key);
    let shutdown = CancellationToken::new();
    let stop_server = shutdown.clone();
    let server = tokio::spawn(async move {
        let incoming = async_stream::stream! { loop { yield listener.accept().await.map(|(stream, _)| stream); } };
        tonic::transport::Server::builder()
            .add_service(VoiceSessionServer::new(FakeServer {
                seen: seen_tx,
                public_key,
                subject,
            }))
            .serve_with_incoming_shutdown(incoming, stop_server.cancelled())
            .await
            .unwrap();
    });
    let audio = AudioBus::new(audio_capacity);
    let events = EventBus::new(32);
    let local = Arc::new(LocalAudio::default());
    let runtime = Runtime {
        audio: audio.subscribe(),
        events: events.subscribe(),
        event_bus: events.clone(),
        wakeword: local.clone(),
        vad: local.clone(),
        playback: local.clone(),
        binding,
        device_id: "test-device".into(),
        keypair_path,
        boot_id: Uuid::new_v4().to_string(),
        config: SpeechConfig {
            mode: crate::config::SpeechMode::ServerTurn,
            pre_roll_ms: 20,
            upload_queue_frames: 8,
        },
        cancel: CancellationToken::new(),
    };
    Harness {
        runtime,
        audio,
        events,
        local,
        seen,
        shutdown,
        server,
        _directory: directory,
    }
}

async fn received(rx: &mut mpsc::Receiver<Seen>) -> Seen {
    tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

fn publish(bus: &AudioBus, value: i16, offset: u64) {
    bus.publish(AudioFrame {
        pcm: vec![value; 320],
        sample_rate: SAMPLE_RATE,
        timestamp_us: offset * 1_000_000 / SAMPLE_RATE as u64,
    });
}

#[tokio::test]
async fn captures_during_playback_receives_all_turn_states_and_reconnects() {
    let Harness {
        runtime,
        audio,
        events,
        local,
        mut seen,
        shutdown,
        server,
        _directory,
    } = harness(16).await;
    let cancel = runtime.cancel.clone();
    let mut lifecycle = events.subscribe();
    // A finite pre-roll followed by wake. Both frames are delivered before any utterance ends.
    publish(&audio, 0, 0);
    publish(&audio, 99, 320);
    let task = tokio::spawn(runtime.run());
    let Seen::Opened {
        boot,
        session,
        output,
    } = received(&mut seen).await
    else {
        panic!("expected Open")
    };
    for expected in [false, true] {
        let Seen::Audio(frame) = received(&mut seen).await else {
            panic!("expected audio")
        };
        assert_eq!(frame.speech_active, expected);
    }
    events.publish(ModuleEvent::PlaybackStarted {
        source: "tts".into(),
    });
    for (index, state) in [
        TurnState::Incomplete,
        TurnState::Backchannel,
        TurnState::Complete,
        TurnState::Wait,
    ]
    .into_iter()
    .enumerate()
    {
        publish(&audio, 7, 640 + index as u64 * 320);
        let Seen::Audio(frame) = received(&mut seen).await else {
            panic!("playback must not stop capture")
        };
        output
            .send(Ok(evidence(&boot, &session, frame.sequence, state)))
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while local.stops.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        local.stops.load(Ordering::SeqCst),
        1,
        "only Wait stops playback"
    );
    // A failed stream ends its capture session, then a fresh wake reconnects.
    output
        .send(Err(Status::unavailable("test disconnect")))
        .await
        .unwrap();
    loop {
        if matches!(
            tokio::time::timeout(Duration::from_secs(3), lifecycle.recv())
                .await
                .unwrap()
                .unwrap(),
            ModuleEvent::SpeechEnd
        ) {
            break;
        }
    }
    publish(&audio, 99, 1920);
    let Seen::Opened {
        boot: next_boot,
        session: next_session,
        output: next_output,
    } = received(&mut seen).await
    else {
        panic!("expected reconnect")
    };
    assert_eq!(next_boot, boot);
    assert_ne!(next_session, session);
    let Seen::Audio(frame) = received(&mut seen).await else {
        panic!("expected new capture")
    };
    assert_eq!((frame.sequence, frame.sample_offset), (0, 0));
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), next_output.closed())
        .await
        .unwrap();
    assert!(local.stops.load(Ordering::SeqCst) >= 3);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn audio_bus_lag_ends_active_stream_instead_of_hiding_a_gap() {
    let Harness {
        runtime,
        audio,
        events,
        local: _,
        mut seen,
        shutdown,
        server,
        _directory,
    } = harness(1).await;
    let cancel = runtime.cancel.clone();
    let mut lifecycle = events.subscribe();
    let task = tokio::spawn(runtime.run());
    publish(&audio, 99, 0);
    let Seen::Opened { output, .. } = received(&mut seen).await else {
        panic!("expected Open")
    };
    assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
    publish(&audio, 1, 320);
    publish(&audio, 1, 640);
    publish(&audio, 1, 960);
    loop {
        if matches!(
            tokio::time::timeout(Duration::from_secs(3), lifecycle.recv())
                .await
                .unwrap()
                .unwrap(),
            ModuleEvent::SpeechEnd
        ) {
            break;
        }
    }
    tokio::time::timeout(Duration::from_secs(3), output.closed())
        .await
        .unwrap();
    cancel.cancel();
    task.await.unwrap();
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn full_upload_queue_cancels_and_joins_without_waiting_for_network() {
    let (tx, _rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let waiting = cancel.clone();
    let task = tokio::spawn(async move {
        waiting.cancelled().await;
        Ok(())
    });
    let mut session = Session {
        tx,
        cancel,
        task,
        sequence: 0,
        sample_offset: 0,
    };
    session
        .enqueue(Captured {
            pcm: vec![1; 320],
            speech_active: true,
        })
        .unwrap();
    assert!(
        session
            .enqueue(Captured {
                pcm: vec![2; 320],
                speech_active: true
            })
            .is_err()
    );
    assert_eq!((session.sequence, session.sample_offset), (1, 320));
    tokio::time::timeout(
        Duration::from_secs(1),
        end_session(&mut Some(session), &EventBus::new(4)),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn unbinding_stops_an_idle_network_session_and_its_playback() {
    let Harness {
        runtime,
        audio,
        events: _,
        local,
        mut seen,
        shutdown,
        server,
        _directory,
    } = harness(8).await;
    let binding = runtime.binding.clone();
    let cancel = runtime.cancel.clone();
    let task = tokio::spawn(runtime.run());
    publish(&audio, 99, 0);
    let Seen::Opened { output, .. } = received(&mut seen).await else {
        panic!("expected Open")
    };
    assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
    binding.unbind("test").await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), output.closed())
        .await
        .unwrap();
    assert_eq!(local.stops.load(Ordering::SeqCst), 1);
    cancel.cancel();
    task.await.unwrap();
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn rejects_format_changes_clock_gaps_and_untrusted_server_identity() {
    let mut clock = CaptureClock::default();
    let mut frame = AudioFrame {
        pcm: vec![0; 320],
        sample_rate: SAMPLE_RATE,
        timestamp_us: 0,
    };
    clock.accept(&frame).unwrap();
    frame.timestamp_us = 21_000;
    assert!(clock.accept(&frame).is_err());
    frame.timestamp_us = 20_000;
    frame.sample_rate = 48000;
    assert!(clock.accept(&frame).is_err());

    let boot = Uuid::new_v4().to_string();
    let session = Uuid::new_v4().to_string();
    let local = LocalAudio::default();
    for invalid in [
        ready(&Uuid::new_v4().to_string(), &session),
        ready(&boot, &Uuid::nil().to_string()),
        evidence(&boot, &session, 0, TurnState::Complete),
    ] {
        assert!(
            SessionGuard::new(boot.clone())
                .accept(invalid, 1, &local)
                .is_err()
        );
    }
    let mut guard = SessionGuard::new(boot.clone());
    guard.accept(ready(&boot, &session), 0, &local).unwrap();
    assert!(
        guard
            .accept(evidence(&boot, &session, 1, TurnState::Complete), 1, &local)
            .is_err()
    );
    let mut regressed = evidence(&boot, &session, 0, TurnState::Wait);
    regressed.generation_epoch = 0;
    assert!(guard.accept(regressed, 1, &local).is_err());
    assert_eq!(local.stops.load(Ordering::SeqCst), 0);
}

#[test]
fn bounded_pre_roll_and_server_turn_config_survive_runtime_roundtrip() {
    let mut pre_roll = PreRoll::new(400);
    pre_roll.push(Captured {
        pcm: vec![1; 320],
        speech_active: false,
    });
    pre_roll.push(Captured {
        pcm: vec![2; 320],
        speech_active: true,
    });
    let samples: Vec<i16> = pre_roll.drain().flat_map(|frame| frame.pcm).collect();
    assert_eq!(samples, [vec![1; 80], vec![2; 320]].concat());
    let runtime: crate::config::EmbeddedRuntimeSettings = toml::from_str(
        "[speech]\nmode = 'server_turn'\npre_roll_ms = 300\nupload_queue_frames = 8",
    )
    .unwrap();
    let mut config = crate::config::EmbeddedConfig::default();
    assert_eq!(config.speech.mode, crate::config::SpeechMode::Utterance);
    config.apply_runtime_settings(runtime);
    config.speech.validate(&config.audio).unwrap();
    let saved: crate::config::EmbeddedRuntimeSettings =
        toml::from_str(&toml::to_string(&config.runtime_settings()).unwrap()).unwrap();
    assert_eq!(saved.speech.mode, crate::config::SpeechMode::ServerTurn);
    assert_eq!(saved.speech.pre_roll_ms, 300);
    config.audio.capture_channels = 2;
    assert!(config.speech.validate(&config.audio).is_err());
}
