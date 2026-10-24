use super::*;
use alia_embedded::{config::SpeechTrigger, grpc::CaptureService, module::noop::NoopWakeWord};
use alia_proto::capture::{
    CaptureRequest, CaptureState, InspectCaptureRequest,
    capture_control_client::CaptureControlClient, capture_control_server::CaptureControlServer,
};

fn signed<T>(key: &ed25519_dalek::SigningKey, message: T) -> Request<T> {
    let token = alia_transport::mint_auth_token(
        key,
        "server",
        alia_transport::EMBEDDED_CONTROL_SCOPE,
        alia_transport::DEFAULT_GRPC_AUTH_TTL,
    )
    .unwrap();
    let mut request = Request::new(message);
    request
        .metadata_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

fn request(state: &CaptureState) -> CaptureRequest {
    CaptureRequest {
        device_boot_id: state.device_boot_id.clone(),
        expected_generation: state.generation,
    }
}

#[tokio::test]
async fn signed_system_start_stop_and_disconnect_use_real_server_ownership() {
    with_db_context(SpeechFixture::default(), |f, owner| Box::pin(async move {
        owner.stage = "initialize signed system capture and real server pair";
        Runtime::initialize(&mut owner.paired, f, 4096).await;
        let paired = owner.paired.as_ref().unwrap();
        let mut s = paired.scene.clone();
        s.session.revoke_all().unwrap();
        let channels = Arc::new(ChannelRegistry::new());
        channels.register(s.channel.clone()).await.unwrap();
        s.channel.binding_client().await.unwrap();
        let (input, mut recognized) = voice_input::channel();
        let (detector, mut detections) = mpsc::channel(2);
        let wire = VoiceWire {
            inner: Arc::new(VoiceSessionService::new(
                VoiceIngressAuth::new(channels), Some(Arc::new(Detector(detector))), input,
                s.service.clone(), Duration::from_secs(30), owner.stop.clone(), owner.tasks.clone(),
            )),
            disconnect: Arc::default(), events: Arc::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let endpoint = format!("http://127.0.0.1:{}", address.port() - 1);
        let binding = paired.binding.clone();
        let key = alia_server::auth::ensure_server_keypair(&paired._directory.0.join("server.key")).unwrap();
        binding.unbind("server").await.unwrap();
        let issued = alia_transport::current_unix_timestamp().unwrap();
        let public = key.public_key_bytes();
        let payload = alia_transport::bind_payload("server", "server", &endpoint, &public, issued);
        binding.bind("server", "server", &endpoint, &public, &alia_transport::sign_payload(key.signing_key(), &payload), issued).await.unwrap();
        assert!(!binding.status().await.dev_skip_auth);
        let identity = IdentityConfig { device_id: "companion-paired-device".into(), device_name: "system capture fixture".into(), keypair_path: paired._directory.0.join("device.key") };
        std::fs::write(&identity.keypair_path, [19u8; 32]).unwrap();
        alia_embedded::identity::load_device_keypair(&identity.keypair_path).unwrap();
        let bus = AudioBus::new(32);
        let consumed = Arc::new(Semaphore::new(0));
        let events = DeviceEvents::new(32);
        let mut lifecycle = events.subscribe();
        owner.speech = Some(SpeechPipeline::new(
            bus.clone(), events, Arc::new(NoopWakeWord), Arc::new(Vad(consumed.clone())),
            Arc::new(NoopDirectResponse), s.session.clone(), Uuid::new_v4(), binding.clone(),
            &SpeechConfig { trigger: SpeechTrigger::System, channel_id: "test-device".into(), pre_roll_ms: 300, upload_queue_frames: 32, ..Default::default() },
            &identity, owner.stop.clone(),
        ));
        let controller = owner.speech.as_ref().unwrap().capture_control();
        let capture_service = CaptureService::new(controller.clone(), binding);
        let stop = owner.stop.clone();
        let service = wire.clone();
        // Co-host the two real services in the test; playback retains its own signed endpoint.
        owner.voice = Some(RpcServer { stop: stop.clone(), task: Some(tokio::spawn(async move {
            let incoming = async_stream::stream! { loop { yield listener.accept().await.map(|(stream, _)| stream); } };
            tonic::transport::Server::builder()
                .add_service(VoiceSessionServer::new(service))
                .add_service(CaptureControlServer::new(capture_service))
                .serve_with_incoming_shutdown(incoming, stop.cancelled()).await.unwrap();
        })) });
        owner.speech.as_ref().unwrap().start().await.unwrap();
        let mut client = CaptureControlClient::connect(format!("http://{address}")).await.unwrap();
        let mut capture = Capture { bus, consumed, offset: 0 };

        owner.stage = "system idle consumes local PCM without opening a server session";
        capture.utterance(999).await;
        assert!(wire.events.lock().unwrap().is_empty());
        assert!(wire.disconnect.lock().unwrap().is_none());
        assert!(detections.try_recv().is_err());
        assert!(s.session.active_scope().unwrap().is_none());
        assert_eq!(client.inspect(InspectCaptureRequest {}).await.unwrap_err().code(), tonic::Code::Unauthenticated);
        let mut idle = client.inspect(signed(key.signing_key(), InspectCaptureRequest {})).await.unwrap().into_inner();
        assert!(!idle.active && idle.available);
        let mut old_active = None;
        let mut previous_session = None;

        for cycle in 0..3 {
            owner.stage = "authenticated Start creates a fresh real Server Ready";
            let active = client.start(signed(key.signing_key(), request(&idle))).await.unwrap().into_inner();
            assert!(active.active && active.available && active.generation > idle.generation);
            assert_eq!(active.device_boot_id, idle.device_boot_id);
            assert_eq!(client.start(signed(key.signing_key(), request(&idle))).await.unwrap_err().code(), tonic::Code::FailedPrecondition);
            if let Some(old_active) = &old_active {
                assert_eq!(client.stop(signed(key.signing_key(), request(old_active))).await.unwrap_err().code(), tonic::Code::FailedPrecondition);
                assert_eq!(controller.inspect().unwrap(), active, "stale Stop replaced the current owner");
            }
            eventually(|| async { s.session.active_scope().unwrap().is_some() }).await;
            let ready = s.session.active_scope().unwrap().unwrap();
            assert_eq!(ready.device_boot_id.to_string(), active.device_boot_id);
            assert_eq!(ready.generation_epoch, 1);
            assert_ne!(Some(ready.session_id), previous_session);
            previous_session = Some(ready.session_id);
            assert_eq!(wire.events.lock().unwrap().iter().filter(|event| matches!(event.payload, Some(voice::session_event::Payload::Ready(_)))).count(), cycle + 1);

            owner.stage = "exact post-Start PCM reaches the production Server detector";
            let value = 10 + cycle as i16;
            capture.utterance(value).await;
            let detected = receive(&mut detections).await;
            let pcm: Vec<_> = detected.pcm.as_bytes().chunks_exact(2).map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]])).collect();
            let mut expected = vec![value; 1600];
            expected.extend_from_slice(&[0; 11200]);
            assert_eq!(pcm, expected, "idle/pre-Start PCM leaked into the new session");
            detected.answer(TurnState::Complete, "system capture fixture response");
            let input = receive(&mut recognized).await;
            assert_eq!(input.session_id, ready.session_id);
            assert_eq!(input.device_boot_id, ready.device_boot_id);
            assert_eq!(input.user_id, s.f.user.to_string());
            assert_eq!(input.channel_id, "test-device");
            s.scope = input.output.as_ref().unwrap().producer.scope();
            eventually(|| async { s.session.active_scope().unwrap() == Some(s.scope) }).await;
            let response = bind_response(&s.f, &s.service, &input).await;
            send_media(owner, &s, &response, &[17; 1600]).await;
            let original = s.device.submissions.lock().unwrap().values().find(|submission| submission.identity.scope == s.scope).unwrap().clone();
            s.device.submit_frames(&original.identity.playback_id.to_string(), 40);
            assert_eq!(original.snapshot(0).submitted_source_frames, Some(40));
            assert_eq!(original.snapshot(0).presented_source_frames, None);

            if cycle == 1 {
                owner.stage = "network loss retires the original output and requires explicit Start";
                wire.disconnect.lock().unwrap().as_ref().unwrap().cancel();
                eventually(|| async { !controller.inspect().unwrap().active }).await;
                idle = client.inspect(signed(key.signing_key(), InspectCaptureRequest {})).await.unwrap().into_inner();
            } else {
                owner.stage = "authenticated Stop waits for entered native output";
                let native_write = original.begin_output().unwrap();
                let stop = client.stop(signed(key.signing_key(), request(&active)));
                tokio::pin!(stop);
                assert!(tokio::time::timeout(Duration::from_millis(25), &mut stop).await.is_err(), "Stop acknowledged before the entered native write settled");
                drop(native_write);
                idle = tokio::time::timeout(Duration::from_secs(3), stop).await.unwrap().unwrap().into_inner();
            }
            assert!(!idle.active && idle.available && idle.generation > active.generation);
            eventually(|| async { input.cancel.is_cancelled() && s.session.active_scope().unwrap().is_none() && owner.tasks.active() == 0 }).await;
            assert!(original.output_quiescent() && !original.is_live());
            assert!(s.device.state.lock().unwrap().1.is_empty());
            assert!(!s.device.submit_audio(&original, 1600, &[99]).unwrap());
            assert_eq!(owner.deliveries.pop().unwrap().await.unwrap().unwrap().status, DeliveryStatus::Cancelled);
            let mut late = s.audio(&response, &[99; 16]).await;
            late.output_key = "late-after-retirement".into();
            let result = s.service.deliver_audio(late).await;
            assert!(result.is_err() || result.is_ok_and(|record| record.status == DeliveryStatus::Cancelled));

            owner.stage = "retired session remains idle while local PCM continues";
            let event_count = wire.events.lock().unwrap().len();
            capture.utterance(999).await;
            assert_eq!(controller.inspect().unwrap(), idle);
            assert_eq!(wire.events.lock().unwrap().len(), event_count);
            assert!(detections.try_recv().is_err());
            assert!(recognized.try_recv().is_err());
            assert!(s.session.active_scope().unwrap().is_none());
            old_active = Some(active);
        }
        assert!(!std::iter::from_fn(|| lifecycle.try_recv().ok()).any(|event| matches!(event, ModuleEvent::WakeWordDetected(_))), "system control fabricated a wakeword event");
        assert_eq!(s.f.count("delivery_responses").await, 3);
        assert_eq!(s.wire.pushes.load(Ordering::SeqCst), 3);
        owner.stage = "body complete";
    }), SpeechFixture::close).await;
}
