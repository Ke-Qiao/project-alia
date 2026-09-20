use super::*;
use crate::module::context::AudioBus;
use crate::module::lifecycle::ModuleLifecycle;
use crate::module::traits::{AudioPlaybackBackend, WakeWordEvent};
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
    generation: AtomicU64,
}

#[tonic::async_trait]
impl ModuleLifecycle for LocalAudio {
    fn name(&self) -> &'static str {
        "test_audio"
    }
}
impl AudioPlaybackBackend for LocalAudio {
    fn open_submission(
        &self,
        identity: alia_core::delivery::PlaybackIdentity,
        rate: u32,
        generation: u64,
    ) -> Result<Arc<crate::module::playback_session::AudioSubmission>> {
        Ok(crate::module::playback_session::AudioSubmission::test(
            identity, generation, rate,
        ))
    }
    fn submit_audio(
        &self,
        submission: &Arc<crate::module::playback_session::AudioSubmission>,
        offset: u64,
        pcm: &[i16],
    ) -> Result<bool> {
        if !self.is_playback_current(submission.generation) {
            return Ok(false);
        }
        submission.accept(offset, pcm.len() as u64)
    }
    fn end_submission(
        &self,
        submission: &Arc<crate::module::playback_session::AudioSubmission>,
        total: u64,
    ) -> Result<bool> {
        submission.accept_end(total)
    }
    fn cancel_submission(
        &self,
        submission: &Arc<crate::module::playback_session::AudioSubmission>,
        reason: &str,
    ) -> Result<bool> {
        if !self.is_playback_current(submission.generation) {
            return Ok(false);
        }
        submission.terminate(alia_core::delivery::ObservationEnd::Cancelled {
            reason: reason.into(),
        });
        Ok(true)
    }
    fn play(&self, _: &[i16], _: u32) -> Result<()> {
        Ok(())
    }
    fn stop_playback(&self) -> Result<()> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn begin_scoped_playback(&self) -> Result<u64> {
        Ok(self.generation.fetch_add(1, Ordering::SeqCst) + 1)
    }
    fn play_if_current(&self, _: &[i16], _: u32, generation: u64) -> Result<bool> {
        Ok(self.is_playback_current(generation))
    }
    fn stop_if_current(&self, generation: u64) -> Result<bool> {
        if self
            .generation
            .compare_exchange(
                generation,
                generation + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn is_playback_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
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
        generation_epoch: match state {
            TurnState::Complete => 2,
            TurnState::Wait => 3,
            _ => 1,
        },
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
        playback_session: Arc::new(PlaybackSession::new(local.clone())),
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
        timing: None,
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
    let playback = runtime.playback_session.clone();
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
        if state == TurnState::Wait {
            playback
                .begin(
                    &VoiceScope {
                        device_boot_id: Uuid::parse_str(&boot).unwrap(),
                        session_id: Uuid::parse_str(&session).unwrap(),
                        generation_epoch: 2,
                    },
                    &Uuid::new_v4().to_string(),
                    1,
                    24000,
                )
                .unwrap();
        }
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
    assert!(
        !playback.is_active(),
        "stopped runtime retained a voice playback session"
    );
    assert_eq!(
        local.stops.load(Ordering::SeqCst),
        1,
        "closing empty sessions must not globally stop unrelated playback"
    );
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
async fn binding_changes_close_idle_session_and_completed_playback_tail() {
    for transition in [
        crate::binding::BindingState::Unbound,
        crate::binding::BindingState::HandoffPending,
        crate::binding::BindingState::Bound,
    ] {
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
        let playback = runtime.playback_session.clone();
        let task = tokio::spawn(runtime.run());
        publish(&audio, 99, 0);
        let Seen::Opened {
            output,
            boot,
            session,
        } = received(&mut seen).await
        else {
            panic!("expected Open")
        };
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
        let scope = VoiceScope {
            device_boot_id: Uuid::parse_str(&boot).unwrap(),
            session_id: Uuid::parse_str(&session).unwrap(),
            generation_epoch: 2,
        };
        let lease = playback
            .begin(&scope, &Uuid::new_v4().to_string(), 1, 24000)
            .unwrap();
        assert!(playback.enqueue(&lease, &[1, 2, 3]).unwrap());
        assert!(playback.end(&lease).unwrap()); // Push End/EOF left a hardware tail, owned by the session.
        match transition {
            crate::binding::BindingState::Unbound => binding.unbind("test").await.unwrap(),
            crate::binding::BindingState::HandoffPending => {
                binding
                    .update_for_test(transition, Some("test".into()))
                    .await
            }
            crate::binding::BindingState::Bound => {
                binding
                    .update_for_test(transition, Some("replacement-server".into()))
                    .await
            }
        }
        tokio::time::timeout(Duration::from_secs(3), output.closed())
            .await
            .unwrap();
        assert!(
            !playback.is_active(),
            "binding change left old session admitted"
        );
        assert_eq!(
            local.stops.load(Ordering::SeqCst),
            1,
            "binding change did not clear playback tail"
        );
        cancel.cancel();
        task.await.unwrap();
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[test]
fn rejects_format_changes_clock_gaps_and_untrusted_server_identity() {
    let mut clock = CaptureClock::default();
    let mut frame = AudioFrame {
        timing: None,
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
    let local = Arc::new(LocalAudio::default());
    let playback = Arc::new(PlaybackSession::new(local.clone()));
    for invalid in [
        ready(&Uuid::new_v4().to_string(), &session),
        ready(&boot, &Uuid::nil().to_string()),
        evidence(&boot, &session, 0, TurnState::Complete),
    ] {
        assert!(
            SessionGuard::new(boot.clone(), playback.clone())
                .accept(invalid, 1)
                .is_err()
        );
    }
    let mut guard = SessionGuard::new(boot.clone(), playback.clone());
    guard.accept(ready(&boot, &session), 0).unwrap();
    assert!(
        guard
            .accept(evidence(&boot, &session, 1, TurnState::Complete), 1)
            .is_err()
    );
    let mut regressed = evidence(&boot, &session, 0, TurnState::Wait);
    regressed.generation_epoch = 0;
    assert!(guard.accept(regressed, 1).is_err());
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

#[tokio::test(start_paused = true)]
async fn outgoing_waits_for_ready_and_refreshes_without_consuming_capture_sequence() {
    use futures::{FutureExt, StreamExt};
    let (audio, rx) = mpsc::channel(2);
    let (ready, ready_rx) = oneshot::channel();
    let (failed, _failure) = oneshot::channel();
    let sent = Arc::new(AtomicU64::new(0));
    let minted = Arc::new(AtomicU64::new(0));
    let counter = minted.clone();
    let stream = capture_stream(
        Uuid::new_v4().to_string(),
        rx,
        ready_rx,
        sent.clone(),
        move || {
            Ok(IngressAuth {
                token: format!("fresh-{}", counter.fetch_add(1, Ordering::SeqCst) + 1),
            })
        },
        failed,
    );
    tokio::pin!(stream);
    assert!(matches!(
        stream.next().await.unwrap().payload,
        Some(device_frame::Payload::Open(_))
    ));
    audio
        .send(CaptureFrame {
            sequence: 0,
            sample_offset: 0,
            pcm_s16le: vec![0; 2],
            speech_active: false,
        })
        .await
        .unwrap();
    assert!(
        stream.next().now_or_never().is_none(),
        "audio escaped before Ready activated playback"
    );
    assert_eq!(sent.load(Ordering::SeqCst), 0);
    ready.send(()).unwrap();
    assert!(matches!(
        stream.next().await.unwrap().payload,
        Some(device_frame::Payload::Audio(_))
    ));
    for revision in 1..=3 {
        tokio::time::advance(Duration::from_secs(120)).await;
        let event = stream.next().await.unwrap();
        let Some(device_frame::Payload::RefreshAuth(auth)) = event.payload else {
            panic!("expected refresh")
        };
        assert_eq!(auth.bearer_token, format!("fresh-{revision}"));
        assert_eq!(
            sent.load(Ordering::SeqCst),
            1,
            "refresh consumed capture sequence"
        );
    }
    assert_eq!(minted.load(Ordering::SeqCst), 3);
    drop(audio);
    assert!(matches!(
        stream.next().await.unwrap().payload,
        Some(device_frame::Payload::Close(_))
    ));
}

#[test]
fn late_old_guard_close_cannot_close_replacement_playback_session() {
    let local = Arc::new(LocalAudio::default());
    let playback = Arc::new(PlaybackSession::new(local.clone()));
    let boot = Uuid::new_v4().to_string();
    let mut old = SessionGuard::new(boot.clone(), playback.clone());
    old.accept(ready(&boot, &Uuid::new_v4().to_string()), 0)
        .unwrap();
    let replacement_id = Uuid::new_v4();
    let mut replacement = SessionGuard::new(boot.clone(), playback.clone());
    replacement
        .accept(ready(&boot, &replacement_id.to_string()), 0)
        .unwrap();
    let scope = VoiceScope {
        device_boot_id: Uuid::parse_str(&boot).unwrap(),
        session_id: replacement_id,
        generation_epoch: 1,
    };
    let lease = playback
        .begin(&scope, &Uuid::new_v4().to_string(), 1, 24000)
        .unwrap();
    drop(old);
    assert!(playback.enqueue(&lease, &[1]).unwrap());
    assert_eq!(local.stops.load(Ordering::SeqCst), 0);
    drop(replacement);
    assert!(!playback.enqueue(&lease, &[1]).unwrap());
    assert!(!playback.is_active());
    assert_eq!(local.stops.load(Ordering::SeqCst), 1);
}
