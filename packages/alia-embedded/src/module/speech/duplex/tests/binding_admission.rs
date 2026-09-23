use super::*;

async fn bound_playback() -> (tempfile::TempDir, Arc<BindingManager>, Arc<PlaybackSession>) {
    let directory = tempfile::tempdir().unwrap();
    let binding = Arc::new(BindingManager::new(
        EmbeddedStore::open(directory.path().join("state.sqlite"))
            .await
            .unwrap(),
        3,
        60,
    ));
    signed_bind(&binding, "server", "http://localhost:1234").await;
    let playback = Arc::new(PlaybackSession::new(Arc::new(LocalAudio::default())));
    let revoke = playback.clone();
    binding
        .install_output_revocation(Arc::new(move || revoke.revoke_all().unwrap()))
        .unwrap();
    (directory, binding, playback)
}

async fn signed_bind(binding: &BindingManager, server: &str, endpoint: &str) {
    let key = ed25519_dalek::SigningKey::from_bytes(&[61; 32]);
    let public = key.verifying_key().to_bytes();
    let issued = alia_transport::current_unix_timestamp().unwrap();
    let payload = alia_transport::bind_payload(server, "Server", endpoint, &public, issued);
    binding
        .bind(
            server,
            "Server",
            endpoint,
            &public,
            &alia_transport::sign_payload(&key, &payload),
            issued,
        )
        .await
        .unwrap();
}
async fn late_ready(transition: u8) {
    let (_directory, binding, playback) = bound_playback().await;
    let original = binding.status().await;
    let boot = Uuid::new_v4().to_string();
    let session = Uuid::new_v4().to_string();
    let mut guard = SessionGuard::new(boot.clone(), playback.clone());
    playback
        .accept_capture(&guard.capture, 0, true, || Ok(()))
        .unwrap();
    match transition {
        0 => binding.unbind("server").await.unwrap(),
        1 => {
            binding.unbind("server").await.unwrap();
            signed_bind(&binding, "next-server", "http://localhost:4321").await;
        }
        _ => {
            binding.load().await.unwrap();
            assert_eq!(
                serde_json::to_value(&original).unwrap(),
                serde_json::to_value(binding.status().await).unwrap(),
                "reload intentionally restores identical persisted fields"
            );
        }
    }
    let result = guard
        .accept_bound(&binding, &original, ready(&boot, &session), 0)
        .await;
    let scope = playback.active_scope().unwrap();
    drop(guard);
    assert!(
        result.is_err(),
        "retired original binding admitted late Ready: scope={scope:?}"
    );
    assert!(scope.is_none());
}
#[tokio::test]
async fn late_ready_after_unbind_is_rejected() {
    late_ready(0).await;
}
#[tokio::test]
async fn late_ready_after_rebind_is_rejected() {
    late_ready(1).await;
}
#[tokio::test]
async fn late_ready_after_same_persisted_binding_reload_is_rejected() {
    late_ready(2).await;
}

#[tokio::test]
async fn old_evidence_and_old_guard_cannot_change_new_binding_capture() {
    let (_directory, binding, playback) = bound_playback().await;
    let original = binding.status().await;
    let boot = Uuid::new_v4().to_string();
    let session = Uuid::new_v4().to_string();
    let mut old = SessionGuard::new(boot.clone(), playback.clone());
    old.accept_bound(&binding, &original, ready(&boot, &session), 0)
        .await
        .unwrap();
    binding.unbind("server").await.unwrap();
    signed_bind(&binding, "new-server", "http://localhost:4321").await;
    let current = binding.status().await;
    let mut replacement = SessionGuard::new(boot.clone(), playback.clone());
    // A revoked epoch remains retired even under the replacement binding.
    let reused = replacement
        .accept_bound(&binding, &current, ready(&boot, &session), 0)
        .await
        .unwrap_err();
    assert!(format!("{reused:#}").contains("retained retired output epoch"));
    assert!(playback.active_scope().unwrap().is_none());
    let replacement_session = Uuid::new_v4().to_string();
    replacement
        .accept_bound(&binding, &current, ready(&boot, &replacement_session), 0)
        .await
        .unwrap();
    let new_scope = playback.active_scope().unwrap().unwrap();
    let result = old
        .accept_bound(
            &binding,
            &original,
            evidence(&boot, &session, 0, TurnState::Wait),
            1,
        )
        .await;
    let after_evidence = playback.active_scope().unwrap();
    drop(old);
    let after_old_drop = playback.active_scope().unwrap();
    drop(replacement);
    let error = result.unwrap_err();
    assert!(format!("{error:#}").contains("original voice binding"));
    assert_eq!(after_evidence, Some(new_scope));
    assert_eq!(
        after_old_drop,
        Some(new_scope),
        "old guard cleanup must retain the new capture owner"
    );
}

async fn signed_exchange(
    harness: &Harness,
) -> (mpsc::Sender<CaptureFrame>, JoinHandle<Result<()>>) {
    let runtime = &harness.runtime;
    let endpoint = runtime.binding.status().await.server_endpoint.unwrap();
    runtime.binding.unbind("test").await.unwrap();
    signed_bind(&runtime.binding, "server", &endpoint).await;
    let playback = runtime.playback_session.clone();
    runtime
        .binding
        .install_output_revocation(Arc::new(move || playback.revoke_all().unwrap()))
        .unwrap();
    let original = runtime.binding.status().await;
    let (endpoint, auth) =
        voice_connection(&original, &runtime.device_id, &runtime.keypair_path).unwrap();
    let capture = CaptureControlBarrier::default();
    let (tx, rx) = mpsc::channel(2);
    runtime
        .playback_session
        .accept_capture(&capture, 0, true, || {
            tx.try_send(CaptureFrame {
                sequence: 0,
                sample_offset: 0,
                pcm_s16le: vec![0; 640],
                speech_active: true,
            })?;
            Ok(())
        })
        .unwrap();
    // Exercise the actual network receive boundary without the 250 ms runtime
    // monitor: that polling task must not hide an admission race.
    let task = tokio::spawn(exchange(
        endpoint,
        auth,
        runtime.boot_id.clone(),
        rx,
        runtime.playback_session.clone(),
        capture,
        runtime.device_id.clone(),
        runtime.keypair_path.clone(),
        runtime.binding.clone(),
        original,
    ));
    (tx, task)
}

async fn loopback_late_ready(rebind: bool) {
    let ready_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let mut harness = harness_with_ready_gate(8, Some(ready_gate.clone())).await;
    let (tx, task) = signed_exchange(&harness).await;
    let Seen::Opened { output, .. } = received(&mut harness.seen).await else {
        panic!("Open must precede capture upload");
    };
    // Ready is blocked at the server. Binding mutation must finish without a
    // network reply, then its retired original can never authorize activation.
    tokio::time::timeout(
        Duration::from_secs(3),
        harness.runtime.binding.unbind("server"),
    )
    .await
    .unwrap()
    .unwrap();
    if rebind {
        signed_bind(
            &harness.runtime.binding,
            "replacement",
            "http://localhost:4321",
        )
        .await;
    }
    ready_gate.add_permits(1);
    let error = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("original voice binding"));
    assert!(
        harness
            .runtime
            .playback_session
            .active_scope()
            .unwrap()
            .is_none()
    );
    assert!(
        harness.seen.try_recv().is_err(),
        "rejected Ready uploaded capture"
    );
    drop(tx);
    drop(output);
    harness.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), harness.server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn loopback_late_ready_after_unbind_never_activates() {
    loopback_late_ready(false).await;
}

#[tokio::test]
async fn loopback_late_ready_after_rebind_never_activates() {
    loopback_late_ready(true).await;
}

#[tokio::test]
async fn loopback_old_evidence_and_disconnect_preserve_replacement_owner() {
    let mut harness = harness(8).await;
    let (tx, task) = signed_exchange(&harness).await;
    let Seen::Opened {
        boot,
        session,
        output,
    } = received(&mut harness.seen).await
    else {
        panic!("Open must precede capture upload");
    };
    assert!(matches!(received(&mut harness.seen).await, Seen::Audio(_)));
    harness.runtime.binding.unbind("server").await.unwrap();
    signed_bind(
        &harness.runtime.binding,
        "replacement",
        "http://localhost:4321",
    )
    .await;
    let current = harness.runtime.binding.status().await;
    let playback = harness.runtime.playback_session.clone();
    let mut replacement = SessionGuard::new(boot.clone(), playback.clone());
    let reused = replacement
        .accept_bound(
            &harness.runtime.binding,
            &current,
            ready(&boot, &session),
            0,
        )
        .await
        .unwrap_err();
    assert!(format!("{reused:#}").contains("retained retired output epoch"));
    assert!(playback.active_scope().unwrap().is_none());
    let replacement_session = Uuid::new_v4().to_string();
    replacement
        .accept_bound(
            &harness.runtime.binding,
            &current,
            ready(&boot, &replacement_session),
            0,
        )
        .await
        .unwrap();
    let scope = playback.active_scope().unwrap().unwrap();
    output
        .send(Ok(evidence(&boot, &session, 0, TurnState::Wait)))
        .await
        .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("original voice binding"));
    assert_eq!(playback.active_scope().unwrap(), Some(scope));
    drop(replacement);
    assert!(playback.active_scope().unwrap().is_none());
    drop(tx);
    drop(output);
    harness.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), harness.server)
        .await
        .unwrap()
        .unwrap();
}

struct ActivationGate {
    entered: std::sync::Mutex<Option<oneshot::Sender<()>>>,
    release: Arc<std::sync::Barrier>,
}
impl crate::module::playback_session::VoiceOutputFence for ActivationGate {
    fn activate_voice(&self, _: &VoiceScope) -> Result<()> {
        self.entered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        self.release.wait();
        Ok(())
    }
    fn fence_voice(&self, _: &VoiceScope) -> Result<()> {
        Ok(())
    }
    fn close_voice(&self, _: &VoiceScope) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn ready_admission_holds_original_binding_until_synchronous_activation_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let binding = Arc::new(BindingManager::new(
        EmbeddedStore::open(directory.path().join("state.sqlite"))
            .await
            .unwrap(),
        3,
        60,
    ));
    signed_bind(&binding, "server", "http://localhost:1234").await;
    let original = binding.status().await;
    let (entered_tx, entered_rx) = oneshot::channel();
    let release = Arc::new(std::sync::Barrier::new(2));
    let playback = Arc::new(PlaybackSession::with_output_fence(
        Arc::new(LocalAudio::default()),
        Arc::new(ActivationGate {
            entered: std::sync::Mutex::new(Some(entered_tx)),
            release: release.clone(),
        }),
    ));
    // Only record entry in this ordering probe, so a missing Binding read lock
    // fails an assertion rather than deadlocking on the blocked Session mutex.
    let revoked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = revoked.clone();
    binding
        .install_output_revocation(Arc::new(move || {
            observed.store(true, Ordering::SeqCst);
        }))
        .unwrap();
    let admit_binding = binding.clone();
    let handle = tokio::runtime::Handle::current();
    let admission = tokio::task::spawn_blocking(move || {
        handle.block_on(async move {
            let boot = Uuid::new_v4().to_string();
            let session = Uuid::new_v4().to_string();
            let mut guard = SessionGuard::new(boot.clone(), playback);
            guard
                .accept_bound(&admit_binding, &original, ready(&boot, &session), 0)
                .await
        })
    });
    entered_rx.await.unwrap();
    let mut unbind = Box::pin(binding.unbind("server"));
    let pending = matches!(futures::poll!(unbind.as_mut()), std::task::Poll::Pending);
    let premature_revocation = revoked.load(Ordering::SeqCst);
    // Always release the test-only synchronous gate before any assertion.
    release.wait();
    admission.await.unwrap().unwrap();
    unbind.await.unwrap();
    assert!(pending, "binding writer passed an active Ready admission");
    assert!(
        !premature_revocation,
        "revocation entered during Ready activation"
    );
    assert!(revoked.load(Ordering::SeqCst));
}
