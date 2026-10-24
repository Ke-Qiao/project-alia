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

    let second = command(&control, &binding, &stopped, true).await.unwrap();
    let Seen::Opened {
        session: next,
        output,
        ..
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
    output
        .send(Err(Status::unavailable("test disconnect")))
        .await
        .unwrap();
    inactive(&control).await;
    reached(&local.wake_resets, 2).await;
    publish(&audio, 99, 640);
    reached(&local.captured, 3).await;
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
    assert_eq!(
        client
            .start(authorized(stale_boot))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::FailedPrecondition
    );
    let active = client
        .start(authorized(request(&idle)))
        .await
        .unwrap()
        .into_inner();
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
    let stopped = client
        .stop(authorized(request(&active)))
        .await
        .unwrap()
        .into_inner();
    assert!(!stopped.active);
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
