//! A buffered HTTP request must not keep Settings alive after server shutdown.
use super::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::io::AsyncWriteExt;

const VALUE: &str = "plugin.settings_shutdown.value";

struct ApplyGate {
    entered: Semaphore,
    release: Semaphore,
    dropped: Semaphore,
    active: AtomicUsize,
    values: Mutex<Vec<Value>>,
}

impl ApplyGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            dropped: Semaphore::new(0),
            active: AtomicUsize::new(0),
            values: Mutex::new(Vec::new()),
        })
    }
}

struct ApplyDrop(Arc<ApplyGate>);

impl Drop for ApplyDrop {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.dropped.add_permits(1);
    }
}

struct HangingSettings(Arc<ApplyGate>);

#[async_trait]
impl ModuleLifecycle for HangingSettings {
    fn name(&self) -> &'static str {
        "settings-shutdown-probe"
    }

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let Some(value) = updates.get(VALUE) else {
            return Ok(Vec::new());
        };
        self.0.values.lock().unwrap().push(value.clone());
        self.0.active.fetch_add(1, Ordering::SeqCst);
        let _drop = ApplyDrop(self.0.clone());
        self.0.entered.add_permits(1);
        self.0.release.acquire().await.unwrap().forget();
        Ok(vec![VALUE.into()])
    }
}

async fn apply_exit(disconnect: bool) {
    with_fixture(move |f| async move {
        let probe = ApplyGate::new();
        let runtime = f
            .builder()
            .with_extension(HangingSettings(probe.clone()))
            .build()
            .await
            .unwrap();
        let state = runtime.state().clone();
        let address = runtime.http_addr();
        let grpc = runtime.grpc_addr();
        let shutdown = runtime.shutdown_handle();
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let mut socket = None;
        let assertions = AssertUnwindSafe(async {
            let (_, token) = delivery::account(&state, "admin").await;
            let initial = state.modules.context.policy_store().current().await.unwrap();
            assert!(!initial.policy.memory_enabled);
            state.settings.set(VALUE, json!(1)).await;
            state.settings.save().await.unwrap();
            let path = f.dir.join("settings.toml");
            let original = std::fs::read(&path).unwrap();
            let body = json!({
                (VALUE): 2,
                "core.policies.memory.enabled": true,
            })
            .to_string();
            // Buffered pipeline bytes prevent relying only on a later socket read
            // to wake Hyper while the first response is waiting in application code.
            let mut wire = format!(
                "PATCH /api/v1/settings HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len(),
            );
            if !disconnect {
                wire.push_str(&format!(
                    "GET /api/v1/health HTTP/1.1\r\nHost: {address}\r\n\r\n",
                ));
            }
            socket = Some(tokio::net::TcpStream::connect(address).await.unwrap());
            socket.as_mut().unwrap().write_all(wire.as_bytes()).await.unwrap();
            signalled(&probe.entered).await;
            assert_eq!(probe.active.load(Ordering::SeqCst), 1);
            assert_eq!(*probe.values.lock().unwrap(), [json!(2)]);
            assert!(state.settings.lock_updates().now_or_never().is_none());
            assert_eq!(state.settings.get(VALUE).await, Some(json!(2)));
            let committed = state.modules.context.policy_store().current().await.unwrap();
            assert_eq!(committed.version, initial.version + 1);
            assert!(committed.policy.memory_enabled);

            if disconnect {
                drop(socket.take());
            } else {
                stop.cancel();
                tokio::time::timeout(Duration::from_secs(1), shutdown.wait())
                    .await
                    .expect("buffered Settings handler must not block runtime shutdown")
                    .unwrap();
                assert!(shutdown.is_finished());
            }
            tokio::time::timeout(Duration::from_secs(1), probe.dropped.acquire())
                .await
                .expect("the server apply future must actually drop")
                .unwrap()
                .forget();
            assert_eq!(probe.active.load(Ordering::SeqCst), 0);
            assert_eq!(probe.release.available_permits(), 0);
            assert_eq!(state.runtime_tasks.active(), 0);
            let gate = tokio::time::timeout(Duration::from_secs(1), state.settings.lock_updates())
                .await
                .expect("cancelled handler must release the Settings gate");
            drop(gate);
            assert_eq!(*probe.values.lock().unwrap(), [json!(2)]);
            assert_eq!(state.settings.get(VALUE).await, Some(json!(2)));
            assert_eq!(std::fs::read(&path).unwrap(), original);
            let after = state.modules.context.policy_store().current().await.unwrap();
            assert_eq!(after.version, committed.version);
            assert!(after.policy.memory_enabled);
            eprintln!(
                "[settings-apply-exit] disconnect={disconnect} apply_dropped=1 active_tasks=0 desired_preserved=true sql_version={} file_unchanged=true",
                after.version,
            );
            if disconnect {
                assert!(!stop.is_cancelled());
                assert!(!serving.is_finished());
                probe.release.add_permits(1);
                let retried = skills::response(
                    skills::client()
                        .patch(format!("http://{address}/api/v1/settings"))
                        .bearer_auth(&token)
                        .header("content-type", "application/json")
                        .body(body),
                    200,
                )
                .await;
                assert_eq!(retried["success"], true, "{retried}");
                assert!(retried["changed"].as_array().unwrap().is_empty());
                assert_eq!(*probe.values.lock().unwrap(), [json!(2), json!(2)]);
                let saved: BTreeMap<String, Value> =
                    toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                assert_eq!(saved.get(VALUE), Some(&json!(2)));
            } else {
                let _http = tokio::net::TcpListener::bind(address).await.unwrap();
                if let Some(address) = grpc {
                    let _grpc = tokio::net::TcpListener::bind(address).await.unwrap();
                }
            }
        })
        .catch_unwind()
        .await;

        // Also release the unfixed baseline after RED so its owned cleanup joins.
        probe.release.add_permits(1);
        drop(socket);
        stop.cancel();
        let cleanup = tokio::time::timeout(Duration::from_secs(5), shutdown.wait()).await;
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        if joined.is_err() {
            serving.abort();
            let _ = serving.await;
        }
        cleanup.expect("Settings cleanup must remain joinable").unwrap();
        joined.expect("Settings runtime owner must join").unwrap().unwrap();
        assert_eq!(probe.active.load(Ordering::SeqCst), 0);
        if let Err(error) = assertions {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}

#[tokio::test]
async fn shutdown_drops_pipelined_settings_apply_and_preserves_partial_outcome() {
    apply_exit(false).await;
}

#[tokio::test]
async fn client_disconnect_drops_apply_and_same_value_retry_completes() {
    apply_exit(true).await;
}

#[tokio::test]
async fn shutdown_releases_privacy_gate_while_waiting_for_shared_policy_lock() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let address = runtime.http_addr();
        let shutdown = runtime.shutdown_handle();
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let mut blocker = None;
        let mut socket = None;
        let assertions = AssertUnwindSafe(async {
            let (_, token) = delivery::account(&state, "admin").await;
            let initial = state.modules.context.policy_store().current().await.unwrap();
            assert!(!initial.policy.memory_enabled);
            state.settings.save().await.unwrap();
            let path = f.dir.join("settings.toml");
            let original = std::fs::read(&path).unwrap();
            blocker = Some(state.interaction_policies.write().await);
            let body = json!({"enabled": true}).to_string();
            let wire = format!(
                "PATCH /api/v1/memories/privacy HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len(),
            );
            socket = Some(tokio::net::TcpStream::connect(address).await.unwrap());
            socket.as_mut().unwrap().write_all(wire.as_bytes()).await.unwrap();
            // Privacy projects SQL into the registry immediately before awaiting
            // the policy lock held above. Observe this actual stage, not a delay.
            tokio::time::timeout(Duration::from_secs(1), async {
                while state.settings.get("core.policies.memory.enabled").await != Some(json!(true)) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("privacy request must commit and reach the shared policy lock");
            let committed = state.modules.context.policy_store().current().await.unwrap();
            assert_eq!(committed.version, initial.version + 1);
            assert!(committed.policy.memory_enabled);
            assert!(state.settings.lock_updates().now_or_never().is_none());
            assert!(!blocker.as_ref().unwrap().memory.enabled);

            stop.cancel();
            let gate = tokio::time::timeout(Duration::from_secs(1), state.settings.lock_updates())
                .await
                .expect("shutdown must drop the privacy handler and release update admission");
            // The policy write lock is still held: this handler could not have
            // completed normally and acquired the lock to apply its projection.
            assert!(!blocker.as_ref().unwrap().memory.enabled);
            assert_eq!(state.settings.get("core.policies.memory.enabled").await, Some(json!(true)));
            assert_eq!(std::fs::read(&path).unwrap(), original);
            let after = state.modules.context.policy_store().current().await.unwrap();
            assert_eq!(after.version, committed.version);
            assert!(after.policy.memory_enabled);
            drop(gate);
            blocker.take();
            tokio::time::timeout(Duration::from_secs(1), shutdown.wait())
                .await
                .expect("shared-lock Settings shutdown must join")
                .unwrap();
            assert!(shutdown.is_finished());
            assert_eq!(state.runtime_tasks.active(), 0);
            eprintln!("[settings-shared-lock] privacy_sql_preserved=true registry_projection_preserved=true file_unchanged=true update_gate_released=true active_tasks=0");
        })
        .catch_unwind()
        .await;

        blocker.take();
        drop(socket);
        stop.cancel();
        let cleanup = tokio::time::timeout(Duration::from_secs(5), shutdown.wait()).await;
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        if joined.is_err() {
            serving.abort();
            let _ = serving.await;
        }
        cleanup.expect("privacy cleanup must remain joinable").unwrap();
        joined.expect("privacy runtime owner must join").unwrap().unwrap();
        if let Err(error) = assertions {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}
