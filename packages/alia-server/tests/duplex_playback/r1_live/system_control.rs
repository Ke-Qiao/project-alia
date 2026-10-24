//! Operator-gated production CaptureControl acceptance; no audio files or playback.
use super::*;
use alia_proto::capture::{
    CaptureRequest, CaptureState, InspectCaptureRequest,
    capture_control_client::CaptureControlClient,
};
use std::path::Path;

const GATES: [&str; 3] = ["start-before", "start-after", "restart-complete"];

pub(super) fn gates() -> PathBuf {
    let path = PathBuf::from(
        std::env::var("ALIA_R1_SYSTEM_GATE_DIR").expect("explicit fresh operator gate directory"),
    );
    assert!(path.is_absolute() && path.is_dir());
    for gate in GATES {
        assert!(!path.join(gate).exists(), "stale operator gate: {gate}");
    }
    path
}

struct Control {
    client: CaptureControlClient<tonic::transport::Channel>,
    key: alia_server::auth::ServerKeypair,
    server_id: String,
}
impl Control {
    async fn connect(key: &Path, server_id: &str) -> Self {
        let endpoint = std::env::var("ALIA_R1_EMBEDDED_ENDPOINT").unwrap();
        Self {
            client: CaptureControlClient::connect(endpoint).await.unwrap(),
            key: alia_server::auth::load_server_keypair(key).unwrap(),
            server_id: server_id.into(),
        }
    }

    fn signed<T>(&self, message: T) -> Request<T> {
        // Every RPC gets a fresh token, including calls after the five-minute idle.
        let token = alia_transport::mint_auth_token(
            self.key.signing_key(),
            &self.server_id,
            alia_transport::EMBEDDED_CONTROL_SCOPE,
            alia_transport::DEFAULT_GRPC_AUTH_TTL,
        )
        .unwrap();
        let mut request = Request::new(message);
        request.set_timeout(Duration::from_secs(5));
        request
            .metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        request
    }

    async fn inspect(&mut self) -> CaptureState {
        let request = self.signed(InspectCaptureRequest {});
        self.client.inspect(request).await.unwrap().into_inner()
    }

    async fn command(&mut self, state: &CaptureState, start: bool) -> Result<CaptureState, Status> {
        let request = self.signed(CaptureRequest {
            device_boot_id: state.device_boot_id.clone(),
            expected_generation: state.generation,
        });
        if start {
            self.client.start(request).await
        } else {
            self.client.stop(request).await
        }
        .map(Response::into_inner)
    }
}

fn idle(state: &RelayState, baseline: (usize, u64, u64)) -> anyhow::Result<()> {
    ensure!(state.error.is_none(), "relay failed: {:?}", state.error);
    ensure!(
        (state.sessions, state.stats.frames, state.stats.samples) == baseline,
        "inactive system capture opened a session or uploaded PCM"
    );
    Ok(())
}

async fn gate(path: &Path, name: &str, state: &Arc<Mutex<RelayState>>) {
    wait_gate(path, name, state, 180, "system_operator_gate").await;
}

async fn wait_gate(
    path: &Path,
    name: &str,
    state: &Arc<Mutex<RelayState>>,
    seconds: u64,
    stage: &str,
) {
    let baseline = {
        let state = state.lock().unwrap();
        (state.sessions, state.stats.frames, state.stats.samples)
    };
    mark(stage, json!({"gate":name,"timeout_sec":seconds}));
    tokio::time::timeout(Duration::from_secs(seconds), async {
        loop {
            idle(&state.lock().unwrap(), baseline).unwrap();
            match std::fs::metadata(path.join(name)) {
                Ok(metadata) => {
                    assert!(metadata.is_file(), "operator gate must be a regular file");
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("reading operator gate: {error}"),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("operator gate deadline expired");
}

pub(super) async fn transport_acceptance(
    live: &Live,
    key: &Path,
    gates: &Path,
    relay: &Arc<Mutex<RelayState>>,
    calls: &mut mpsc::Receiver<Detection>,
    recognized: &mut mpsc::Receiver<voice_input::RecognizedVoiceInput>,
) {
    let mut control = Control::connect(key, &live.server_id).await;
    let mut state = control.inspect().await;
    assert!(state.available && !state.active);
    assert_eq!(relay.lock().unwrap().sessions, 0);
    assert_eq!(live.tasks.active(), 0);
    assert_eq!(
        control
            .client
            .inspect(InspectCaptureRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let unsigned = CaptureRequest {
        device_boot_id: state.device_boot_id.clone(),
        expected_generation: state.generation,
    };
    assert_eq!(
        control
            .client
            .start(unsigned.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        control.client.stop(unsigned).await.unwrap_err().code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(control.inspect().await, state);
    mark(
        "system_transport_ready",
        json!({"device_boot_id":state.device_boot_id,"generation":state.generation,"unauthenticated_inspect_start_stop_rejected":true,"pcm_saved":false,"playback":false,"uart_initialization":false,"speech_required":false}),
    );
    quiet(&mut control, &state, relay, 2).await;
    let original_boot = state.device_boot_id.clone();
    let mut previous_session = None;
    for index in 0..3 {
        if index == 2 {
            mark(
                "system_restart_requested",
                json!({"operation":"ordinary Embedded process restart with same config and state; no initializer argument","device_boot_id":original_boot,"gate":"restart-complete","timeout_sec":60}),
            );
            wait_gate(
                gates,
                "restart-complete",
                relay,
                60,
                "system_automatic_restart_gate",
            )
            .await;
            control = Control::connect(key, &live.server_id).await;
            let restarted = control.inspect().await;
            assert!(restarted.available && !restarted.active);
            assert_ne!(restarted.device_boot_id, original_boot);
            for start in [true, false] {
                assert_eq!(
                    control.command(&state, start).await.unwrap_err().code(),
                    tonic::Code::FailedPrecondition,
                    "old process boot must not control the restarted runtime"
                );
            }
            assert_eq!(control.inspect().await, restarted);
            mark(
                "system_stale_boot_rejected",
                json!({"old_boot":original_boot,"new_boot":restarted.device_boot_id,"start":true,"stop":true}),
            );
            state = restarted;
            quiet(&mut control, &state, relay, 2).await;
        }
        assert_eq!(control.inspect().await, state);
        let previous = state.clone();
        state = control.command(&previous, true).await.unwrap();
        assert!(state.available && state.active && state.generation > previous.generation);
        assert_eq!(state.device_boot_id, previous.device_boot_id);
        assert_eq!(
            control.command(&previous, false).await.unwrap_err().code(),
            tonic::Code::FailedPrecondition,
            "stale Stop must not cancel the new session"
        );
        assert_eq!(control.inspect().await, state);
        let seconds = if index == 0 { 12 } else { 1 };
        let mut ready_at = None;
        let scope = tokio::time::timeout(Duration::from_secs(seconds + 10), async {
            loop {
                {
                    let observed = relay.lock().unwrap();
                    assert!(
                        observed.error.is_none(),
                        "relay failed: {:?}",
                        observed.error
                    );
                    assert!(
                        observed.sessions <= index + 1,
                        "unexpected additional session"
                    );
                    if observed.sessions == index + 1
                        && let Some(scope) = observed.ready
                    {
                        assert_eq!(scope.device_boot_id.to_string(), state.device_boot_id);
                        assert_ne!(Some(scope.session_id), previous_session);
                        assert_eq!(observed.ready_format, Some((16000, 1)));
                        assert_eq!(
                            live.tasks.active(),
                            1,
                            "production session ended during active capture"
                        );
                        let ready_at = ready_at.get_or_insert_with(tokio::time::Instant::now);
                        if ready_at.elapsed() >= Duration::from_secs(seconds)
                            && observed.stats.samples >= seconds * 16000
                        {
                            break scope;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("Start did not sustain authenticated PCM transport");
        previous_session = Some(scope.session_id);
        assert_eq!(control.inspect().await, state);
        mark(
            "system_transport_pcm",
            json!({"session_number":index+1,"device_boot_id":scope.device_boot_id,"session_id":scope.session_id,"observed_active_ms":ready_at.unwrap().elapsed().as_millis(),"minimum_seconds":seconds,"sample_rate_hz":16000,"channels":1,"forwarding_relay_capture":relay.lock().unwrap().stats,"server_tasks":live.tasks.active(),"speech_required":false,"acoustic_quality_verified":false}),
        );
        mark(
            "system_transport_stop_requested",
            json!({"session_number":index+1,"server_tasks":live.tasks.active(),"forwarding_streams":relay.lock().unwrap().forwarding_streams}),
        );
        let active = state.clone();
        relay.lock().unwrap().stopping = true;
        state = control.command(&active, false).await.unwrap();
        assert!(state.available && !state.active && state.generation > active.generation);
        assert_eq!(state.device_boot_id, active.device_boot_id);
        tokio::time::timeout(Duration::from_secs(5), async {
            while live.tasks.active() != 0 || relay.lock().unwrap().forwarding_streams != 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("Stop did not drain the original Server session and forwarding stream");
        while calls.try_recv().is_ok() {}
        assert!(
            recognized.try_recv().is_err(),
            "automatic transport test must not emit a transcript"
        );
        assert_eq!(
            control.command(&active, true).await.unwrap_err().code(),
            tonic::Code::FailedPrecondition,
            "stale Start must not reopen the stopped session"
        );
        assert_eq!(control.inspect().await, state);
        mark(
            "system_transport_stopped",
            json!({"session_number":index+1,"generation":state.generation,"server_tasks":live.tasks.active(),"forwarding_streams":relay.lock().unwrap().forwarding_streams,"stale_start_stop_rejected":true,"physical_microphone_power_off":false}),
        );
        quiet(&mut control, &state, relay, 2).await;
    }
    mark(
        "field_acceptance_complete",
        json!({"acceptance_scope":"system_control_transport_and_process_restart","sessions":relay.lock().unwrap().sessions,"ordinary_restart_new_boot":true,"stale_boot_start_stop_rejected":true,"pcm_saved":false,"playback":false,"uart_initialization":false,"speech_and_five_minute_silence_retested":false,"server_semantic_processing_verified":false}),
    );
}

async fn quiet(
    control: &mut Control,
    expected: &CaptureState,
    state: &Arc<Mutex<RelayState>>,
    seconds: u64,
) {
    let baseline = {
        let state = state.lock().unwrap();
        (state.sessions, state.stats.frames, state.stats.samples)
    };
    let started = tokio::time::Instant::now();
    for elapsed in 0..seconds {
        idle(&state.lock().unwrap(), baseline).unwrap();
        if elapsed % 30 == 0 {
            assert_eq!(control.inspect().await, *expected);
            mark(
                "system_idle_progress",
                json!({"elapsed_sec":elapsed,"required_sec":seconds,"sessions":baseline.0,"frames":baseline.1,"samples":baseline.2}),
            );
        }
        tokio::time::sleep_until(started + Duration::from_secs(elapsed + 1)).await;
    }
    idle(&state.lock().unwrap(), baseline).unwrap();
    assert_eq!(control.inspect().await, *expected);
    mark(
        "system_idle_complete",
        json!({"observed_ms":started.elapsed().as_millis(),"required_sec":seconds,"sessions":baseline.0,"frames":baseline.1,"samples":baseline.2,"active":false}),
    );
}

pub(super) async fn acceptance(
    live: &Live,
    key: &Path,
    gates: &Path,
    relay: &Arc<Mutex<RelayState>>,
    calls: &mut mpsc::Receiver<Detection>,
    recognized: &mut mpsc::Receiver<voice_input::RecognizedVoiceInput>,
) {
    let mut control = Control::connect(key, &live.server_id).await;
    let mut state = control.inspect().await;
    assert!(state.available && !state.active);
    assert_eq!(relay.lock().unwrap().sessions, 0);
    assert_eq!(live.tasks.active(), 0);
    mark(
        "system_ready",
        json!({"device_boot_id":state.device_boot_id,"generation":state.generation,"pcm_saved":false,"playback":false,"turn_model":"fixed_fixture","silence_mode":"stopped_session_same_capture_process","silence_sec":300}),
    );
    quiet(&mut control, &state, relay, 5).await;
    let original_boot = state.device_boot_id.clone();
    let mut current_boot = original_boot.clone();
    let mut previous_session = None;
    for (index, name) in GATES.iter().enumerate() {
        if index == 2 {
            mark(
                "system_restart_requested",
                json!({"operation":"ordinary Embedded process restart with same config and state; no initializer argument","device_boot_id":original_boot}),
            );
        }
        gate(gates, name, relay).await;
        if index == 2 {
            control = Control::connect(key, &live.server_id).await;
            let restarted = control.inspect().await;
            assert!(restarted.available && !restarted.active);
            assert_ne!(restarted.device_boot_id, original_boot);
            assert_eq!(
                control.command(&state, true).await.unwrap_err().code(),
                tonic::Code::FailedPrecondition
            );
            assert_eq!(control.inspect().await, restarted);
            quiet(&mut control, &restarted, relay, 5).await;
            current_boot = restarted.device_boot_id.clone();
            state = restarted;
        }
        assert_eq!(control.inspect().await, state);
        let previous = state.clone();
        state = control.command(&previous, true).await.unwrap();
        assert!(state.available && state.active && state.generation > previous.generation);
        assert_eq!(state.device_boot_id, current_boot);
        assert_eq!(
            control.command(&previous, false).await.unwrap_err().code(),
            tonic::Code::FailedPrecondition,
            "stale Stop must not cancel the new session"
        );
        assert_eq!(control.inspect().await, state);
        let scope = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                {
                    let state = relay.lock().unwrap();
                    assert!(state.error.is_none(), "relay failed: {:?}", state.error);
                    assert!(state.sessions <= index + 1, "unexpected additional session");
                    if state.sessions == index + 1
                        && let Some(scope) = state.ready
                    {
                        assert_eq!(scope.device_boot_id.to_string(), current_boot);
                        assert_ne!(Some(scope.session_id), previous_session);
                        break scope;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("Start did not reach authenticated Server Ready");
        previous_session = Some(scope.session_id);
        mark(
            match index {
                0 => "system_speak_before_silence",
                1 => "system_speak_after_silence",
                _ => "system_speak_after_restart",
            },
            json!({"session_number":index+1,"wakeword_required":false,"speech_window_sec":120,"instruction":"ordinary short speech, then remain quiet"}),
        );
        let detected = tokio::time::timeout(Duration::from_secs(120), calls.recv())
            .await
            .expect("ordinary speech endpoint deadline expired")
            .expect("turn detector owner ended");
        let stats = relay.lock().unwrap().stats.clone();
        assert!(detected.window_ms > 0 && stats.speech_samples > 0 && stats.peak > 0);
        mark(
            "system_speech_endpoint",
            json!({"session_number":index+1,"window_ms":detected.window_ms,"capture":stats}),
        );
        detected.answer(TurnState::Complete, "system capture field fixture");
        let input = tokio::time::timeout(Duration::from_secs(5), recognized.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(input.session_id, scope.session_id);
        let active = state.clone();
        relay.lock().unwrap().stopping = true;
        state = control.command(&active, false).await.unwrap();
        assert!(state.available && !state.active && state.generation > active.generation);
        assert_eq!(state.device_boot_id, current_boot);
        tokio::time::timeout(Duration::from_secs(5), async {
            while live.tasks.active() != 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("Stop did not drain the original Server session");
        assert!(input.cancel.is_cancelled());
        drop(input);
        while calls.try_recv().is_ok() {}
        assert!(recognized.try_recv().is_err());
        assert_eq!(
            control.command(&active, true).await.unwrap_err().code(),
            tonic::Code::FailedPrecondition
        );
        mark(
            "system_stopped",
            json!({"session_number":index+1,"generation":state.generation,"server_tasks":live.tasks.active(),"capture":relay.lock().unwrap().stats,"stale_start_rejected":true,"physical_microphone_power_off":false}),
        );
        if index == 0 {
            mark(
                "system_silence_started",
                json!({"seconds":300,"active":false,"same_embedded_boot":true,"instruction":"remain quiet; do not say the wakeword"}),
            );
            quiet(&mut control, &state, relay, 300).await;
        } else {
            quiet(&mut control, &state, relay, 5).await;
        }
    }
    mark(
        "field_acceptance_complete",
        json!({"acceptance_scope":"system_start_stop_and_silent_recovery","sessions":relay.lock().unwrap().sessions,"silence_sec":300,"silence_mode":"stopped_session_same_capture_process","ordinary_restart_new_boot":true,"stale_boot_start_rejected":true,"pcm_saved":false,"playback":false,"uart_initialization_evidence":"verify separately from Embedded claim and logs"}),
    );
}

#[test]
fn idle_evidence_rejects_new_sessions_late_pcm_and_relay_failure() {
    let mut state = RelayState::default();
    assert!(idle(&state, (0, 0, 0)).is_ok());
    state.sessions = 1;
    assert!(idle(&state, (0, 0, 0)).is_err());
    state.stats.frames = 1;
    state.stats.samples = 320;
    assert!(idle(&state, (1, 0, 0)).is_err());
    assert!(idle(&state, (1, 1, 320)).is_ok());
    state.error = Some("capture_transport_error");
    assert!(idle(&state, (1, 1, 320)).is_err());
}

#[test]
fn only_explicit_stop_allows_cancelled_transport_status() {
    let mut state = RelayState::default();
    state.transport_error("capture_transport_error", &Status::cancelled("peer ended"));
    assert_eq!(state.error, Some("capture_transport_error"));
    let mut state = RelayState {
        stopping: true,
        ..Default::default()
    };
    state.transport_error("capture_transport_error", &Status::cancelled("peer ended"));
    assert_eq!(state.error, None);
    state.transport_error(
        "capture_transport_error",
        &Status::unavailable("transport lost"),
    );
    assert_eq!(state.error, Some("capture_transport_error"));
}
