use super::*;
use alia_core::delivery::{
    PlaybackIdentity, PlaybackProgress, SubmissionBackend, SubmissionBoundary, VoiceScope,
};
use alia_core::output_control::{
    OutputBinding, OutputRegistration, OutputRetirementEvidence, OutputRetirementReceipt,
    OutputRetirementRequest,
};
use alia_server::channels::adapter::{
    AudioDeliveryReport, AudioPayloadStream, ChannelAdapter, ChannelCapabilities, ChannelTarget,
    PlaybackObserver,
};
use alia_server::modules::delivery::repository::DeliveryRepository;
use alia_server::modules::delivery::types::{
    NewDelivery, NewResponse, ResponseKey, ResponseStatus,
};
use alia_server::providers::adapter::ChatMessage;
use alia_server::state::AppState;
use sea_orm::{DatabaseBackend, Statement};
use serde_json::json;

pub(super) async fn account(state: &AppState, role: &str) -> (Uuid, String) {
    let id = Uuid::new_v4();
    state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Delivery test','not-a-login-hash',$3,'active')",
        [id.into(), format!("{id}@delivery.test").into(), role.into()],
    )).await.unwrap();
    let token = state
        .jwt_keys
        .encode(id, "test@delivery.test", "Delivery test", role, 1)
        .unwrap();
    (id, token)
}

pub(super) async fn draft(
    state: &AppState,
    owner: Uuid,
) -> alia_server::modules::delivery::types::ResponseRecord {
    draft_with_voice(state, owner, None).await
}

async fn draft_with_voice(
    state: &AppState,
    owner: Uuid,
    voice: Option<VoiceScope>,
) -> alia_server::modules::delivery::types::ResponseRecord {
    let session = state
        .modules
        .conversation
        .create_session("delivery-test", &owner.to_string())
        .await
        .unwrap();
    let admission = alia_server::modules::memory::source::capture_source_epochs(&state.db, owner)
        .await
        .unwrap();
    let epoch = admission.privacy_epoch;
    let received = chrono::Utc::now();
    let source = state
        .modules
        .conversation
        .append_sourced_message(
            owner,
            session,
            ChatMessage::new("user", "请回应"),
            received,
            epoch,
            admission.relationship_epoch,
        )
        .await
        .unwrap();
    let request_id = Uuid::new_v4();
    let context = state
        .modules
        .context
        .materialize(
            alia_server::modules::memory::context::ContextSource {
                owner_id: owner,
                request_id,
                channel_id: "delivery-test".into(),
                conversation_id: Some(session),
                source_event_id: Some(source),
                source_received_at: received,
                source_privacy_epoch: epoch,
            },
            "请回应",
        )
        .await
        .unwrap();
    let mut content = json!({"intent":"respond_to_source"});
    if let Some(scope) = voice {
        content["voice_scope"] = json!(scope);
    }
    DeliveryRepository::new(state.db.clone())
        .prepare(NewResponse {
            context_id: context.lease().id(),
            key: ResponseKey {
                user_id: owner,
                request_id,
                output_key: "assistant_response".into(),
            },
            conversation_id: Some(session),
            source_event_id: Some(source),
            source_received_at: received,
            source_privacy_epoch: Some(epoch),
            channel_id: "delivery-test".into(),
            kind: "assistant_response".into(),
            content,
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn canonical_response_and_history_commit_once_or_roll_back_together() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, _) = account(&state, "user").await;
        let (other, _) = account(&state, "user").await;
        let response = draft(&state, owner).await;
        let repository = DeliveryRepository::new(state.db.clone());
        let session = response.conversation_id.unwrap();
        state.db.execute_unprepared("CREATE FUNCTION fail_response_message() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.role='assistant' THEN RAISE EXCEPTION 'injected assistant insert failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_response_message BEFORE INSERT ON chat_messages FOR EACH ROW EXECUTE FUNCTION fail_response_message()").await.unwrap();
        assert!(state.modules.conversation.commit_response(owner, session, response.response_id, ChatMessage::new("assistant", "规范响应")).await.is_err());
        let unchanged = repository.response(owner, response.response_id).await.unwrap();
        assert_eq!(unchanged.status, ResponseStatus::Pending);
        assert!(unchanged.assistant_message_id.is_none());
        assert!(unchanged.final_content.is_none());
        state.db.execute_unprepared("DROP TRIGGER fail_response_message ON chat_messages; DROP FUNCTION fail_response_message()").await.unwrap();

        let (left, right) = tokio::join!(
            state.modules.conversation.commit_response(owner, session, response.response_id, ChatMessage::new("assistant", "规范响应")),
            state.modules.conversation.commit_response(owner, session, response.response_id, ChatMessage::new("assistant", "规范响应")),
        );
        let id = left.unwrap();
        assert_eq!(right.unwrap(), id);
        let committed = repository.response(owner, response.response_id).await.unwrap();
        assert_eq!(committed.status, ResponseStatus::Committed);
        assert_eq!(committed.assistant_message_id, Some(id));
        assert_eq!(committed.final_content, Some(json!({"text":"规范响应"})));
        let count: i64 = state.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT count(*) AS count FROM chat_messages WHERE session_id=$1 AND role='assistant'", [session.into()],
        )).await.unwrap().unwrap().try_get("", "count").unwrap();
        assert_eq!(count, 1);
        assert!(state.modules.conversation.commit_response(owner, session, response.response_id, ChatMessage::new("assistant", "篡改重试")).await.is_err());
        assert!(state.modules.conversation.commit_response(other, session, response.response_id, ChatMessage::new("assistant", "规范响应")).await.is_err());
        let different_session = state.modules.conversation.create_session("delivery-test", &owner.to_string()).await.unwrap();
        assert!(state.modules.conversation.commit_response(owner, different_session, response.response_id, ChatMessage::new("assistant", "规范响应")).await.is_err());
        // Committing text alone never creates device presentation evidence.
        assert!(state.delivery.list_deliveries(owner, response.response_id).await.unwrap().is_empty());
        runtime.shutdown().await.unwrap();
    }).await;
}

#[tokio::test]
async fn delivery_http_is_owner_scoped_and_cancellation_is_separate_from_generation() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let address = runtime.http_addr();
        let state = runtime.state().clone();
        let (owner, owner_token) = account(&state, "user").await;
        let (_, other_token) = account(&state, "admin").await;
        let response = draft(&state, owner).await;
        let delivery = DeliveryRepository::new(state.db.clone())
            .prepare_delivery(
                owner,
                NewDelivery {
                    response_id: response.response_id,
                    output_key: "speech:0".into(),
                    content: json!({"text":"规范响应"}),
                    identity: None,
                    owner_id: Uuid::new_v4(),
                },
            )
            .await
            .unwrap();
        let stop = state.cancel_token.clone();
        let _stop_on_drop = stop.clone().drop_guard();
        let serving = tokio::spawn(runtime.run());
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        for path in [
            format!("responses/{}", response.response_id),
            format!("responses/{}/deliveries", response.response_id),
            format!("responses/{}/events", response.response_id),
            format!("deliveries/{}", delivery.delivery_id),
        ] {
            let url = format!("http://{address}/api/v1/{path}");
            assert_eq!(
                client
                    .get(&url)
                    .bearer_auth(&owner_token)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
            assert_eq!(
                client
                    .get(&url)
                    .bearer_auth(&other_token)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                404
            );
            assert_eq!(client.get(&url).send().await.unwrap().status(), 401);
        }
        let invalid = client
            .get(format!("http://{address}/api/v1/deliveries/invalid-uuid"))
            .bearer_auth(&owner_token)
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), 400);
        assert_eq!(
            invalid.json::<serde_json::Value>().await.unwrap()["error"]["code"],
            "invalid_input"
        );
        let cancel_url = format!(
            "http://{address}/api/v1/responses/{}/cancel",
            response.response_id
        );
        assert_eq!(
            client
                .post(&cancel_url)
                .bearer_auth(&other_token)
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        let cancel = client
            .post(cancel_url)
            .bearer_auth(&owner_token)
            .send()
            .await
            .unwrap();
        assert_eq!(cancel.status(), 200);
        let cancelled = cancel.json::<serde_json::Value>().await.unwrap();
        assert_eq!(cancelled["cancel_requested"], true);
        assert_ne!(cancelled["status"], "committed");
        let fact = client
            .get(format!(
                "http://{address}/api/v1/deliveries/{}",
                delivery.delivery_id
            ))
            .bearer_auth(&owner_token)
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(fact["status"], "cancelled");
        assert!(fact["presented_frames"].is_null());
        assert!(fact["progress"].is_null());
        let events_url = format!(
            "http://{address}/api/v1/responses/{}/events",
            response.response_id
        );
        let first: Vec<serde_json::Value> = client
            .get(format!("{events_url}?limit=1"))
            .bearer_auth(&owner_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        let cursor = first[0]["event_id"].as_i64().unwrap();
        let next: Vec<serde_json::Value> = client
            .get(format!("{events_url}?after={cursor}&limit=1"))
            .bearer_auth(&owner_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(next.len(), 1);
        assert!(next[0]["event_id"].as_i64().unwrap() > cursor);
        assert_eq!(next[0]["response_id"], response.response_id.to_string());
        for invalid in [
            "after=-1",
            "limit=0",
            "limit=257",
            "after=broken",
            "limit=-2",
        ] {
            let result = client
                .get(format!("{events_url}?{invalid}"))
                .bearer_auth(&owner_token)
                .send()
                .await
                .unwrap();
            assert_eq!(result.status(), 400);
            assert_eq!(
                result.json::<serde_json::Value>().await.unwrap()["error"]["code"],
                "invalid_input"
            );
        }
        stop.cancel();
        serving.await.unwrap().unwrap();
    })
    .await;
}

struct NativePlayback {
    owner: Uuid,
    release: Arc<NativeGate>,
    entered: Arc<Semaphore>,
    active: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
}

#[async_trait]
impl ModuleLifecycle for NativePlayback {
    fn name(&self) -> &'static str {
        "native_playback_probe"
    }
    async fn stop(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.active.load(Ordering::SeqCst) == 0,
            "channel stopped while native playback still owns it"
        );
        self.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for NativePlayback {
    fn channel_type(&self) -> &'static str {
        "native_playback_probe"
    }
    fn channel_id(&self) -> &str {
        "delivery-test"
    }
    fn owner_user_id(&self) -> Option<Uuid> {
        Some(self.owner)
    }
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            voice_output: true,
            ..Default::default()
        }
    }
    async fn send_message(&self, _: &ChannelTarget, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn deliver_audio(
        &self,
        _: &ChannelTarget,
        _: Option<PlaybackIdentity>,
        _: AudioPayloadStream,
        _: Arc<dyn PlaybackObserver>,
        _: CancellationToken,
    ) -> anyhow::Result<AudioDeliveryReport> {
        let (release, entered, active) = (
            self.release.clone(),
            self.entered.clone(),
            self.active.clone(),
        );
        // This native API deliberately cannot cooperate with async cancellation.
        // The owning future must remain alive until its actual join finishes.
        tokio::task::spawn_blocking(move || {
            active.fetch_add(1, Ordering::SeqCst);
            entered.add_permits(1);
            release.wait();
            active.fetch_sub(1, Ordering::SeqCst);
        })
        .await?;
        Ok(AudioDeliveryReport::unobserved(
            "native probe returned without a device clock",
        ))
    }
}

#[tokio::test]
async fn delivery_shutdown_retains_native_owner_channels_and_database_past_both_deadlines() {
    with_fixture(|mut f| async move {
        f.config.server.shutdown_grace_ms = 40;
        alia_server::modules::platform::migrations::run(&f.db).await.unwrap();
        let owner = Uuid::new_v4();
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Native test','unused','user','active')",
            [owner.into(), format!("{owner}@native.test").into()],
        )).await.unwrap();
        let release = Arc::new(NativeGate::default());
        let _release_on_drop = ReleaseOnDrop(release.clone());
        let entered = Arc::new(Semaphore::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let runtime = ServerBuilder::from_config(Arc::new(f.config), CancellationToken::new())
            .with_channel(NativePlayback { owner, release: release.clone(), entered: entered.clone(), active: active.clone(), stops: stops.clone() })
            .build().await.unwrap();
        let state = runtime.state().clone();
        let response = draft(&state, owner).await;
        let service = state.delivery.clone();
        let channel = state.channels.get("delivery-test").await.unwrap();
        let dispatch = tokio::spawn(async move {
            service.deliver_audio(alia_server::modules::delivery::AudioDelivery {
                response_id: response.response_id, output_key: "native:0".into(), content: json!({"text":"native probe"}),
                channel, target: ChannelTarget { user_id: owner.to_string(), voice: None },
                audio: Box::pin(futures::stream::empty()), cancel: CancellationToken::new(),
            }).await
        });
        signalled(&entered).await;
        // Losing the pipeline's reply waiter cannot release the sole native owner.
        dispatch.abort();
        assert!(dispatch.await.unwrap_err().is_cancelled());
        let handle = runtime.shutdown_handle();
        assert!(runtime.shutdown().await.unwrap_err().is::<ShutdownIncomplete>());
        tokio::time::sleep(Duration::from_millis(10_350)).await;
        assert!(!handle.is_finished());
        assert!(handle.pending_modules().iter().any(|name| name == "delivery"));
        assert_eq!(active.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 0);
        state.db.ping().await.unwrap();
        release.release();
        // The earlier local shutdown timeout remains an honest historical failure.
        let final_result = tokio::time::timeout(Duration::from_secs(5), handle.wait()).await.unwrap();
        assert!(final_result.is_err());
        assert!(handle.is_finished());
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert!(state.db.ping().await.is_err());
    }).await;
}

struct PanicFencePlayback {
    inner: NativePlayback,
    fence_entered: Arc<Semaphore>,
    media_dropped: Arc<AtomicUsize>,
    registration: std::sync::OnceLock<OutputRegistration>,
    retired: std::sync::atomic::AtomicBool,
    retirement_finished: Arc<AtomicUsize>,
}

struct MediaDrop(Arc<AtomicUsize>);
impl Drop for MediaDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl ModuleLifecycle for PanicFencePlayback {
    fn name(&self) -> &'static str {
        "panic_fence_playback_probe"
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.inner.stop().await
    }
}

#[async_trait]
impl ChannelAdapter for PanicFencePlayback {
    fn channel_type(&self) -> &'static str {
        "panic_fence_playback_probe"
    }

    fn channel_id(&self) -> &str {
        self.inner.channel_id()
    }

    fn owner_user_id(&self) -> Option<Uuid> {
        self.inner.owner_user_id()
    }

    fn capabilities(&self) -> ChannelCapabilities {
        self.inner.capabilities()
    }

    fn reserve_playback(
        &self,
        target: &ChannelTarget,
        playback_id: Uuid,
    ) -> anyhow::Result<Option<PlaybackIdentity>> {
        assert!(!self.retired.load(Ordering::SeqCst));
        assert_eq!(
            target.voice,
            Some(self.registration.get().unwrap().binding.scope)
        );
        Ok(Some(PlaybackIdentity {
            scope: target.voice.expect("probe needs an original voice scope"),
            playback_id,
            playback_sequence: 1,
        }))
    }

    async fn send_message(&self, _: &ChannelTarget, _: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn register_output(&self, binding: OutputBinding) -> anyhow::Result<OutputRegistration> {
        assert!(!self.retired.load(Ordering::SeqCst));
        let registration = OutputRegistration {
            binding,
            backend: SubmissionBackend {
                instance_id: Uuid::new_v4(),
                boundary: SubmissionBoundary::CpalCallbackBuffer,
                output_sample_rate_hz: 48000,
                output_channels: 1,
            },
            maximum_hold_ms: 60000,
        };
        self.registration.set(registration.clone()).unwrap();
        Ok(registration)
    }

    async fn retire_voice_output(
        &self,
        request: &OutputRetirementRequest,
    ) -> anyhow::Result<OutputRetirementReceipt> {
        request.validate()?;
        let registration = self.registration.get().unwrap();
        assert_eq!(request.scope, registration.binding.scope);
        assert_eq!(request.binding, Some(registration.binding));
        assert_eq!(
            request.expected_backend.as_ref(),
            Some(&registration.backend)
        );
        self.retired.store(true, Ordering::SeqCst);
        while self.inner.active.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        // This empty-PCM probe owns only a software gate and a blocking call.
        // It proves those writes settled, without inventing media progress or
        // claiming that the independently panicked precise cancel completed.
        let receipt = OutputRetirementReceipt {
            scope: request.scope,
            binding: request.binding,
            evidence: OutputRetirementEvidence::Settled {
                backend: registration.backend.clone(),
                progress: vec![],
            },
        };
        receipt.validate(request)?;
        self.retirement_finished.fetch_add(1, Ordering::SeqCst);
        Ok(receipt)
    }

    async fn deliver_audio(
        &self,
        target: &ChannelTarget,
        identity: Option<PlaybackIdentity>,
        audio: AudioPayloadStream,
        observer: Arc<dyn PlaybackObserver>,
        cancel: CancellationToken,
    ) -> anyhow::Result<AudioDeliveryReport> {
        assert!(!self.retired.load(Ordering::SeqCst));
        let _drop = MediaDrop(self.media_dropped.clone());
        self.inner
            .deliver_audio(target, identity, audio, observer, cancel)
            .await
    }

    async fn cancel_playback(
        &self,
        identity: PlaybackIdentity,
        _: &str,
    ) -> anyhow::Result<PlaybackProgress> {
        assert!(identity.is_valid());
        self.fence_entered.add_permits(1);
        panic!("injected precise cancellation panic");
    }
}

// An unproven panicked control is deliberately quarantined until process exit.
// Use a child so that this test never leaves its native cleanup owner behind
// in the normal test runner. The child still uses real PG and spawn_blocking.
#[test]
fn panicked_playback_fence_retains_original_native_future_and_dependencies() {
    const CHILD: &str = "ALIA_DELIVERY_PANIC_FENCE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "delivery::panicked_playback_fence_retains_original_native_future_and_dependencies",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "isolated playback panic contract failed");
                return;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("playback panic child exceeded its process deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let host = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let checked = host.block_on(async {
        with_fixture(|mut f| async move {
            f.config.server.shutdown_grace_ms = 40;
            alia_server::modules::platform::migrations::run(&f.db).await.unwrap();
            let owner = Uuid::new_v4();
            f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Panic test','unused','user','active')",
                [owner.into(), format!("{owner}@panic.test").into()],
            )).await.unwrap();
            let release = Arc::new(NativeGate::default());
            let _release_on_drop = ReleaseOnDrop(release.clone());
            let entered = Arc::new(Semaphore::new(0));
            let fence_entered = Arc::new(Semaphore::new(0));
            let active = Arc::new(AtomicUsize::new(0));
            let stops = Arc::new(AtomicUsize::new(0));
            let media_dropped = Arc::new(AtomicUsize::new(0));
            let retirement_finished = Arc::new(AtomicUsize::new(0));
            let runtime = ServerBuilder::from_config(Arc::new(f.config), CancellationToken::new())
                .with_channel(PanicFencePlayback {
                    inner: NativePlayback {
                        owner, release: release.clone(), entered: entered.clone(),
                        active: active.clone(), stops: stops.clone(),
                    },
                    fence_entered: fence_entered.clone(), media_dropped: media_dropped.clone(),
                    registration: std::sync::OnceLock::new(),
                    retired: std::sync::atomic::AtomicBool::new(false),
                    retirement_finished: retirement_finished.clone(),
                }).build().await.unwrap();
            let state = runtime.state().clone();
            let scope = VoiceScope {
                device_boot_id: Uuid::new_v4(), session_id: Uuid::new_v4(), generation_epoch: 1,
            };
            let response = draft_with_voice(&state, owner, Some(scope)).await;
            let response_id = response.response_id;
            let service = state.delivery.clone();
            let channel = state.channels.get("delivery-test").await.unwrap();
            let _producer = service.begin_voice_output(
                owner, response.key.request_id, scope, channel.clone(), CancellationToken::new(), None,
            ).unwrap();
            _producer.bind(response).await.unwrap();
            let dispatch = tokio::spawn(async move {
                service.deliver_audio(alia_server::modules::delivery::AudioDelivery {
                    response_id, output_key: "native:panic".into(), content: json!({"text":"panic probe"}),
                    channel, target: ChannelTarget { user_id: owner.to_string(), voice: Some(scope) },
                    audio: Box::pin(futures::stream::empty()), cancel: CancellationToken::new(),
                }).await
            });
            signalled(&entered).await;
            dispatch.abort();
            assert!(dispatch.await.unwrap_err().is_cancelled());
            let completion = runtime.shutdown_handle();
            assert!(runtime.shutdown().await.unwrap_err().is::<ShutdownIncomplete>());
            signalled(&fence_entered).await;
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let records = state.delivery.list_deliveries(owner, response_id).await.unwrap();
                    if records[0].status == alia_server::modules::delivery::types::DeliveryStatus::Unknown {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.unwrap();
            assert_eq!(active.load(Ordering::SeqCst), 1);
            assert_eq!(media_dropped.load(Ordering::SeqCst), 0);
            assert_eq!(retirement_finished.load(Ordering::SeqCst), 0);
            assert_eq!(stops.load(Ordering::SeqCst), 0);
            assert!(state.delivery.shutdown_pending());
            state.db.ping().await.unwrap();
            release.release();
            tokio::time::timeout(Duration::from_secs(2), async {
                while active.load(Ordering::SeqCst) != 0 || retirement_finished.load(Ordering::SeqCst) != 1 {
                    tokio::task::yield_now().await;
                }
            }).await.unwrap();
            // The native call ending cannot prove a panicked control completed.
            assert!(completion.wait_until(tokio::time::Instant::now() + Duration::from_millis(40)).await.unwrap_err().is::<ShutdownIncomplete>());
            assert!(!completion.is_finished());
            assert!(completion.pending_modules().iter().any(|name| name == "delivery"));
            assert_eq!(media_dropped.load(Ordering::SeqCst), 0);
            assert_eq!(stops.load(Ordering::SeqCst), 0);
            state.db.ping().await.unwrap();
        }).await;
        true
    });
    assert!(checked);
    // All assertions and fixture cleanup have completed. The quarantined
    // control cannot be safely joined; end this isolated test process instead
    // of dismantling its live Tokio context beneath retained cleanup futures.
    std::process::exit(0);
}
