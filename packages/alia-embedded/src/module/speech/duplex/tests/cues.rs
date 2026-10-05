use super::*;

#[derive(Default)]
struct CueLookup(AtomicU64);

impl ModuleLifecycle for CueLookup {
    fn name(&self) -> &'static str {
        "cue_lookup"
    }
}

impl DirectResponseEngine for CueLookup {
    fn match_keyword(&self, _: &str) -> Option<crate::module::traits::DirectResponseAction> {
        None
    }

    fn match_action(&self, action: &str) -> Option<crate::module::traits::DirectResponseAction> {
        assert_eq!(action, "wakeword");
        self.0.fetch_add(1, Ordering::SeqCst);
        // An unconfigured optional cue must leave continuous capture available.
        None
    }
}

#[tokio::test]
async fn local_wake_cue_waits_for_authenticated_ready_and_does_not_gate_capture() {
    let ready_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let Harness {
        mut runtime,
        audio,
        events: _,
        local: _,
        mut seen,
        shutdown,
        server,
        _directory,
    } = harness_with_ready_gate(8, Some(ready_gate.clone())).await;
    let cue = Arc::new(CueLookup::default());
    runtime.direct_response = cue.clone();
    let playback = runtime.playback_session.clone();
    let cancel = runtime.cancel.clone();
    publish(&audio, 99, 0);
    let task = tokio::spawn(runtime.run());
    let Seen::Opened {
        boot,
        session,
        output,
    } = received(&mut seen).await
    else {
        panic!("expected Open before Ready");
    };
    assert_eq!(cue.0.load(Ordering::SeqCst), 0);
    assert!(playback.active_scope().unwrap().is_none());

    ready_gate.add_permits(1);
    assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
    reached(&cue.0, 1).await;
    assert_eq!(
        playback.active_scope().unwrap(),
        Some(VoiceScope {
            device_boot_id: Uuid::parse_str(&boot).unwrap(),
            session_id: Uuid::parse_str(&session).unwrap(),
            generation_epoch: 1,
        })
    );
    publish(&audio, 1, 320);
    assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
    assert_eq!(cue.0.load(Ordering::SeqCst), 1);

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    drop(output);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn cancellation_before_ready_joins_cue_worker_without_local_playback() {
    let ready_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let Harness {
        mut runtime,
        audio,
        events: _,
        local: _,
        mut seen,
        shutdown,
        server,
        _directory,
    } = harness_with_ready_gate(8, Some(ready_gate.clone())).await;
    let cue = Arc::new(CueLookup::default());
    runtime.direct_response = cue.clone();
    let cancel = runtime.cancel.clone();
    publish(&audio, 99, 0);
    let task = tokio::spawn(runtime.run());
    let Seen::Opened { output, .. } = received(&mut seen).await else {
        panic!("expected Open before Ready");
    };
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cue.0.load(Ordering::SeqCst), 0);
    assert!(
        seen.try_recv().is_err(),
        "cancelled handshake must not upload capture"
    );

    ready_gate.add_permits(1);
    drop(output);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn loopback_missing_or_foreign_scope_retires_capture_and_never_replays_wake_cue() {
    for missing_boot in [true, false] {
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
        let cue = Arc::new(CueLookup::default());
        runtime.direct_response = cue.clone();
        let playback = runtime.playback_session.clone();
        let cancel = runtime.cancel.clone();
        let mut lifecycle = events.subscribe();
        publish(&audio, 99, 0);
        let task = tokio::spawn(runtime.run());
        let Seen::Opened {
            boot,
            session,
            output,
        } = received(&mut seen).await
        else {
            panic!("expected Open");
        };
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
        reached(&cue.0, 1).await;
        let mut invalid = evidence(&boot, &session, 0, TurnState::Complete);
        if missing_boot {
            invalid.device_boot_id.clear();
        } else {
            invalid.session_id = Uuid::new_v4().to_string();
        }
        output.send(Ok(invalid)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !matches!(lifecycle.recv().await.unwrap(), ModuleEvent::SpeechEnd) {}
        })
        .await
        .unwrap();
        assert!(playback.active_scope().unwrap().is_none());
        assert_eq!(cue.0.load(Ordering::SeqCst), 1);
        tokio::time::timeout(Duration::from_secs(3), output.closed())
            .await
            .unwrap();

        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        drop(output);
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    }
}
