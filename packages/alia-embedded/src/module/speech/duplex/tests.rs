use super::*;
use crate::module::context::AudioBus;
use crate::module::lifecycle::ModuleLifecycle;
use crate::module::traits::{AudioPlaybackBackend, WakeWordEvent};
use crate::storage::EmbeddedStore;
use alia_core::audio_clock::{AudioClockReading, AudioStreamId, AudioTimeQuality, AudioTiming};
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
    captured: AtomicU64,
    wake_prefix: std::sync::atomic::AtomicBool,
    wake_checks: AtomicU64,
    wake_resets: AtomicU64,
    control: std::sync::Mutex<Option<Arc<crate::module::playback_session::PlaybackControl>>>,
}

#[tonic::async_trait]
impl ModuleLifecycle for LocalAudio {
    fn name(&self) -> &'static str {
        "test_audio"
    }
}
impl AudioPlaybackBackend for LocalAudio {
    fn playback_control(
        &self,
        scope: &alia_core::delivery::VoiceScope,
        generation: u64,
    ) -> Result<Arc<crate::module::playback_session::PlaybackControl>> {
        ensure!(self.is_playback_current(generation), "original owner gone");
        let mut control = self.control.lock().unwrap();
        if let Some(control) = control.as_ref() {
            return Ok(control.clone());
        }
        let created = crate::module::playback_session::PlaybackControl::new(
            *scope,
            generation,
            alia_core::delivery::SubmissionBackend {
                instance_id: Uuid::from_u128(42),
                boundary: alia_core::delivery::SubmissionBoundary::CpalCallbackBuffer,
                output_sample_rate_hz: SAMPLE_RATE,
                output_channels: 1,
            },
        )?;
        *control = Some(created.clone());
        Ok(created)
    }
    fn apply_playback_control(
        &self,
        control: &Arc<crate::module::playback_session::PlaybackControl>,
        command: &alia_core::playback_control::ControlCommand,
    ) -> Result<alia_core::playback_control::ControlReceipt> {
        ensure!(
            self.is_playback_current(control.generation),
            "original owner gone"
        );
        Ok(control.apply(command))
    }
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
    fn begin_scoped_playback(&self) -> Result<u64> {
        if let Some(control) = self.control.lock().unwrap().take() {
            control.retire(alia_core::playback_control::RetiredReason::Cancelled);
        }
        Ok(self.generation.fetch_add(1, Ordering::SeqCst) + 1)
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
            if let Some(control) = self.control.lock().unwrap().take() {
                control.retire(alia_core::playback_control::RetiredReason::Cancelled);
            }
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
        self.captured.fetch_add(1, Ordering::SeqCst);
        Ok(if pcm.iter().any(|sample| *sample != 0) {
            VoiceActivityState::Speech
        } else {
            VoiceActivityState::Silence
        })
    }
}
impl WakeWordEngine for LocalAudio {
    fn reset(&self) -> Result<()> {
        self.wake_prefix.store(false, Ordering::SeqCst);
        self.wake_resets.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn detect(&self, pcm: &[i16]) -> Option<WakeWordEvent> {
        let detected = match pcm.first() {
            Some(97) => {
                self.wake_prefix.store(true, Ordering::SeqCst);
                false
            }
            Some(98) => self.wake_prefix.swap(false, Ordering::SeqCst),
            Some(99) => true,
            _ => false,
        };
        self.wake_checks.fetch_add(1, Ordering::SeqCst);
        detected.then(|| WakeWordEvent {
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
            protocol_version: VOICE_CONTRACT_VERSION,
            software_hold: true,
            max_hold_ms: MAX_HOLD_MS,
            sample_rate_hz: SAMPLE_RATE,
            channels: 1,
            max_frame_samples: MAX_FRAME_SAMPLES as u32,
            initiative_admission: false,
            initiative_capture_gate_version: 0,
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
            output_decision: if state == TurnState::Wait {
                OutputDecision::Cancelled
            } else {
                OutputDecision::Unchanged
            } as i32,
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
    expected_channel_id: String,
    ready_gate: Option<Arc<tokio::sync::Semaphore>>,
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
            request
                .metadata()
                .get("x-alia-channel-id")
                .unwrap()
                .to_str()
                .unwrap(),
            self.expected_channel_id
        );
        let mut incoming = request.into_inner();
        let open = incoming.message().await?.unwrap();
        let Some(device_frame::Payload::Open(open)) = open.payload else {
            panic!("expected Open")
        };
        assert_eq!(open.protocol_version, VOICE_CONTRACT_VERSION);
        assert!(open.software_hold);
        assert_eq!(open.max_hold_ms, MAX_HOLD_MS);
        assert_eq!(open.channel_id, self.expected_channel_id);
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
        let ready_gate = self.ready_gate.clone();
        let response = async_stream::stream! {
            if let Some(gate) = ready_gate { let _permit = gate.acquire().await.unwrap(); }
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
    harness_with_ready_gate(audio_capacity, None).await
}
async fn harness_with_ready_gate(
    audio_capacity: usize,
    ready_gate: Option<Arc<tokio::sync::Semaphore>>,
) -> Harness {
    harness_with_channel_id(
        audio_capacity,
        ready_gate,
        SpeechConfig::default().channel_id,
    )
    .await
}
async fn harness_with_channel_id(
    audio_capacity: usize,
    ready_gate: Option<Arc<tokio::sync::Semaphore>>,
    channel_id: String,
) -> Harness {
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
    let expected_channel_id = channel_id.clone();
    let server = tokio::spawn(async move {
        let incoming = async_stream::stream! { loop { yield listener.accept().await.map(|(stream, _)| stream); } };
        tonic::transport::Server::builder()
            .add_service(VoiceSessionServer::new(FakeServer {
                expected_channel_id,
                ready_gate,
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
        direct_response: Arc::new(crate::module::noop::NoopDirectResponse),
        audio_debug: crate::module::audio_debug::AudioDebugRecorder::new(Default::default()),
        playback_session: Arc::new(PlaybackSession::new(local.clone())),
        binding,
        device_id: "test-device".into(),
        keypair_path,
        boot_id: Uuid::new_v4().to_string(),
        config: SpeechConfig {
            channel_id,
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

fn native_frame(value: i16, offset: u64, stream_id: Uuid, continuity: u64) -> AudioFrame {
    AudioFrame {
        timing: Some(CaptureTiming {
            native: AudioTiming {
                stream: AudioStreamId {
                    boot_id: Uuid::from_u128(1),
                    stream_id,
                    direction: AudioDirection::Capture,
                },
                format: AudioFormat {
                    sample_rate_hz: 48000,
                    channels: 2,
                },
                start_frame: offset * 3,
                frames: 960,
                continuity,
                clock: AudioClockReading {
                    clock_id: stream_id,
                    callback_ns: None,
                    sample_ns: None,
                    queued_frames: None,
                    quality: AudioTimeQuality::Unknown,
                },
                observed_boot_ns: 0,
            },
            converted_format: AudioFormat {
                sample_rate_hz: SAMPLE_RATE,
                channels: 1,
            },
            converted_start_frame: offset,
            converted_frames: 320,
            resampled: true,
        }),
        pcm: vec![value; 320],
        sample_rate: SAMPLE_RATE,
        timestamp_us: offset * 1_000_000 / SAMPLE_RATE as u64,
    }
}

async fn reached(counter: &AtomicU64, count: u64) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while counter.load(Ordering::SeqCst) < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn configured_channel_instance_reaches_open_and_auth_metadata_unchanged() {
    let Harness {
        runtime,
        audio,
        events: _,
        local: _,
        mut seen,
        shutdown,
        server,
        _directory,
    } = harness_with_channel_id(8, None, "pi-living-room".into()).await;
    let cancel = runtime.cancel.clone();
    publish(&audio, 99, 0);
    let task = tokio::spawn(runtime.run());
    // FakeServer verifies both metadata and Open against its concrete instance.
    let Seen::Opened { output, .. } = received(&mut seen).await else {
        panic!("expected authenticated Open on the configured instance");
    };
    assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), output.closed())
        .await
        .unwrap();
    drop(output);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
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
        "explicit Cancelled evidence fences playback"
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
async fn native_xrun_or_restart_retires_active_output_before_fresh_wake() {
    for restart in [false, true] {
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
        let cancel = runtime.cancel.clone();
        let playback = runtime.playback_session.clone();
        let task = tokio::spawn(runtime.run());
        let original = Uuid::new_v4();
        audio.publish(native_frame(99, 0, original, 1));
        let Seen::Opened {
            output,
            boot,
            session,
        } = received(&mut seen).await
        else {
            panic!("expected Open")
        };
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
        let lease = playback
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
        assert!(playback.enqueue(&lease, &[1, 2, 3]).unwrap());
        assert!(playback.end(&lease).unwrap());

        let (next_stream, next_continuity) = if restart {
            (Uuid::new_v4(), 1)
        } else {
            (original, 2)
        };
        // The converted clock remains continuous. Only native evidence exposes
        // the xrun/reopen, before this frame can reach VAD or the old upload.
        audio.publish(native_frame(98, 320, next_stream, next_continuity));
        reached(&local.wake_resets, 2).await;
        tokio::time::timeout(Duration::from_secs(3), output.closed())
            .await
            .unwrap();
        assert!(!playback.is_active());
        assert_eq!(local.stops.load(Ordering::SeqCst), 1);
        assert_eq!(local.captured.load(Ordering::SeqCst), 1);
        assert!(
            seen.try_recv().is_err(),
            "damaged capture reached the server"
        );

        audio.publish(native_frame(99, 640, next_stream, next_continuity));
        let Seen::Opened {
            output: next_output,
            session: next_session,
            ..
        } = received(&mut seen).await
        else {
            panic!("fresh wake must open a new session")
        };
        assert_ne!(session, next_session);
        let Seen::Audio(frame) = received(&mut seen).await else {
            panic!("fresh capture must upload")
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
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn native_capture_loss_clears_partial_wake_before_accepting_new_audio() {
    for restart in [false, true] {
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
        let cancel = runtime.cancel.clone();
        let task = tokio::spawn(runtime.run());
        let original = Uuid::new_v4();
        audio.publish(native_frame(97, 0, original, 1));
        reached(&local.wake_checks, 1).await;
        let (next_stream, next_continuity) = if restart {
            (Uuid::new_v4(), 1)
        } else {
            (original, 2)
        };
        audio.publish(native_frame(98, 320, next_stream, next_continuity));
        reached(&local.wake_resets, 2).await;
        audio.publish(native_frame(98, 640, next_stream, next_continuity));
        reached(&local.wake_checks, 2).await;
        assert!(
            seen.try_recv().is_err(),
            "partial wake crossed a native gap"
        );
        assert!(!local.wake_prefix.load(Ordering::SeqCst));

        audio.publish(native_frame(97, 960, next_stream, next_continuity));
        audio.publish(native_frame(98, 1280, next_stream, next_continuity));
        let Seen::Opened { output, .. } = received(&mut seen).await else {
            panic!("a complete fresh wake must still open the session")
        };
        assert_eq!(local.captured.load(Ordering::SeqCst), 4);
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), output.closed())
            .await
            .unwrap();
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    }
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
    let cue_cancel = cancel.clone();
    let cue_task = tokio::spawn(async move {
        cue_cancel.cancelled().await;
    });
    let mut session = Session {
        playback: Arc::new(PlaybackSession::new(Arc::new(LocalAudio::default()))),
        capture: CaptureControlBarrier::default(),
        tx,
        cancel,
        task,
        cue_task,
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
fn native_capture_timing_validates_format_cursor_and_evidence_without_aligning_clocks() {
    let original = Uuid::new_v4();
    for damage in [
        "native format",
        "converted format",
        "converted count",
        "converted cursor",
        "direction",
        "clock identity",
        "resampling flag",
        "missing evidence",
        "new evidence",
    ] {
        let mut clock = CaptureClock::default();
        let mut first = native_frame(1, 0, original, 1);
        let mut next = native_frame(1, 320, original, 1);
        let timing = next.timing.as_mut().unwrap();
        match damage {
            "native format" => timing.native.format.sample_rate_hz += 1,
            "converted format" => timing.converted_format.channels = 2,
            "converted count" => timing.converted_frames -= 1,
            "converted cursor" => timing.converted_start_frame += 1,
            "direction" => timing.native.stream.direction = AudioDirection::Playback,
            "clock identity" => timing.native.clock.clock_id = Uuid::new_v4(),
            "resampling flag" => timing.resampled = false,
            "missing evidence" => next.timing = None,
            "new evidence" => first.timing = None,
            _ => unreachable!(),
        }
        clock.accept(&first).unwrap();
        assert!(clock.accept(&next).is_err(), "accepted {damage}");
    }

    let mut clock = CaptureClock::default();
    clock.accept(&native_frame(1, 0, original, 1)).unwrap();
    let mut next = native_frame(1, 320, original, 1);
    // An input batch may produce no output during conversion. Native cursors
    // and converted cursors are separate, and unknown timestamps remain valid.
    next.timing.as_mut().unwrap().native.start_frame += 960;
    clock.accept(&next).unwrap();
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
        "[speech]\nchannel_id = 'pi-living-room'\npre_roll_ms = 300\nupload_queue_frames = 8",
    )
    .unwrap();
    let mut config = crate::config::EmbeddedConfig::default();
    config.apply_runtime_settings(runtime);
    config.speech.validate(&config.audio).unwrap();
    let saved: crate::config::EmbeddedRuntimeSettings =
        toml::from_str(&toml::to_string(&config.runtime_settings()).unwrap()).unwrap();
    assert_eq!(saved.speech.pre_roll_ms, 300);
    assert_eq!(saved.speech.channel_id, "pi-living-room");
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
        "pi-living-room".into(),
        rx,
        ready_rx,
        sent.clone(),
        move || {
            Ok(VoiceAuth {
                token: format!("fresh-{}", counter.fetch_add(1, Ordering::SeqCst) + 1),
            })
        },
        failed,
    );
    tokio::pin!(stream);
    let Some(device_frame::Payload::Open(open)) = stream.next().await.unwrap().payload else {
        panic!("expected Open");
    };
    assert_eq!(open.channel_id, "pi-living-room");
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

#[tokio::test]
async fn queued_capture_before_wire_ready_fences_resume_and_seeds_later_generation() {
    use alia_core::output_control::{BoundControlCommand, OutputBinding};
    use alia_core::playback_control::*;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let Harness {
        runtime,
        audio,
        local,
        mut seen,
        shutdown,
        server,
        _directory,
        ..
    } = harness_with_ready_gate(16, Some(gate.clone())).await;
    let cancel = runtime.cancel.clone();
    let playback = runtime.playback_session.clone();
    publish(&audio, 99, 0);
    let task = tokio::spawn(runtime.run());
    let Seen::Opened {
        boot,
        session,
        output,
        ..
    } = received(&mut seen).await
    else {
        panic!("Open expected")
    };
    publish(&audio, 7, 320);
    tokio::time::timeout(Duration::from_secs(2), async {
        while local.captured.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        seen.try_recv().is_err(),
        "capture cannot upload before Ready"
    );
    assert_eq!(playback.active_scope().unwrap(), None);
    gate.add_permits(1);
    for sequence in 0..2 {
        let Seen::Audio(frame) = received(&mut seen).await else {
            panic!("Audio expected")
        };
        assert_eq!(frame.sequence, sequence);
        assert!(frame.speech_active);
    }
    let mut scope = VoiceScope {
        device_boot_id: Uuid::parse_str(&boot).unwrap(),
        session_id: Uuid::parse_str(&session).unwrap(),
        generation_epoch: 1,
    };
    for epoch in 1..=2 {
        scope.generation_epoch = epoch;
        if epoch == 2 {
            playback.fence(&scope, false).unwrap();
        }
        let binding = OutputBinding {
            scope,
            response_id: Uuid::new_v4(),
            context_id: Uuid::new_v4(),
            server_instance_id: Uuid::new_v4(),
            initiative_capture: None,
        };
        let registered = playback.register_output(&binding).unwrap();
        let pause = BoundControlCommand {
            binding,
            expected_backend: registered.backend,
            command: ControlCommand {
                scope,
                hold_id: Uuid::new_v4(),
                command_id: Uuid::new_v4(),
                sequence: 1,
                action: ControlAction::Pause {
                    reason: HoldReason::AcousticCandidate,
                    through_sequence: 0,
                    budget_ms: 30_000,
                },
            },
        };
        assert_eq!(
            playback
                .apply_bound_control(&pause)
                .unwrap()
                .receipt
                .snapshot
                .phase,
            ControlPhase::Held
        );
        let resume = BoundControlCommand {
            command: ControlCommand {
                command_id: Uuid::new_v4(),
                sequence: 2,
                action: ControlAction::Resume {
                    authorization: ResumeAuthorization {
                        kind: ResumeKind::FreshBackchannel,
                        detection_id: Uuid::new_v4(),
                        through_sequence: 0,
                    },
                },
                ..pause.command.clone()
            },
            ..pause
        };
        assert_eq!(
            playback
                .apply_bound_control(&resume)
                .unwrap()
                .receipt
                .outcome,
            CommandOutcome::Rejected(ControlRejection::StaleEvidence)
        );
    }
    cancel.cancel();
    task.await.unwrap();
    drop(output);
    shutdown.cancel();
    server.await.unwrap();
}

#[test]
fn ready_rejects_previous_version_and_missing_or_excessive_hold_capability() {
    let boot = Uuid::new_v4().to_string();
    let session = Uuid::new_v4().to_string();
    for invalid in 0..4 {
        let playback = Arc::new(PlaybackSession::new(Arc::new(LocalAudio::default())));
        let mut event = ready(&boot, &session);
        let Some(session_event::Payload::Ready(ref mut ready)) = event.payload else {
            unreachable!()
        };
        match invalid {
            0 => ready.protocol_version = 1,
            1 => ready.software_hold = false,
            2 => ready.max_hold_ms = 0,
            _ => ready.max_hold_ms = MAX_HOLD_MS + 1,
        }
        assert!(
            SessionGuard::new(boot.clone(), playback.clone())
                .accept(event, 0)
                .is_err()
        );
        assert_eq!(playback.active_scope().unwrap(), None);
    }
}

#[test]
fn paired_output_decision_validates_epoch_and_never_resumes_from_semantic_evidence() {
    use alia_core::output_control::{BoundControlCommand, OutputBinding};
    use alia_core::playback_control::*;
    let boot = Uuid::new_v4().to_string();
    let session = Uuid::new_v4().to_string();
    let playback = Arc::new(PlaybackSession::new(Arc::new(LocalAudio::default())));
    let mut guard = SessionGuard::new(boot.clone(), playback.clone());
    playback
        .accept_capture(&guard.capture, 0, true, || Ok(()))
        .unwrap();
    guard.accept(ready(&boot, &session), 0).unwrap();
    let scope = playback.active_scope().unwrap().unwrap();
    let binding = OutputBinding {
        scope,
        response_id: Uuid::new_v4(),
        context_id: Uuid::new_v4(),
        server_instance_id: Uuid::new_v4(),
        initiative_capture: None,
    };
    let registration = playback.register_output(&binding).unwrap();
    let command = BoundControlCommand {
        binding,
        expected_backend: registration.backend,
        command: ControlCommand {
            scope,
            hold_id: Uuid::new_v4(),
            command_id: Uuid::new_v4(),
            sequence: 1,
            action: ControlAction::Pause {
                reason: HoldReason::ExplicitWait,
                through_sequence: 0,
                budget_ms: 30_000,
            },
        },
    };
    playback.apply_bound_control(&command).unwrap();
    for decision in [
        OutputDecision::PauseRequested,
        OutputDecision::ResumeRequested,
        OutputDecision::Unchanged,
    ] {
        let mut event = evidence(&boot, &session, 0, TurnState::Wait);
        event.generation_epoch = 1;
        let Some(session_event::Payload::Evidence(ref mut value)) = event.payload else {
            unreachable!()
        };
        value.output_decision = decision as i32;
        guard.accept(event, 1).unwrap();
        assert_eq!(
            playback
                .observe_bound_control(&command)
                .unwrap()
                .receipt
                .snapshot
                .phase,
            ControlPhase::Held
        );
    }
    guard
        .accept(evidence(&boot, &session, 0, TurnState::Wait), 1)
        .unwrap();
    assert!(playback.observe_bound_control(&command).is_err());
    assert_eq!(
        playback.active_scope().unwrap().unwrap().generation_epoch,
        3
    );

    for (state, decision, enqueue, epoch, valid) in [
        (
            TurnState::Complete,
            OutputDecision::Unchanged,
            true,
            2,
            true,
        ),
        (
            TurnState::Complete,
            OutputDecision::Unchanged,
            true,
            1,
            false,
        ),
        (
            TurnState::Complete,
            OutputDecision::PauseRequested,
            true,
            2,
            false,
        ),
        (TurnState::Wait, OutputDecision::Unchanged, true, 2, false),
        (TurnState::Wait, OutputDecision::Unchanged, false, 1, true),
        (
            TurnState::Wait,
            OutputDecision::PauseRequested,
            false,
            1,
            true,
        ),
        (
            TurnState::Backchannel,
            OutputDecision::ResumeRequested,
            false,
            1,
            true,
        ),
        (
            TurnState::Wait,
            OutputDecision::PauseRequested,
            false,
            2,
            false,
        ),
        (
            TurnState::Backchannel,
            OutputDecision::ResumeRequested,
            false,
            2,
            false,
        ),
        (TurnState::Wait, OutputDecision::Cancelled, false, 2, true),
        (TurnState::Wait, OutputDecision::Cancelled, false, 1, false),
        (
            TurnState::Complete,
            OutputDecision::Unchanged,
            false,
            2,
            false,
        ),
        (
            TurnState::Incomplete,
            OutputDecision::Unspecified,
            false,
            1,
            false,
        ),
        (
            TurnState::Unspecified,
            OutputDecision::Unchanged,
            false,
            1,
            false,
        ),
    ] {
        let playback = Arc::new(PlaybackSession::new(Arc::new(LocalAudio::default())));
        let mut guard = SessionGuard::new(boot.clone(), playback);
        guard.accept(ready(&boot, &session), 0).unwrap();
        let mut event = evidence(&boot, &session, 0, state);
        event.generation_epoch = epoch;
        let Some(session_event::Payload::Evidence(ref mut value)) = event.payload else {
            unreachable!()
        };
        value.response_enqueued = enqueue;
        value.output_decision = decision as i32;
        assert_eq!(
            guard.accept(event, 1).is_ok(),
            valid,
            "{state:?} {decision:?} enqueue={enqueue} epoch={epoch}"
        );
    }
}

mod binding_admission;
mod cues;
mod initiative;
