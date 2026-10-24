use super::*;

#[derive(Default)]
struct QueuedWake {
    pending: std::sync::Mutex<Option<WakeWordEvent>>,
    checks: AtomicU64,
    resets: AtomicU64,
}

#[tonic::async_trait]
impl ModuleLifecycle for QueuedWake {
    fn name(&self) -> &'static str {
        "test_queued_wake"
    }
}

impl WakeWordEngine for QueuedWake {
    fn reset(&self) -> Result<()> {
        self.pending.lock().unwrap().take();
        self.resets.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn detect(&self, pcm: &[i16]) -> Option<WakeWordEvent> {
        assert!(!pcm.is_empty());
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.pending.lock().unwrap().take()
    }
}

fn notification(message_id: u16) -> WakeWordEvent {
    WakeWordEvent {
        keyword: "xiao3-fei1-xiao3-fei1".into(),
        timestamp_us: 1_792_583_300_123_456,
        source: WakeWordSource::R1 {
            message_id,
            angle_deg: Some(359.999_999),
            beam: Some(0),
        },
    }
}

#[tokio::test]
async fn external_wake_requires_pcm_preserves_source_and_never_reopens_from_active_session_queue() {
    let Harness {
        mut runtime,
        audio,
        events,
        local: _,
        mut seen,
        shutdown,
        server,
        _directory,
    } = harness(8).await;
    let wake = Arc::new(QueuedWake::default());
    *wake.pending.lock().unwrap() = Some(notification(1));
    runtime.wakeword = wake.clone();
    let cancel = runtime.cancel.clone();
    let mut lifecycle = events.subscribe();
    let task = tokio::spawn(runtime.run());
    reached(&wake.resets, 1).await;
    assert!(wake.pending.lock().unwrap().is_none());

    publish(&audio, 0, 0);
    reached(&wake.checks, 1).await;
    assert!(
        seen.try_recv().is_err(),
        "PCM alone must not open a session"
    );
    let observed = notification(2);
    *wake.pending.lock().unwrap() = Some(observed.clone());
    assert!(
        tokio::time::timeout(Duration::from_millis(25), seen.recv())
            .await
            .is_err(),
        "a UART notification alone must not open a session"
    );
    assert_eq!(wake.checks.load(Ordering::SeqCst), 1);

    publish(&audio, 0, 320);
    let Seen::Opened {
        output, session, ..
    } = received(&mut seen).await
    else {
        panic!("external wake and PCM must open a session")
    };
    for _ in 0..2 {
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
    }
    assert!(matches!(
        lifecycle.try_recv().unwrap(),
        ModuleEvent::WakeWordDetected(event) if event == observed
    ));
    assert!(matches!(
        lifecycle.try_recv().unwrap(),
        ModuleEvent::SpeechStart
    ));
    assert!(wake.pending.lock().unwrap().is_none());

    events.publish(ModuleEvent::PlaybackStarted {
        source: "tts".into(),
    });
    *wake.pending.lock().unwrap() = Some(notification(3));
    publish(&audio, 7, 640);
    publish(&audio, 7, 960);
    for _ in 0..2 {
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
    }
    assert_eq!(wake.checks.load(Ordering::SeqCst), 2);
    assert!(
        seen.try_recv().is_err(),
        "active capture opened another session"
    );
    assert!(wake.pending.lock().unwrap().is_some());
    assert!(
        !std::iter::from_fn(|| lifecycle.try_recv().ok())
            .any(|event| matches!(event, ModuleEvent::WakeWordDetected(_)))
    );

    output
        .send(Err(Status::unavailable("test disconnect")))
        .await
        .unwrap();
    reached(&wake.resets, 2).await;
    assert!(wake.pending.lock().unwrap().is_none());
    publish(&audio, 0, 1280);
    reached(&wake.checks, 3).await;
    assert!(
        seen.try_recv().is_err(),
        "queued wake survived session teardown"
    );

    *wake.pending.lock().unwrap() = Some(notification(4));
    publish(&audio, 7, 1600);
    let Seen::Opened {
        output: next_output,
        session: next_session,
        ..
    } = received(&mut seen).await
    else {
        panic!("a fresh external wake must reconnect")
    };
    assert_ne!(session, next_session);
    for _ in 0..2 {
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
    }
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

#[tokio::test]
async fn native_capture_gap_discards_queued_external_wake_before_fresh_capture() {
    for restart in [false, true] {
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
        let wake = Arc::new(QueuedWake::default());
        runtime.wakeword = wake.clone();
        let cancel = runtime.cancel.clone();
        let task = tokio::spawn(runtime.run());
        reached(&wake.resets, 1).await;
        let original = Uuid::new_v4();
        audio.publish(native_frame(0, 0, original, 1));
        reached(&wake.checks, 1).await;
        *wake.pending.lock().unwrap() = Some(notification(1));
        let (stream, continuity) = if restart {
            (Uuid::new_v4(), 1)
        } else {
            (original, 2)
        };
        audio.publish(native_frame(0, 320, stream, continuity));
        reached(&wake.resets, 2).await;
        assert!(wake.pending.lock().unwrap().is_none());
        audio.publish(native_frame(0, 640, stream, continuity));
        reached(&wake.checks, 2).await;
        assert!(
            seen.try_recv().is_err(),
            "queued wake crossed a capture gap"
        );

        *wake.pending.lock().unwrap() = Some(notification(2));
        audio.publish(native_frame(7, 960, stream, continuity));
        let Seen::Opened { output, .. } = received(&mut seen).await else {
            panic!("a new external wake must work after capture recovery")
        };
        for _ in 0..2 {
            assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
        }
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
