//! Explicit field acceptance: real remote Embedded capture/playback, production
//! VoiceSession and Delivery. Only turn labels and generated response PCM are fixtures.
//! User PCM is forwarded in memory; logs contain scalar evidence, never recordings.
use super::*;
use alia_core::voice::{DetectionId, ModelIdentity, PcmWindow, TurnObservation, TurnState};
use alia_proto::voice::{
    self, device_frame, session_event,
    voice_session_client::VoiceSessionClient,
    voice_session_server::{VoiceSession, VoiceSessionServer},
};
use alia_server::{
    channels::ChannelRegistry,
    grpc::{voice_auth::VoiceIngressAuth, voice_session::VoiceSessionService},
    modules::{
        interaction::{
            turn_detector::{DetectionError, TurnDetector},
            voice_input,
        },
        platform::lifecycle::RuntimeTasks,
    },
};
use anyhow::ensure;
use futures::Stream;
use sea_orm::{DatabaseBackend, Statement};
use std::pin::Pin;
use tokio::sync::{mpsc, oneshot};

#[path = "r1_live/system_control.rs"]
mod system_control;

const CHANNEL: &str = "r1-live";
fn mark(stage: &str, evidence: serde_json::Value) {
    eprintln!(
        "ALIA_R1_LIVE {}",
        json!({"stage":stage,"evidence":evidence})
    );
}

#[derive(Clone, Default, serde::Serialize)]
struct CaptureStats {
    frames: u64,
    samples: u64,
    speech_samples: u64,
    current_speech_samples: u64,
    longest_speech_samples: u64,
    peak: i32,
    #[serde(skip)]
    window_samples: u64,
    #[serde(skip)]
    window_speech_samples: u64,
    #[serde(skip)]
    window_peak: i32,
}
impl CaptureStats {
    fn natural_long_utterance_ready(&self, window_ms: u32) -> bool {
        window_ms >= 20000 && self.speech_samples >= 320000
    }

    fn accept(&mut self, frame: &voice::CaptureFrame) -> anyhow::Result<()> {
        ensure!(
            frame.sequence == self.frames && frame.sample_offset == self.samples,
            "capture clock discontinuity"
        );
        ensure!(
            !frame.pcm_s16le.is_empty()
                && frame.pcm_s16le.len() % 2 == 0
                && frame.pcm_s16le.len() <= 3200,
            "invalid capture frame size"
        );
        self.frames += 1;
        let samples = frame.pcm_s16le.len() as u64 / 2;
        self.samples += samples;
        self.window_samples += samples;
        self.current_speech_samples = if frame.speech_active {
            self.current_speech_samples + samples
        } else {
            0
        };
        let previous_speech_samples = self.speech_samples;
        self.speech_samples += if frame.speech_active { samples } else { 0 };
        self.window_speech_samples += if frame.speech_active { samples } else { 0 };
        let previous = self.longest_speech_samples;
        self.longest_speech_samples = previous.max(self.current_speech_samples);
        for pair in frame.pcm_s16le.chunks_exact(2) {
            let magnitude = i32::from(i16::from_le_bytes([pair[0], pair[1]])).abs();
            self.peak = self.peak.max(magnitude);
            self.window_peak = self.window_peak.max(magnitude);
        }
        if self.window_samples >= 16000 * 5 {
            mark(
                "capture_window",
                json!({"start_sample_offset":self.samples - self.window_samples,"end_sample_offset":self.samples,"samples":self.window_samples,"speech_samples":self.window_speech_samples,"peak":self.window_peak}),
            );
            self.window_samples = 0;
            self.window_speech_samples = 0;
            self.window_peak = 0;
        }
        if previous_speech_samples < 320000 && self.speech_samples >= 320000 {
            mark(
                "cumulative_speech_threshold",
                json!({"seconds":20,"speech_samples":self.speech_samples,"received_samples":self.samples}),
            );
        }
        for seconds in [15, 20] {
            if previous < seconds * 16000 && self.longest_speech_samples >= seconds * 16000 {
                mark(
                    "continuous_speech_threshold",
                    json!({"seconds":seconds,"received_samples":self.samples}),
                );
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct RelayState {
    sessions: usize,
    forwarding_streams: usize,
    ready: Option<VoiceScope>,
    ready_format: Option<(u32, u32)>,
    stats: CaptureStats,
    error: Option<&'static str>,
    disconnect: Option<CancellationToken>,
    stopping: bool,
}
struct ForwardingStream(Arc<Mutex<RelayState>>);
impl Drop for ForwardingStream {
    fn drop(&mut self) {
        self.0.lock().unwrap().forwarding_streams -= 1;
    }
}
impl RelayState {
    fn transport_error(&mut self, source: &'static str, status: &Status) {
        if self.stopping && status.code() == tonic::Code::Cancelled {
            mark(
                "expected_stop_transport_cancellation",
                json!({"source":source}),
            );
        } else {
            self.error = Some(source);
        }
    }
}
#[derive(Clone)]
struct Relay {
    endpoint: String,
    state: Arc<Mutex<RelayState>>,
    stop: CancellationToken,
}
#[tonic::async_trait]
impl VoiceSession for Relay {
    type StreamStream = Pin<Box<dyn Stream<Item = Result<voice::SessionEvent, Status>> + Send>>;
    async fn stream(
        &self,
        request: Request<tonic::Streaming<voice::DeviceFrame>>,
    ) -> Result<Response<Self::StreamStream>, Status> {
        let metadata = request.metadata().clone();
        let mut input = request.into_inner();
        let cancel = self.stop.child_token();
        {
            let mut state = self.state.lock().unwrap();
            state.sessions += 1;
            state.forwarding_streams += 1;
            state.stats = CaptureStats::default();
            state.ready = None;
            state.ready_format = None;
            state.disconnect = Some(cancel.clone());
            state.stopping = false;
        }
        let capture_cancel = cancel.clone();
        let capture_state = self.state.clone();
        let forwarding = ForwardingStream(self.state.clone());
        let forward = async_stream::stream! {
            let _forwarding = forwarding;
            loop {
                let frame = tokio::select! { biased;
                    _ = capture_cancel.cancelled() => break,
                    frame = input.message() => frame,
                };
                let frame = match frame {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break,
                    Err(error) => { capture_state.lock().unwrap().transport_error("capture_transport_error", &error); break; },
                };
                if let Some(device_frame::Payload::Audio(audio)) = &frame.payload {
                    if capture_state.lock().unwrap().stats.accept(audio).is_err() {
                        capture_state.lock().unwrap().error = Some("capture_clock_or_format_error");
                        break;
                    }
                }
                yield frame;
            }
        };
        let mut forwarded = Request::new(forward);
        *forwarded.metadata_mut() = metadata;
        let mut client = VoiceSessionClient::connect(self.endpoint.clone())
            .await
            .map_err(|_| Status::unavailable("local production voice listener unavailable"))?;
        let mut output = client.stream(forwarded).await?.into_inner();
        let state = self.state.clone();
        Ok(Response::new(Box::pin(async_stream::stream! {
            let _cancel = cancel.clone().drop_guard();
            loop {
                let event = tokio::select! { biased;
                    _ = cancel.cancelled() => { yield Err(Status::unavailable("field test disconnect")); break; },
                    event = output.message() => event,
                };
                match event {
                    Ok(Some(event)) => {
                        if let Some(session_event::Payload::Ready(ready)) = &event.payload {
                            let scope = VoiceScope { device_boot_id: event.device_boot_id.parse().unwrap(), session_id: event.session_id.parse().unwrap(), generation_epoch: event.generation_epoch };
                            let mut state = state.lock().unwrap();
                            state.ready = Some(scope);
                            state.ready_format = Some((ready.sample_rate_hz, ready.channels));
                            mark("authenticated_ready", json!({"session_number":state.sessions}));
                        }
                        yield Ok(event);
                    },
                    Ok(None) => break,
                    Err(error) => { state.lock().unwrap().transport_error("production_voice_session_error", &error); yield Err(error); break; },
                }
            }
        })))
    }
}

struct Detection {
    id: DetectionId,
    window_ms: u32,
    reply: oneshot::Sender<TurnObservation>,
}
impl Detection {
    fn answer(self, state: TurnState, fixture_text: &str) {
        self.reply
            .send(TurnObservation {
                id: self.id,
                state,
                transcript: fixture_text.into(),
                inference_ms: 0.0,
                model: ModelIdentity {
                    source: "field-test-fixed-label".into(),
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
        audio: &PcmWindow,
        cancel: &CancellationToken,
    ) -> Result<TurnObservation, DetectionError> {
        let (reply, receive) = oneshot::channel();
        self.0
            .send(Detection {
                id,
                window_ms: audio.duration_ms(),
                reply,
            })
            .await
            .map_err(|_| DetectionError::Cancelled)?;
        tokio::select! { _ = cancel.cancelled() => Err(DetectionError::Cancelled), result = receive => result.map_err(|_| DetectionError::Cancelled) }
    }
}

#[derive(Default)]
struct Live {
    channel: Option<Arc<EmbeddedChannel>>,
    service: Option<Arc<DeliveryService>>,
    servers: Vec<RpcServer>,
    tasks: Arc<RuntimeTasks>,
    stop: CancellationToken,
    media: Vec<tokio::task::JoinHandle<DeliveryResult<DeliveryRecord>>>,
    server_id: String,
    attempted_bind: bool,
    directory: Option<Directory>,
}
impl Live {
    async fn close(mut self) {
        self.stop.cancel();
        self.tasks.close();
        let mut failed = Vec::new();
        for server in self.servers.drain(..).rev() {
            if AssertUnwindSafe(server.close())
                .catch_unwind()
                .await
                .is_err()
            {
                failed.push("RPC join");
            }
        }
        if !matches!(
            tokio::time::timeout(Duration::from_secs(10), self.tasks.wait()).await,
            Ok(Ok(()))
        ) {
            failed.push("runtime tasks join");
        }
        if let Some(service) = self.service.take() {
            let (outcome, error) = match AssertUnwindSafe(service.shutdown()).catch_unwind().await {
                Ok(Ok(())) => ("ok", None),
                Ok(Err(error)) => ("error", Some(format!("{error:#}"))),
                Err(_) => ("panic", None),
            };
            let pending = service.shutdown_pending();
            mark(
                "delivery_shutdown",
                json!({"outcome":outcome,"error":error,"shutdown_pending":pending}),
            );
            if outcome != "ok" || pending {
                failed.push("delivery shutdown");
            }
        }
        for mut media in self.media {
            match tokio::time::timeout(Duration::from_secs(5), &mut media).await {
                Ok(Ok(_)) => {}
                Ok(Err(_)) => failed.push("media join"),
                Err(_) => {
                    media.abort();
                    let _ = media.await;
                    failed.push("media join timeout");
                }
            }
        }
        if let Some(channel) = self.channel.take() {
            if self.attempted_bind {
                let unbind = async {
                    let mut binding = channel.binding_client().await?;
                    let status = binding.get_binding_status(()).await?.into_inner();
                    if status.is_bound && status.bound_server_id == self.server_id {
                        let result = channel
                            .execute_action(ChannelAction {
                                name: "unbind_device".into(),
                                params: json!({"server_id":self.server_id}),
                            })
                            .await?;
                        ensure!(result["success"] == true, "unbind rejected");
                    }
                    Ok::<_, anyhow::Error>(())
                };
                if !matches!(
                    AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(10), unbind))
                        .catch_unwind()
                        .await,
                    Ok(Ok(Ok(())))
                ) {
                    failed.push("own binding cleanup");
                }
            }
            if !matches!(
                AssertUnwindSafe(channel.stop()).catch_unwind().await,
                Ok(Ok(()))
            ) {
                failed.push("channel stop");
            }
        }
        mark(
            "server_cleanup_complete",
            json!({"runtime_tasks":self.tasks.active(),"pcm_saved":false,"failed":failed}),
        );
        assert!(failed.is_empty(), "field cleanup failed at {failed:?}");
    }
}

async fn serve<S>(
    service: VoiceSessionServer<S>,
    address: std::net::SocketAddr,
    stop: CancellationToken,
) -> (RpcServer, String)
where
    S: VoiceSession,
{
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let shutdown = stop.clone();
    let task = tokio::spawn(async move {
        let incoming = async_stream::stream! { loop { yield listener.accept().await.map(|(stream, _)| stream); } };
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming_shutdown(incoming, shutdown.cancelled())
            .await
            .unwrap();
    });
    (
        RpcServer {
            stop,
            task: Some(task),
        },
        endpoint,
    )
}

async fn detection(
    receiver: &mut mpsc::Receiver<Detection>,
    state: &Arc<Mutex<RelayState>>,
) -> Detection {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Some(error) = state.lock().unwrap().error {
                panic!("field capture failed: {error}");
            }
            tokio::select! {
                result = receiver.recv() => return result.expect("turn detector owner ended"),
                _ = tokio::time::sleep(Duration::from_millis(100)) => {},
            }
        }
    })
    .await
    .expect("60 second utterance window expired")
}

async fn command_evidence(f: &Fixture, response: Uuid, required: usize) -> Vec<serde_json::Value> {
    let mut evidence = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let rows = f.db.query_all(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "SELECT command->'command'->'action' AS action, latest_receipt->'receipt'->'outcome' AS outcome, latest_receipt->'receipt'->'snapshot'->'phase' AS phase FROM output_control_commands WHERE response_id=$1 AND status='receipted' ORDER BY sequence", [response.into()])).await.unwrap();
            if rows.len() >= required {
                evidence = rows.iter().map(|row| json!({"action":row.try_get::<serde_json::Value>("","action").unwrap(),"outcome":row.try_get::<serde_json::Value>("","outcome").unwrap(),"phase":row.try_get::<serde_json::Value>("","phase").unwrap()})).collect();
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await;
    if result.is_err() {
        for (stage, query) in [
            (
                "failed_control_commands",
                "SELECT jsonb_build_object('sequence',sequence,'action',command->'command'->'action','status',status,'unknown_reason',unknown_reason,'latest_receipt',latest_receipt) AS evidence FROM output_control_commands WHERE response_id=$1 ORDER BY sequence",
            ),
            (
                "failed_delivery_progress",
                "SELECT jsonb_build_object('status',status,'offered_frames',offered_frames,'accepted_frames',accepted_frames,'submitted_frames',submitted_frames,'outcome',outcome,'progress',progress) AS evidence FROM deliveries WHERE response_id=$1",
            ),
            (
                "failed_output_scope",
                "SELECT jsonb_build_object('status',status,'retirement_reason',retirement_reason,'latest_receipt',latest_receipt) AS evidence FROM output_control_scopes WHERE response_id=$1",
            ),
        ] {
            let rows = tokio::time::timeout(
                Duration::from_secs(1),
                f.db.query_all(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    query,
                    [response.into()],
                )),
            )
            .await;
            match rows {
                Ok(Ok(rows)) => mark(
                    stage,
                    json!({"rows":rows.iter().map(|row| row.try_get::<serde_json::Value>("", "evidence").unwrap()).collect::<Vec<_>>()}),
                ),
                Ok(Err(error)) => mark(stage, json!({"query_error":error.to_string()})),
                Err(_) => mark(stage, json!({"query_timeout":true})),
            }
        }
    }
    result.expect("missing remote control receipts");
    evidence
}

fn field_media_stream(
    budget: ResponseAudioBudget,
    stop: CancellationToken,
    next_tone: Arc<AtomicBool>,
) -> alia_server::modules::delivery::transport::AudioPayloadStream {
    Box::pin(async_stream::stream! {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        let mut first = true;
        loop {
            let scheduled = tokio::select! { biased; _ = stop.cancelled() => break, scheduled = tick.tick() => scheduled };
            // Match the 300 ms software target; a long Held period is not missing media.
            if scheduled.elapsed() > Duration::from_millis(300) {
                tick.reset();
            }
            let audible = next_tone.swap(false, Ordering::AcqRel);
            let tone_samples = if audible { 3200 } else { 0 };
            let samples = if first { 6400 } else { tone_samples.max(1600) };
            first = false;
            let pcm: Vec<u8> = (0..samples).flat_map(|sample| {
                let value = if sample < tone_samples {
                    let envelope = (sample.min(tone_samples - 1 - sample) as f32 / 160.0).min(1.0);
                    (24576.0 * envelope * (sample as f32 * std::f32::consts::TAU * 440.0 / 16000.0).sin()) as i16
                } else { 0 };
                value.to_le_bytes()
            }).collect();
            let data = budget.copy_from_slice(&pcm).await.unwrap().into_buffer();
            yield Ok(AudioPayload { data, sample_rate:16000, channels:1, format:AudioFormat::Pcm16 });
        }
    })
}

#[tokio::test(start_paused = true)]
async fn field_media_catches_up_after_a_delayed_poll() {
    use futures::StreamExt;
    let mut stream = field_media_stream(
        ResponseAudioBudget::for_response(Uuid::new_v4()).unwrap(),
        CancellationToken::new(),
        Arc::new(AtomicBool::new(true)),
    );
    stream.next().await.unwrap().unwrap();
    tokio::time::advance(Duration::from_millis(100)).await;
    stream.next().await.unwrap().unwrap();
    // The consumer resumes at 350 ms: both the 200 and 300 ms slots are due.
    tokio::time::advance(Duration::from_millis(250)).await;
    for scheduled_ms in [200, 300] {
        let payload = stream
            .next()
            .now_or_never()
            .unwrap_or_else(|| {
                panic!("fixture permanently skipped the {scheduled_ms} ms audio slot")
            })
            .unwrap()
            .unwrap();
        assert_eq!(payload.data.len(), 3200);
        assert!(payload.data.iter().all(|byte| *byte == 0));
    }
    assert!(stream.next().now_or_never().is_none());
}

#[tokio::test(start_paused = true)]
async fn field_media_prebuffers_without_extending_tones_or_replaying_a_long_hold() {
    use futures::StreamExt;
    let stop = CancellationToken::new();
    let tone = Arc::new(AtomicBool::new(true));
    let mut stream = field_media_stream(
        ResponseAudioBudget::for_response(Uuid::new_v4()).unwrap(),
        stop.clone(),
        tone.clone(),
    );
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.data.len(), 12800, "400 ms startup PCM");
    assert!(first.data[..6400].iter().any(|byte| *byte != 0));
    assert!(
        first.data[6400..].iter().all(|byte| *byte == 0),
        "only the first 200 ms may be audible"
    );
    tokio::time::advance(Duration::from_secs(3)).await;
    let after_hold = stream.next().now_or_never().unwrap().unwrap().unwrap();
    assert_eq!(after_hold.data.len(), 3200);
    assert!(after_hold.data.iter().all(|byte| *byte == 0));
    assert!(
        stream.next().now_or_never().is_none(),
        "a long hold must not enqueue seconds of stale silence"
    );
    tone.store(true, Ordering::Release);
    tokio::time::advance(Duration::from_millis(100)).await;
    let resumed_tone = stream.next().await.unwrap().unwrap();
    assert_eq!(resumed_tone.data.len(), 6400, "resume tone stays 200 ms");
    assert!(resumed_tone.data.iter().any(|byte| *byte != 0));
    stop.cancel();
    assert!(matches!(stream.next().now_or_never(), Some(None)));
}

async fn start_media(
    live: &mut Live,
    f: &Fixture,
    response: &ResponseRecord,
    scope: VoiceScope,
) -> Arc<AtomicBool> {
    let tone = Arc::new(AtomicBool::new(true));
    let stream = field_media_stream(
        ResponseAudioBudget::for_response(response.response_id).unwrap(),
        live.stop.clone(),
        tone.clone(),
    );
    let service = live.service.as_ref().unwrap().clone();
    let audio = AudioDelivery {
        response_id: response.response_id,
        output_key: "field-tone-and-silence".into(),
        content: json!({"fixture":"0.2 second 440Hz tone plus silence","peak":24576}),
        channel: live.channel.as_ref().unwrap().clone(),
        target: DeliveryTarget {
            user_id: f.user.to_string(),
            voice: Some(scope),
        },
        audio: stream,
        cancel: live.stop.child_token(),
    };
    live.media.push(tokio::spawn(
        async move { service.deliver_audio(audio).await },
    ));
    tone
}

async fn wait_submitted(live: &Live, f: &Fixture, response: Uuid, minimum: u64) -> u64 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let rows = live.service.as_ref().unwrap().list_deliveries(f.user, response).await.unwrap();
            if let Some(row) = rows.iter().find(|row| row.submitted_frames.is_some_and(|frames| frames >= minimum)) {
                let backend = row.progress.as_ref().unwrap().backend.as_ref().unwrap();
                assert_eq!(backend.boundary, SubmissionBoundary::AlsaWriteAccepted);
                assert_eq!((backend.output_sample_rate_hz, backend.output_channels), (48000, 2));
                assert_eq!(row.presented_frames, None);
                mark("alsa_submission", json!({"submitted_frames":row.submitted_frames,"boundary":backend.boundary,"output_sample_rate_hz":backend.output_sample_rate_hz,"output_channels":backend.output_channels,"presented_frames":row.presented_frames}));
                return row.submitted_frames.unwrap();
            }
            assert!(live.media.last().is_some_and(|task| !task.is_finished()), "media stopped before host submission evidence");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("remote ALSA submission evidence missing")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "opens an explicitly configured remote R1/ALSA node; requires natural long speech with breaths, two short utterances, rewake and low-volume playback"]
async fn remote_r1_natural_long_speech_real_server_duplex_and_rewake() {
    remote_r1_real_server_acceptance(Acceptance::NaturalLong).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "opens an explicitly configured remote R1/ALSA node; requires human short utterances, rewake and low-volume playback; does not validate natural long speech"]
async fn remote_r1_short_speech_real_server_duplex_and_rewake() {
    remote_r1_real_server_acceptance(Acceptance::NaturalShort).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicitly initialized remote R1, three ordinary speech windows, 300 seconds silent idle, operator gates and an ordinary Embedded process restart"]
async fn remote_r1_system_start_stop_and_silent_recovery() {
    remote_r1_real_server_acceptance(Acceptance::SystemSpeech).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses an explicitly configured remote R1 and real Server; automatic PCM transport and Start/Stop checks with an ordinary Embedded restart gate, no speech, playback or UART initialization"]
async fn remote_r1_system_control_transport() {
    remote_r1_real_server_acceptance(Acceptance::SystemTransport).await;
}

#[derive(Clone, Copy)]
enum Acceptance {
    NaturalLong,
    NaturalShort,
    SystemSpeech,
    SystemTransport,
}

async fn remote_r1_real_server_acceptance(acceptance: Acceptance) {
    let require_long_speech = matches!(acceptance, Acceptance::NaturalLong);
    let system_control = matches!(
        acceptance,
        Acceptance::SystemSpeech | Acceptance::SystemTransport
    );
    let automatic = matches!(acceptance, Acceptance::SystemTransport);
    let acceptance_scope = match acceptance {
        Acceptance::NaturalLong => "natural_long_speech_duplex_and_rewake",
        Acceptance::NaturalShort => "short_speech_duplex_and_rewake",
        Acceptance::SystemSpeech => "system_start_stop_and_silent_recovery",
        Acceptance::SystemTransport => "system_control_transport_and_process_restart",
    };
    let remote = std::env::var("ALIA_R1_EMBEDDED_ENDPOINT")
        .expect("explicit remote Embedded endpoint required");
    let device_id = std::env::var("ALIA_R1_DEVICE_ID").expect("pinned device identity required");
    let public = std::env::var("ALIA_R1_DEVICE_PUBLIC_KEY")
        .expect("pinned base64 device public key required");
    let base = std::env::var("ALIA_R1_SERVER_ENDPOINT").expect("HTTP base endpoint required");
    let voice_endpoint =
        alia_embedded::module::speech::transport::derive_voice_endpoint(&base).unwrap();
    let listen = voice_endpoint
        .strip_prefix("http://")
        .expect("fixture uses loopback HTTP tunnel")
        .parse::<std::net::SocketAddr>()
        .unwrap();
    assert!(
        listen.ip().is_loopback(),
        "use the authorized SSH loopback tunnel"
    );
    let listen = std::env::var("ALIA_R1_VOICE_LISTEN")
        .map(|address| {
            let address = address.parse::<std::net::SocketAddr>().unwrap();
            assert_eq!(
                address.port(),
                listen.port(),
                "listener must match the derived voice port"
            );
            assert!(
                address.ip().is_loopback() || address.ip().is_unspecified(),
                "explicit container listener must use the local interface"
            );
            address
        })
        .unwrap_or(listen);
    let seconds = std::env::var("ALIA_R1_LIVE_TIMEOUT_SEC")
        .unwrap_or_else(|_| {
            if automatic {
                "180"
            } else if system_control {
                "1200"
            } else {
                "300"
            }
            .into()
        })
        .parse::<u64>()
        .unwrap();
    assert!(if automatic {
        (120..=300).contains(&seconds)
    } else if system_control {
        (900..=1200).contains(&seconds)
    } else {
        (180..=600).contains(&seconds)
    });
    let gates = system_control.then(system_control::gates);
    let mut database =
        delivery_support::FixtureInitialization::start_with(|_, _| Box::pin(async {}));
    let f = database.ready().await;
    let mut live = Live::default();
    let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(seconds), async {
        live.directory = Some(Directory(std::env::temp_dir().join(format!("alia-r1-field-{}", Uuid::new_v4()))));
        let directory = &live.directory.as_ref().unwrap().0;
        std::fs::create_dir(directory).unwrap();
        let key = directory.join("server.key");
        alia_server::auth::ensure_server_keypair(&key).unwrap();
        live.server_id = format!("r1-field-{}", Uuid::new_v4());
        let mut config: alia_server::config::ServerConfig = toml::from_str(include_str!("../../../../config/server.example.toml")).unwrap();
        config.server.id = live.server_id.clone();
        config.server.host = "127.0.0.1".into();
        let mut channel = EmbeddedChannel::new(EmbeddedChannelConfig { id:CHANNEL.into(), owner_user_id:Some(f.user), expected_device_id:Some(device_id), expected_device_public_key:Some(public), approved_animations:vec![], endpoint:remote, discovery_service_name:"unused-r1-field".into(), discovery_timeout_sec:1, reconnect_interval_sec:1, reconnect_max_backoff_sec:1, grpc_timeout_sec:5 }, EventBus::new(16))
            .with_runtime(EmbeddedRuntimeDependencies { db:f.db.clone(), sensors:Arc::new(SensorRegistry::new(Default::default())), server:config.server, server_keypair_path:key.clone() });
        channel.init().await.unwrap();
        let channel = Arc::new(channel);
        live.channel = Some(channel.clone());
        let mut binding = channel.binding_client().await.unwrap();
        assert!(!binding.get_binding_status(()).await.unwrap().into_inner().is_bound, "field node must use a fresh unbound private state");
        let channels = Arc::new(ChannelRegistry::new());
        channels.register(channel.clone()).await.unwrap();
        let service = DeliveryService::new(f.repo(), &live.stop);
        service.start().await.unwrap();
        live.service = Some(service.clone());
        let (input, mut recognized) = voice_input::channel();
        let (detector, mut calls) = mpsc::channel(1);
        let voice = VoiceSessionService::new(VoiceIngressAuth::new(channels), Some(Arc::new(Detector(detector))), input, service.clone(), Duration::from_secs(10), live.stop.clone(), live.tasks.clone()).with_barge_in_enabled(true);
        let (server, endpoint) = serve(VoiceSessionServer::new(voice), "127.0.0.1:0".parse().unwrap(), live.stop.clone()).await;
        live.servers.push(server);
        let state = Arc::new(Mutex::new(RelayState::default()));
        let (server, _) = serve(VoiceSessionServer::new(Relay { endpoint, state:state.clone(), stop:live.stop.clone() }), listen, live.stop.clone()).await;
        live.servers.push(server);
        live.attempted_bind = true;
        let bound = channel.execute_action(ChannelAction { name:"bind_device".into(), params:json!({"server_id":live.server_id,"server_name":"R1 field acceptance","server_endpoint":base,"require_confirmation":false}) }).await.unwrap();
        assert_eq!(bound["success"], true);
        if let Some(gates) = gates {
            if automatic {
                system_control::transport_acceptance(&live, &key, &gates, &state, &mut calls, &mut recognized).await;
            } else {
                system_control::acceptance(&live, &key, &gates, &state, &mut calls, &mut recognized).await;
            }
            return;
        }
        let volume = channel.execute_action(ChannelAction { name:"get_volume".into(), params:json!({}) }).await.unwrap();
        let gain = volume["playback_volume"].as_f64().unwrap();
        assert_eq!(gain, 1.0, "effective playback gain must match the authorized 30-fold tone test");
        let ready_stage = if require_long_speech { "ready_natural_wake_then_22_to_25_seconds_natural_speech" } else { "ready_natural_wake_then_short_phrase" };
        mark(ready_stage, json!({"acceptance_scope":acceptance_scope,"pcm_saved":false,"turn_model":"fixed_fixture","tone_peak":24576,"tone_ms":200,"effective_playback_gain":gain}));
        loop {
            let detected = detection(&mut calls, &state).await;
            let stats = state.lock().unwrap().stats.clone();
            let window_stage = if require_long_speech { "natural_long_utterance_window" } else { "short_utterance_window" };
            mark(window_stage, json!({"window_ms":detected.window_ms,"capture":stats}));
            if !require_long_speech || stats.natural_long_utterance_ready(detected.window_ms) {
                assert!(detected.window_ms > 0 && stats.speech_samples > 0 && stats.peak > 0, "first endpoint must contain nonzero speech evidence");
                detected.answer(TurnState::Complete, "field fixture response");
                break;
            }
            detected.answer(TurnState::Incomplete, "");
        }
        let input = tokio::time::timeout(Duration::from_secs(5), recognized.recv()).await.unwrap().unwrap();
        let scope = input.output.as_ref().unwrap().producer.scope();
        let response = speech::bind_response(&f, &service, &input).await;
        let tone = start_media(&mut live, &f, &response, scope).await;
        wait_submitted(&live, &f, response.response_id, 3200).await;
        mark("say_one_short_phrase_for_pause_and_resume", json!({"first_tone_submitted":true,"same_session":true}));
        let detected = detection(&mut calls, &state).await;
        mark("pause_phrase_endpoint", json!({"window_ms":detected.window_ms}));
        detected.answer(TurnState::Backchannel, "嗯");
        let commands = command_evidence(&f, response.response_id, 2).await;
        assert!(commands.iter().any(|item| item["action"].get("Pause").is_some() && item["outcome"] == "Applied"));
        assert!(commands.iter().any(|item| item["action"].get("Resume").is_some() && item["outcome"] == "Applied"));
        mark("pause_resume_receipted", json!({"commands":commands.iter().map(|item| json!({"outcome":item["outcome"],"phase":item["phase"]})).collect::<Vec<_>>()}));
        let before = wait_submitted(&live, &f, response.response_id, 3200).await;
        tone.store(true, Ordering::Release);
        wait_submitted(&live, &f, response.response_id, before + 3200).await;
        mark("say_one_short_phrase_for_cancel", json!({"submission_advanced_after_resume":true}));
        let detected = detection(&mut calls, &state).await;
        mark("cancel_phrase_endpoint", json!({"window_ms":detected.window_ms}));
        detected.answer(TurnState::Complete, "停止");
        tokio::time::timeout(Duration::from_secs(5), input.cancel.cancelled()).await.unwrap();
        let cancelled = tokio::time::timeout(Duration::from_secs(5), live.media.last_mut().unwrap()).await.unwrap().unwrap().unwrap();
        live.media.pop();
        mark("original_output_cancel_result", json!({"status":cancelled.status,"cancel_requested":cancelled.cancel_requested,"offered_frames":cancelled.offered_frames,"accepted_frames":cancelled.accepted_frames,"submitted_frames":cancelled.submitted_frames,"presented_frames":cancelled.presented_frames,"end_accepted":cancelled.end_accepted,"receipt_sequence":cancelled.receipt_sequence,"progress":cancelled.progress,"outcome":cancelled.outcome}));
        assert_eq!(cancelled.status, DeliveryStatus::Cancelled);
        assert!(cancelled.submitted_frames.is_some_and(|n| n >= 3200));
        assert_eq!(cancelled.presented_frames, None);
        mark("cancelled_original_output", json!({"submitted_frames":cancelled.submitted_frames,"presented_frames":cancelled.presented_frames}));
        state.lock().unwrap().disconnect.as_ref().unwrap().cancel();
        tokio::time::timeout(Duration::from_secs(5), async { while live.tasks.active() != 0 { tokio::time::sleep(Duration::from_millis(20)).await; } }).await.unwrap();
        mark("rewake_then_short_phrase", json!({"original_session_drained":true}));
        let detected = detection(&mut calls, &state).await;
        let new_scope = state.lock().unwrap().ready.unwrap();
        assert_ne!(new_scope.session_id, scope.session_id);
        assert_eq!(new_scope.device_boot_id, scope.device_boot_id);
        assert_eq!(state.lock().unwrap().sessions, 2);
        detected.answer(TurnState::Complete, "fresh field session");
        let next = tokio::time::timeout(Duration::from_secs(5), recognized.recv()).await.unwrap().unwrap();
        assert_eq!(next.session_id, new_scope.session_id);
        mark("field_acceptance_complete", json!({"acceptance_scope":acceptance_scope,"new_session":true,"capture":state.lock().unwrap().stats,"pcm_saved":false}));
        state.lock().unwrap().disconnect.as_ref().unwrap().cancel();
        drop(next);
        drop(input);
    })).catch_unwind().await;
    let cleanup = AssertUnwindSafe(live.close()).catch_unwind().await;
    database.close().await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("field acceptance exceeded its explicit window"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
    if let Err(panic) = cleanup {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn scalar_capture_statistics_keep_continuity_and_distinguish_total_from_active_run() {
    let mut stats = CaptureStats::default();
    for sequence in 0..210 {
        stats
            .accept(&voice::CaptureFrame {
                sequence,
                sample_offset: sequence * 1600,
                pcm_s16le: vec![1; 3200],
                speech_active: sequence != 100,
            })
            .unwrap();
    }
    assert_eq!(stats.samples, 336000);
    assert_eq!(stats.longest_speech_samples, 109 * 1600);
    assert_eq!(stats.window_samples, 16000);
    assert_eq!(stats.window_speech_samples, 16000);
    assert_eq!(stats.window_peak, 257);
    assert!(
        stats
            .accept(&voice::CaptureFrame {
                sequence: 212,
                sample_offset: 336000,
                pcm_s16le: vec![0; 3200],
                speech_active: true
            })
            .is_err()
    );
    for sequence in 210..251 {
        stats
            .accept(&voice::CaptureFrame {
                sequence,
                sample_offset: sequence * 1600,
                pcm_s16le: vec![0; 3200],
                speech_active: false,
            })
            .unwrap();
    }
    assert_eq!(stats.peak, 257);
    assert_eq!(stats.speech_samples, 209 * 1600);
    assert_eq!(stats.window_samples, 1600);
    assert_eq!(stats.window_speech_samples, 0);
    assert_eq!(stats.window_peak, 0);
}

#[test]
fn natural_long_speech_accepts_breaths_but_requires_speech_and_window_duration() {
    let mut stats = CaptureStats::default();
    for sequence in 0..207 {
        let speech_active = !(100..107).contains(&sequence);
        stats
            .accept(&voice::CaptureFrame {
                sequence,
                sample_offset: sequence * 1600,
                pcm_s16le: vec![u8::from(speech_active); 3200],
                speech_active,
            })
            .unwrap();
        if sequence < 206 {
            assert!(!stats.natural_long_utterance_ready(20700));
        }
    }
    assert_eq!(stats.samples, 331200);
    assert_eq!(stats.speech_samples, 320000);
    assert_eq!(stats.longest_speech_samples, 160000);
    assert!(!stats.natural_long_utterance_ready(19999));
    assert!(stats.natural_long_utterance_ready(20700));
}

#[tokio::test]
async fn field_relay_forwards_metadata_and_pcm_and_closes_on_owner_stop() {
    struct Upstream;
    #[tonic::async_trait]
    impl VoiceSession for Upstream {
        type StreamStream = Pin<Box<dyn Stream<Item = Result<voice::SessionEvent, Status>> + Send>>;
        async fn stream(
            &self,
            request: Request<tonic::Streaming<voice::DeviceFrame>>,
        ) -> Result<Response<Self::StreamStream>, Status> {
            assert_eq!(
                request.metadata().get("x-field-forwarding-check").unwrap(),
                "unchanged"
            );
            let mut frames = request.into_inner();
            let first = frames.message().await?.unwrap();
            assert_eq!(
                first.payload,
                Some(device_frame::Payload::Audio(voice::CaptureFrame {
                    sequence: 0,
                    sample_offset: 0,
                    pcm_s16le: vec![1, 2, 3, 4],
                    speech_active: true
                }))
            );
            Ok(Response::new(Box::pin(async_stream::stream! {
                yield Ok(voice::SessionEvent { session_id:Uuid::new_v4().to_string(), device_boot_id:Uuid::new_v4().to_string(), generation_epoch:1, payload:Some(session_event::Payload::Ready(voice::SessionReady { sample_rate_hz:16000, channels:1, ..Default::default() })) });
                assert!(frames.message().await.unwrap().is_none());
            })))
        }
    }
    let stop = CancellationToken::new();
    let mut servers = Vec::new();
    let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(5), async {
        let (server, endpoint) = serve(
            VoiceSessionServer::new(Upstream),
            "127.0.0.1:0".parse().unwrap(),
            stop.clone(),
        )
        .await;
        servers.push(server);
        let state = Arc::new(Mutex::new(RelayState::default()));
        let (server, endpoint) = serve(
            VoiceSessionServer::new(Relay {
                endpoint,
                state: state.clone(),
                stop: stop.clone(),
            }),
            "127.0.0.1:0".parse().unwrap(),
            stop.clone(),
        )
        .await;
        servers.push(server);
        let mut client = VoiceSessionClient::connect(endpoint).await.unwrap();
        let (sender, mut frames) = mpsc::channel(1);
        sender
            .send(voice::DeviceFrame {
                payload: Some(device_frame::Payload::Audio(voice::CaptureFrame {
                    sequence: 0,
                    sample_offset: 0,
                    pcm_s16le: vec![1, 2, 3, 4],
                    speech_active: true,
                })),
            })
            .await
            .unwrap();
        let mut request = Request::new(
            async_stream::stream! {while let Some(frame) = frames.recv().await {yield frame;}},
        );
        request
            .metadata_mut()
            .insert("x-field-forwarding-check", "unchanged".parse().unwrap());
        let mut output = client.stream(request).await.unwrap().into_inner();
        assert!(matches!(
            output.message().await.unwrap().unwrap().payload,
            Some(session_event::Payload::Ready(_))
        ));
        assert_eq!(state.lock().unwrap().stats.samples, 2);
        assert_eq!(state.lock().unwrap().ready_format, Some((16000, 1)));
        assert_eq!(state.lock().unwrap().forwarding_streams, 1);
        stop.cancel();
        assert!(output.message().await.is_err());
        drop(sender);
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.lock().unwrap().forwarding_streams != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled relay retained its forwarding stream");
    }))
    .catch_unwind()
    .await;
    stop.cancel();
    let mut cleanup_failed = false;
    for server in servers.into_iter().rev() {
        cleanup_failed |= AssertUnwindSafe(server.close())
            .catch_unwind()
            .await
            .is_err();
    }
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("relay smoke test deadline expired"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
    assert!(!cleanup_failed);
}
