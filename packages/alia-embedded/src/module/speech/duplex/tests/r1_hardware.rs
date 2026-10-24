//! Explicit opt-in only: real UART/ALSA, local VAD, authenticated loopback fixture.
use super::*;
use crate::config::{AudioBackend, EmbeddedConfig};
use crate::module::EmbeddedBuilder;
use crate::module::lifecycle::{ModuleHealth, ShutdownIncomplete};
use futures::FutureExt;

#[derive(Default, serde::Serialize)]
struct CaptureStats {
    frames: u64,
    samples: u64,
    nonzero_samples: u64,
    peak: i32,
    speech_frames: u64,
    trailing_silence_samples: u64,
}

impl CaptureStats {
    fn accept(&mut self, frame: &CaptureFrame) -> Result<()> {
        ensure!(frame.sequence == self.frames, "capture sequence skipped");
        ensure!(
            frame.sample_offset == self.samples,
            "capture sample cursor skipped"
        );
        ensure!(frame.pcm_s16le.len() % 2 == 0, "odd PCM byte count");
        self.frames += 1;
        let samples = frame.pcm_s16le.len() as u64 / 2;
        self.samples += samples;
        for bytes in frame.pcm_s16le.chunks_exact(2) {
            let sample = i16::from_le_bytes([bytes[0], bytes[1]]) as i32;
            self.nonzero_samples += u64::from(sample != 0);
            self.peak = self.peak.max(sample.abs());
        }
        if frame.speech_active {
            self.speech_frames += 1;
            self.trailing_silence_samples = 0;
        } else if self.speech_frames > 0 {
            self.trailing_silence_samples += samples;
        }
        Ok(())
    }
}

fn hardware_config(directory: &tempfile::TempDir) -> Result<EmbeddedConfig> {
    let path = PathBuf::from(std::env::var_os("ALIA_R1_HARDWARE_CONFIG").context(
        "set ALIA_R1_HARDWARE_CONFIG to an explicit dedicated R1 config before opening devices",
    )?);
    ensure!(
        path.is_absolute() && path.is_file(),
        "hardware config must be an existing absolute file"
    );
    let mut config = crate::config::load_config(&path)?;
    ensure!(
        config.modules.wakeword == "r1"
            && config.modules.vad == "silero"
            && config.modules.audio_capture == "cpal"
            && config.audio.capture_backend == AudioBackend::Alsa
            && config.audio.capture_device == "hw:CARD=L6Microphone,DEV=0"
            && config.audio.capture_sample_rate == 16_000
            && config.audio.capture_channels == 1,
        "hardware test requires R1 L6Microphone ALSA at 16 kHz mono and Silero VAD"
    );
    ensure!(
        config.modules.audio_playback == "noop"
            && config.modules.direct_response == "noop"
            && config.modules.face_tracker == "noop"
            && config.modules.discovery == "noop"
            && !config.motion.enabled
            && !config.motion.hardware_enabled
            && !config.vision.enabled
            && !config.vision_debug.enabled
            && !config.hmmd_mmwave.enabled
            && !config.discovery.mdns_enabled
            && !config.discovery.ble_provision_enabled
            && !config.dev.skip_binding,
        "capture test requires output, actuators, discovery and dev binding bypass disabled"
    );
    ensure!(
        config.vad.model_path.is_file(),
        "provide the local Silero VAD model first"
    );
    config.config_path = directory.path().join("embedded.toml");
    config.device_runtime.listen_addr = "127.0.0.1:0".parse()?;
    config.device_runtime.state_path = directory.path().join("state.sqlite3");
    config.device_runtime.plugins_dir = directory.path().join("plugins");
    config.device_runtime.shutdown_grace_ms = 5_000;
    config.identity.device_id = Uuid::new_v4().to_string();
    config.identity.keypair_path = directory.path().join("device.key");
    Ok(config)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "opens the explicitly configured R1 UART and ALSA microphone; two live wake/short-utterance windows, no audio files or external models"]
async fn real_r1_uart_alsa_silero_authenticated_voice_and_rewake() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
                .add_directive("sqlx::query=warn".parse().expect("valid sqlx query filter")),
        )
        .with_test_writer()
        .try_init();
    let directory = tempfile::tempdir().unwrap();
    let config = hardware_config(&directory).unwrap();
    let seconds = std::env::var("ALIA_R1_HARDWARE_TIMEOUT_SEC")
        .unwrap_or_else(|_| "120".into())
        .parse::<u64>()
        .unwrap();
    assert!(
        (1..=120).contains(&seconds),
        "hardware window must be 1..=120 seconds"
    );
    let key = crate::identity::ensure_device_keypair(&config.identity.keypair_path).unwrap();
    let public_key = key.public_key_bytes().to_vec();
    let subject = embedded_device_auth_subject(&config.identity.device_id, &public_key);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().unwrap().port() - 1
    );
    let (seen_tx, mut seen) = mpsc::channel(32);
    let server_cancel = CancellationToken::new();
    let server_stop = server_cancel.clone();
    let channel = config.speech.channel_id.clone();
    let device_cancel = CancellationToken::new();
    let builder = EmbeddedBuilder::from_config(config)
        .await
        .unwrap()
        .with_cancel_token(device_cancel.clone());
    let mut server = tokio::spawn(async move {
        let incoming = async_stream::stream! {
            loop { yield listener.accept().await.map(|(stream, _)| stream); }
        };
        tonic::transport::Server::builder()
            .add_service(VoiceSessionServer::new(FakeServer {
                expected_channel_id: channel,
                ready_gate: None,
                seen: seen_tx,
                public_key,
                subject,
            }))
            .serve_with_incoming_shutdown(incoming, server_stop.cancelled())
            .await
    });
    eprintln!(
        "ALIA_R1_HARDWARE {{\"stage\":\"starting_real_uart_and_capture\",\"model_response\":\"local_protocol_fixture_only\",\"pcm_saved\":false}}"
    );
    let mut startup = tokio::spawn(builder.build());
    let mut startup_joined = false;
    let mut node = None;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(seconds),
        async {
            let started = (&mut startup).await;
            startup_joined = true;
            node = Some(started??);
            let node = node.as_ref().unwrap();
            let key = ed25519_dalek::SigningKey::from_bytes(&rand::random());
            let public = key.verifying_key().to_bytes();
            let issued = alia_transport::current_unix_timestamp()?;
            let payload = alia_transport::bind_payload("r1-fixture", "Local protocol fixture", &endpoint, &public, issued);
            node.binding_manager.bind("r1-fixture", "Local protocol fixture", &endpoint, &public,
                &alia_transport::sign_payload(&key, &payload), issued).await?;
            let mut events = node.event_bus.subscribe();
            while node.registry.wakeword.health_check().await != ModuleHealth::Healthy {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let mut previous_session = None;
            for round in 1..=2 {
                eprintln!("ALIA_R1_HARDWARE {}", serde_json::json!({"stage":"await_natural_wake_then_short_phrase_and_silence","round":round}));
                let wake = loop {
                    if let ModuleEvent::WakeWordDetected(wake) = events.recv().await? { break wake; }
                };
                ensure!(matches!(wake.source, WakeWordSource::R1 { .. }), "wake did not originate on R1 UART");
                let Seen::Opened { session, output, .. } = seen.recv().await.context("voice fixture stopped")? else {
                    bail!("capture preceded authenticated Open")
                };
                ensure!(previous_session.as_ref() != Some(&session), "rewake reused the old session");
                previous_session = Some(session);
                eprintln!("ALIA_R1_HARDWARE {}", serde_json::json!({"stage":"authenticated_voice_open","round":round,"wake_source":format!("{:?}",wake.source)}));
                let mut stats = CaptureStats::default();
                while stats.trailing_silence_samples < u64::from(SAMPLE_RATE) {
                    let Seen::Audio(frame) = seen.recv().await.context("voice fixture stopped")? else {
                        bail!("another voice session opened during active capture")
                    };
                    stats.accept(&frame)?;
                }
                ensure!(stats.nonzero_samples > 0 && stats.speech_frames > 0, "no real speech evidence");
                eprintln!("ALIA_R1_HARDWARE {}", serde_json::json!({"stage":"speech_and_one_second_tail_received","round":round,"statistics":stats}));
                output.send(Err(Status::unavailable("intentional local fixture disconnect; no model response"))).await?;
                loop {
                    tokio::select! {
                        event = events.recv() => if matches!(event?, ModuleEvent::SpeechEnd) { break; },
                        frame = seen.recv() => {
                            ensure!(matches!(frame, Some(Seen::Audio(_))), "unexpected session while closing capture");
                        },
                    }
                }
                // The voice task is joined before SpeechEnd; drain only its queued telemetry.
                while let Ok(item) = seen.try_recv() {
                    ensure!(matches!(item, Seen::Audio(_)), "old wake reopened a voice session");
                }
                output.closed().await;
                ensure!(node.registry.playback_session.active_scope()?.is_none(), "closed voice scope retained");
                eprintln!("ALIA_R1_HARDWARE {}", serde_json::json!({"stage":"session_closed_fresh_wake_required","round":round}));
            }
            Ok::<_, anyhow::Error>(())
        },
    )).catch_unwind().await;
    device_cancel.cancel();
    // Always drain bounded telemetry while joining, including after panic or timeout.
    let drainer = tokio::spawn(async move { while seen.recv().await.is_some() {} });
    let mut late_startup_error = None;
    if !startup_joined {
        // Keep the Builder future owned until its rollback has actually finished.
        match startup.await {
            Ok(Ok(started)) => node = Some(started),
            Ok(Err(error)) => late_startup_error = Some(error),
            Err(error) => late_startup_error = Some(error.into()),
        }
    }
    let mut cleanup_errors = Vec::new();
    let startup_error = late_startup_error.as_ref().or_else(|| match &result {
        Ok(Ok(Err(error))) if node.is_none() => Some(error),
        _ => None,
    });
    if let Some(error) = startup_error {
        cleanup_errors.push(format!("startup: {error:#}"));
        if let Some(incomplete) = error.downcast_ref::<ShutdownIncomplete>() {
            if let Err(error) = incomplete.handle.wait().await {
                cleanup_errors.push(format!("startup cleanup: {error:#}"));
            }
        }
    }
    if let Some(node) = node {
        let completion = node.shutdown_handle();
        if let Err(error) = node.shutdown().await {
            cleanup_errors.push(format!("device shutdown: {error:#}"));
        }
        // A deadline error retains ownership; wait for the actual native joins.
        if let Err(error) = completion.wait().await {
            cleanup_errors.push(format!("device cleanup: {error:#}"));
        }
    }
    server_cancel.cancel();
    let server_result = tokio::time::timeout(Duration::from_secs(5), &mut server).await;
    if server_result.is_err() {
        server.abort();
        let _ = server.await;
    }
    drainer.abort();
    let _ = drainer.await;
    eprintln!(
        "ALIA_R1_HARDWARE {}",
        serde_json::json!({"stage":if cleanup_errors.is_empty() {"cleanup_complete"} else {"cleanup_failed"},"device_cleanup_ok":cleanup_errors.is_empty(),"cleanup_errors":cleanup_errors,"server_joined":server_result.is_ok(),"pcm_saved":false})
    );
    assert!(cleanup_errors.is_empty(), "{}", cleanup_errors.join("; "));
    server_result.unwrap().unwrap().unwrap();
    match result {
        Ok(result) => result.expect("R1 live test window expired").unwrap(),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
