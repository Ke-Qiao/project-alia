use super::*;
use crate::module::speech::CaptureControl;
use alia_proto::capture::capture_control_client::CaptureControlClient;
use alia_proto::capture::capture_control_server::CaptureControlServer;
use alia_proto::capture::{CaptureRequest, CaptureState, InspectCaptureRequest};

fn request(state: &CaptureState) -> CaptureRequest {
    CaptureRequest {
        device_boot_id: state.device_boot_id.clone(),
        expected_generation: state.generation,
    }
}

async fn command(
    control: &CaptureControl,
    binding: &BindingManager,
    state: &CaptureState,
    start: bool,
) -> Result<CaptureState> {
    tokio::time::timeout(
        Duration::from_secs(3),
        control.enqueue(request(state), start, binding.status().await, String::new())?,
    )
    .await??
}

async fn inactive(control: &CaptureControl) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.inspect().unwrap().active {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn system_start_stop_is_explicit_scoped_and_waits_for_original_native_writes() {
    let Harness {
        mut runtime,
        audio,
        events,
        local,
        mut seen,
        shutdown,
        server,
        _directory,
    } = harness(8).await;
    runtime.config.trigger = SpeechTrigger::System;
    let control = runtime.control.controller.clone();
    let binding = runtime.binding.clone();
    let cancel = runtime.cancel.clone();
    let playback = runtime.playback_session.clone();
    let mut lifecycle = events.subscribe();
    let task = tokio::spawn(runtime.run());
    publish(&audio, 99, 0);
    reached(&local.captured, 1).await;
    assert!(seen.try_recv().is_err(), "system idle uploaded PCM");
    assert_eq!(local.wake_checks.load(Ordering::SeqCst), 0);
    let idle = control.inspect().unwrap();
    let first = command(&control, &binding, &idle, true).await.unwrap();
    assert!(first.active && first.generation > idle.generation);
    let Seen::Opened {
        boot,
        session,
        output,
    } = received(&mut seen).await
    else {
        panic!("missing system Open");
    };
    publish(&audio, 1, 320);
    let Seen::Audio(frame) = received(&mut seen).await else {
        panic!("missing system audio");
    };
    assert_eq!(
        (frame.sequence, frame.sample_offset, frame.pcm_s16le.len()),
        (0, 0, 640)
    );
    assert!(
        !std::iter::from_fn(|| lifecycle.try_recv().ok())
            .any(|event| matches!(event, ModuleEvent::WakeWordDetected(_)))
    );
    assert!(command(&control, &binding, &idle, false).await.is_err());
    let scope = VoiceScope {
        device_boot_id: Uuid::parse_str(&boot).unwrap(),
        session_id: Uuid::parse_str(&session).unwrap(),
        generation_epoch: 2,
    };
    let lease = playback
        .begin(&scope, &Uuid::new_v4().to_string(), 1, 24000)
        .unwrap();
    playback.enqueue(&lease, &[1, 2, 3]).unwrap();
    let native_write = lease.submission.begin_output().unwrap();
    let stop = command(&control, &binding, &first, false);
    tokio::pin!(stop);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut stop)
            .await
            .is_err(),
        "Stop acknowledged an entered native write"
    );
    drop(native_write);
    let stopped = stop.await.unwrap();
    assert!(!stopped.active && stopped.generation > first.generation);
    tokio::time::timeout(Duration::from_secs(3), output.closed())
        .await
        .unwrap();
    assert!(lease.submission.output_quiescent() && !lease.submission.is_live());
    assert_eq!(local.playback_starts.load(Ordering::SeqCst), 0);
    local.playback_unavailable.store(true, Ordering::SeqCst);

    let second = command(&control, &binding, &stopped, true).await.unwrap();
    let Seen::Opened {
        boot,
        session: next,
        output,
    } = received(&mut seen).await
    else {
        panic!("missing fresh Open");
    };
    assert_ne!(session, next);
    assert!(
        command(&control, &binding, &first, false).await.is_err(),
        "old Stop ended replacement"
    );
    assert_eq!(control.inspect().unwrap(), second);
    publish(&audio, 1, 640);
    assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
    let next_binding = alia_core::output_control::OutputBinding {
        scope: VoiceScope {
            device_boot_id: Uuid::parse_str(&boot).unwrap(),
            session_id: Uuid::parse_str(&next).unwrap(),
            generation_epoch: 1,
        },
        response_id: Uuid::new_v4(),
        context_id: Uuid::new_v4(),
        server_instance_id: Uuid::new_v4(),
        initiative_capture: None,
    };
    playback.register_output(&next_binding).unwrap();
    let next_lease = playback
        .begin_bound(&next_binding, &Uuid::new_v4().to_string(), 1, 24000)
        .unwrap();
    assert!(playback.enqueue(&next_lease, &[4, 5, 6]).unwrap());
    assert!(!playback.enqueue(&lease, &[7]).unwrap());
    assert_eq!(local.playback_starts.load(Ordering::SeqCst), 1);
    output
        .send(Err(Status::unavailable("test disconnect")))
        .await
        .unwrap();
    inactive(&control).await;
    reached(&local.wake_resets, 2).await;
    publish(&audio, 99, 960);
    reached(&local.captured, 4).await;
    assert!(
        seen.try_recv().is_err(),
        "disconnect automatically reopened system capture"
    );
    let recovered = command(&control, &binding, &control.inspect().unwrap(), true)
        .await
        .unwrap();
    let Seen::Opened { output, .. } = received(&mut seen).await else {
        panic!("fresh Start must recover after disconnect");
    };
    command(&control, &binding, &recovered, false)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), output.closed())
        .await
        .unwrap();
    cancel.cancel();
    task.await.unwrap();
    assert!(!control.inspect().unwrap().available);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn cancelled_or_rebound_queued_start_cannot_open_a_session() {
    for replaced in [false, true] {
        let Harness {
            mut runtime,
            audio,
            events: _,
            local,
            mut seen,
            shutdown,
            server,
            _directory,
        } = harness(8).await;
        runtime.config.trigger = SpeechTrigger::System;
        let control = runtime.control.controller.clone();
        let binding = runtime.binding.clone();
        let cancel = runtime.cancel.clone();
        let state = control.inspect().unwrap();
        let result = control
            .enqueue(request(&state), true, binding.status().await, String::new())
            .unwrap();
        let pending = if replaced {
            let original = binding.status().await;
            binding
                .seed_dev_binding("test", "test", original.server_endpoint.as_ref().unwrap())
                .await
                .unwrap();
            Some(result)
        } else {
            drop(result);
            None
        };
        let task = tokio::spawn(runtime.run());
        if let Some(result) = pending {
            assert!(result.await.unwrap().is_err());
        }
        publish(&audio, 99, 0);
        reached(&local.captured, 1).await;
        assert_eq!(control.inspect().unwrap(), state);
        assert!(seen.try_recv().is_err());
        cancel.cancel();
        task.await.unwrap();
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    }
}

fn authorized<T>(message: T) -> Request<T> {
    let mut request = Request::new(message);
    request
        .metadata_mut()
        .insert("authorization", "Bearer dev-test".parse().unwrap());
    request
}

#[tokio::test]
async fn capture_rpc_checks_auth_boot_generation_and_joins_stop() {
    let Harness {
        mut runtime,
        audio,
        events: _,
        local,
        mut seen,
        shutdown,
        server,
        _directory,
    } = harness(8).await;
    runtime.config.trigger = SpeechTrigger::System;
    let control = runtime.control.controller.clone();
    let binding = runtime.binding.clone();
    let cancel = runtime.cancel.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let rpc_cancel = CancellationToken::new();
    let rpc_stop = rpc_cancel.clone();
    let service = crate::grpc::CaptureService::new(control, binding.clone());
    let rpc = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(CaptureControlServer::new(service))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                rpc_stop.cancelled(),
            )
            .await
            .unwrap();
    });
    let mut client = CaptureControlClient::connect(endpoint).await.unwrap();
    let task = tokio::spawn(runtime.run());
    assert_eq!(
        client
            .inspect(InspectCaptureRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let idle = client
        .inspect(authorized(InspectCaptureRequest {}))
        .await
        .unwrap()
        .into_inner();
    let mut stale_boot = request(&idle);
    stale_boot.device_boot_id = Uuid::new_v4().to_string();
    local.playback_unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        client
            .start(authorized(stale_boot))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::FailedPrecondition
    );
    assert_eq!(local.playback_starts.load(Ordering::SeqCst), 0);
    local.playback_start_fails.store(true, Ordering::SeqCst);
    let unavailable = client.start(authorized(request(&idle))).await.unwrap_err();
    assert_eq!(unavailable.code(), tonic::Code::FailedPrecondition);
    assert_eq!(unavailable.message(), "test playback start failed");
    assert_eq!(
        client
            .inspect(authorized(InspectCaptureRequest {}))
            .await
            .unwrap()
            .into_inner(),
        idle,
        "unavailable playback advanced capture admission"
    );
    assert!(seen.try_recv().is_err(), "rejected Start opened a session");
    assert_eq!(local.generation.load(Ordering::SeqCst), 0);
    assert_eq!(local.playback_starts.load(Ordering::SeqCst), 1);
    assert_eq!(local.captured.load(Ordering::SeqCst), 0);
    local.playback_start_fails.store(false, Ordering::SeqCst);
    let active = client
        .start(authorized(request(&idle)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(local.playback_starts.load(Ordering::SeqCst), 2);
    let Seen::Opened { output, .. } = received(&mut seen).await else {
        panic!("missing Open");
    };
    publish(&audio, 1, 0);
    assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
    assert_eq!(
        client
            .start(authorized(request(&idle)))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::FailedPrecondition
    );
    local.playback_unavailable.store(true, Ordering::SeqCst);
    let stopped = client
        .stop(authorized(request(&active)))
        .await
        .unwrap()
        .into_inner();
    assert!(!stopped.active);
    assert_eq!(local.playback_starts.load(Ordering::SeqCst), 2);
    tokio::time::timeout(Duration::from_secs(3), output.closed())
        .await
        .unwrap();
    binding.unbind("test").await.unwrap();
    assert_eq!(
        client
            .inspect(authorized(InspectCaptureRequest {}))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        client
            .start(authorized(request(&stopped)))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    cancel.cancel();
    task.await.unwrap();
    rpc_cancel.cancel();
    rpc.await.unwrap();
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn playback_recovery_rechecks_cancel_binding_and_capture_before_session_admission() {
    for change in ["caller", "binding", "shutdown", "capture", "stale_stop"] {
        let Harness {
            mut runtime,
            audio: _audio,
            local,
            mut seen,
            shutdown,
            server,
            _directory,
            ..
        } = harness(8).await;
        runtime.config.trigger = SpeechTrigger::System;
        let capture_stopped = CancellationToken::new();
        runtime.capture_stopped = Some(capture_stopped.clone());
        let control = runtime.control.controller.clone();
        let binding = runtime.binding.clone();
        let cancel = runtime.cancel.clone();
        let playback = runtime.playback_session.clone();
        let idle = control.inspect().unwrap();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        *local.playback_start_gate.lock().unwrap() = Some(gate.clone());
        local.playback_unavailable.store(true, Ordering::SeqCst);
        let mut pending = Some(
            control
                .enqueue(request(&idle), true, binding.status().await, String::new())
                .unwrap(),
        );
        let task = tokio::spawn(runtime.run());
        reached(&local.playback_starts, 1).await;
        assert_eq!(control.inspect().unwrap(), idle);
        assert!(seen.try_recv().is_err());
        let mut stale_stop = None;
        match change {
            "caller" => drop(pending.take()),
            "binding" => {
                let original = binding.status().await;
                binding
                    .seed_dev_binding("test", "test", original.server_endpoint.as_ref().unwrap())
                    .await
                    .unwrap();
            }
            "shutdown" => cancel.cancel(),
            "capture" => capture_stopped.cancel(),
            "stale_stop" => {
                stale_stop = Some(
                    control
                        .enqueue(request(&idle), false, binding.status().await, String::new())
                        .unwrap(),
                );
            }
            _ => unreachable!(),
        }
        assert!(!task.is_finished(), "native recovery was abandoned");
        gate.add_permits(1);
        if change == "stale_stop" {
            let active = tokio::time::timeout(Duration::from_secs(3), pending.take().unwrap())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(active.active && active.generation > idle.generation);
            assert!(
                tokio::time::timeout(Duration::from_secs(3), stale_stop.unwrap())
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err(),
                "old Stop retired the recovered generation"
            );
            assert_eq!(control.inspect().unwrap(), active);
            let Seen::Opened { output, .. } = received(&mut seen).await else {
                panic!("recovered Start did not open a session");
            };
            command(&control, &binding, &active, false).await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), output.closed())
                .await
                .unwrap();
        } else if let Some(pending) = pending {
            assert!(
                tokio::time::timeout(Duration::from_secs(3), pending)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
            if change == "binding" {
                assert_eq!(control.inspect().unwrap(), idle);
            }
        } else {
            assert!(command(&control, &binding, &idle, false).await.is_err());
            assert_eq!(control.inspect().unwrap(), idle);
        }
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert!(!control.inspect().unwrap().active);
        assert_eq!(playback.active_scope().unwrap(), None);
        assert!(seen.try_recv().is_err());
        assert_eq!(local.playback_starts.load(Ordering::SeqCst), 1);
        assert_eq!(local.captured.load(Ordering::SeqCst), 0);
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn playback_recovery_requires_retired_settled_owners_and_never_revives_old_output() {
    let local = Arc::new(LocalAudio::default());
    let playback = PlaybackSession::new(local.clone());
    let scope = VoiceScope {
        device_boot_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        generation_epoch: 1,
    };
    playback.activate(&scope).unwrap();
    let lease = playback
        .begin(&scope, &Uuid::new_v4().to_string(), 1, 16000)
        .unwrap();
    assert!(playback.enqueue(&lease, &[1, 2, 3]).unwrap());
    let entered = lease.submission.begin_output().unwrap();
    let unknown = alia_core::delivery::ObservationEnd::Unknown {
        reason: "old native device failed".into(),
    };
    lease.submission.terminate(unknown.clone());
    local.playback_unavailable.store(true, Ordering::SeqCst);
    assert!(playback.recover_backend_for_capture_start().await.is_err());
    playback.close(&scope).unwrap();
    assert!(playback.recover_backend_for_capture_start().await.is_err());
    assert_eq!(local.playback_starts.load(Ordering::SeqCst), 0);
    drop(entered);
    playback.recover_backend_for_capture_start().await.unwrap();
    assert_eq!(local.playback_starts.load(Ordering::SeqCst), 1);
    assert!(!lease.submission.is_live());
    assert_eq!(lease.submission.snapshot(1).observation_end, Some(unknown));
    assert!(!playback.enqueue(&lease, &[4]).unwrap());
    assert!(playback.activate(&scope).is_err());
    playback.shutdown_token().cancel();
    local.playback_unavailable.store(true, Ordering::SeqCst);
    assert!(playback.recover_backend_for_capture_start().await.is_err());
    assert_eq!(local.playback_starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn playback_shutdown_joins_recovery_after_its_waiter_is_dropped() {
    let local = Arc::new(LocalAudio::default());
    let playback = Arc::new(PlaybackSession::new(local.clone()));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *local.playback_start_gate.lock().unwrap() = Some(gate.clone());
    local.playback_unavailable.store(true, Ordering::SeqCst);
    let pending = {
        let playback = playback.clone();
        tokio::spawn(async move { playback.recover_backend_for_capture_start().await })
    };
    reached(&local.playback_starts, 1).await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    let stop = playback.stop();
    tokio::pin!(stop);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut stop)
            .await
            .is_err(),
        "shutdown abandoned a native startup still in progress"
    );
    gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(3), stop)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(local.playback_starts.load(Ordering::SeqCst), 1);
    assert_eq!(playback.active_scope().unwrap(), None);
}

#[tokio::test]
async fn wakeword_mode_rejects_system_start_without_changing_capture() {
    let Harness {
        runtime,
        audio: _audio,
        events: _,
        local: _,
        mut seen,
        shutdown,
        server,
        _directory,
    } = harness(8).await;
    let control = runtime.control.controller.clone();
    let binding = runtime.binding.clone();
    let cancel = runtime.cancel.clone();
    let idle = control.inspect().unwrap();
    let task = tokio::spawn(runtime.run());
    assert!(
        command(&control, &binding, &idle, true)
            .await
            .unwrap_err()
            .to_string()
            .contains("speech.trigger")
    );
    assert_eq!(control.inspect().unwrap(), idle);
    assert!(seen.try_recv().is_err());
    cancel.cancel();
    task.await.unwrap();
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn signed_capture_rpc_rejects_wrong_scope_and_foreign_signer_before_session_admission() {
    use alia_proto::capture::capture_control_server::CaptureControl as _;
    let Harness {
        mut runtime,
        audio,
        events: _,
        local: _,
        mut seen,
        shutdown,
        server,
        _directory,
    } = harness(8).await;
    runtime.config.trigger = SpeechTrigger::System;
    let control = runtime.control.controller.clone();
    let binding = runtime.binding.clone();
    let cancel = runtime.cancel.clone();
    let endpoint = binding.status().await.server_endpoint.unwrap();
    binding.unbind("test").await.unwrap();
    super::binding_admission::signed_bind(&binding, "server", &endpoint).await;
    let service = crate::grpc::CaptureService::new(control.clone(), binding.clone());
    let key = ed25519_dalek::SigningKey::from_bytes(&[61; 32]);
    let foreign = ed25519_dalek::SigningKey::from_bytes(&[62; 32]);
    let signed = |key: &ed25519_dalek::SigningKey, scope: &str, message| {
        let token = alia_transport::mint_auth_token(
            key,
            "server",
            scope,
            alia_transport::DEFAULT_GRPC_AUTH_TTL,
        )
        .unwrap();
        let mut request = Request::new(message);
        request
            .metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        request
    };
    let idle = control.inspect().unwrap();
    let task = tokio::spawn(runtime.run());
    for (key, scope) in [
        (&key, EMBEDDED_INGRESS_SCOPE),
        (&foreign, alia_transport::EMBEDDED_CONTROL_SCOPE),
    ] {
        let error = service
            .start(signed(key, scope, request(&idle)))
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unauthenticated);
        assert_eq!(control.inspect().unwrap(), idle);
        assert!(seen.try_recv().is_err());
    }
    let active = service
        .start(signed(
            &key,
            alia_transport::EMBEDDED_CONTROL_SCOPE,
            request(&idle),
        ))
        .await
        .unwrap()
        .into_inner();
    let Seen::Opened { output, .. } = received(&mut seen).await else {
        panic!("missing signed Open");
    };
    publish(&audio, 1, 0);
    assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
    let stopped = service
        .stop(signed(
            &key,
            alia_transport::EMBEDDED_CONTROL_SCOPE,
            request(&active),
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(!stopped.active);
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
async fn native_timeout_after_stop_or_disconnect_rejects_new_capture() {
    for disconnected in [false, true] {
        let Harness {
            mut runtime,
            audio,
            events: _,
            local: _,
            mut seen,
            shutdown,
            server,
            _directory,
        } = harness(8).await;
        runtime.config.trigger = SpeechTrigger::System;
        let control = runtime.control.controller.clone();
        let binding = runtime.binding.clone();
        let cancel = runtime.cancel.clone();
        let playback = runtime.playback_session.clone();
        let task = tokio::spawn(runtime.run());
        let active = command(&control, &binding, &control.inspect().unwrap(), true)
            .await
            .unwrap();
        let Seen::Opened {
            boot,
            session,
            output,
        } = received(&mut seen).await
        else {
            panic!("missing Open");
        };
        publish(&audio, 1, 0);
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
        let scope = VoiceScope {
            device_boot_id: Uuid::parse_str(&boot).unwrap(),
            session_id: Uuid::parse_str(&session).unwrap(),
            generation_epoch: 2,
        };
        let lease = playback
            .begin(&scope, &Uuid::new_v4().to_string(), 1, 24000)
            .unwrap();
        playback.enqueue(&lease, &[1, 2, 3]).unwrap();
        let native_write = lease.submission.begin_output().unwrap();
        if disconnected {
            output
                .send(Err(Status::unavailable(
                    "test disconnect with native write",
                )))
                .await
                .unwrap();
            inactive(&control).await;
        } else {
            let error = command(&control, &binding, &active, false)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("did not settle"));
        }
        let failed = control.inspect().unwrap();
        assert!(!failed.active && !failed.available);
        assert!(command(&control, &binding, &failed, true).await.is_err());
        assert!(!lease.submission.output_quiescent());
        drop(native_write);
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
}
