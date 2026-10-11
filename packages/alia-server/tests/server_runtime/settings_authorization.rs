//! Current account authority must survive waits before non-policy settings apply.
use super::*;
use alia_server::providers::adapter::{EmbeddingAdapter, EmbeddingProfile};
use sea_orm::{DatabaseBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const TOOL_LIMIT: &str = "core.policies.tools.max_tools_per_turn";

struct ApplyProbe(Arc<AtomicUsize>);

struct PhaseGate {
    stop_at: AtomicUsize,
    calls: AtomicUsize,
    reached: Semaphore,
    release: Semaphore,
}

impl Default for PhaseGate {
    fn default() -> Self {
        Self {
            stop_at: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            reached: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }
}

impl PhaseGate {
    fn arm(&self, call: usize) {
        self.calls.store(0, Ordering::SeqCst);
        self.stop_at.store(call, Ordering::SeqCst);
    }

    async fn enter(&self) {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.stop_at.load(Ordering::SeqCst) == call {
            self.reached.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
    }
}

#[derive(Default)]
struct GatedEmbedding {
    preview: Arc<PhaseGate>,
    apply: Arc<PhaseGate>,
}

#[async_trait]
impl ModuleLifecycle for GatedEmbedding {
    fn name(&self) -> &'static str {
        "settings-authorization-embedding"
    }

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        if updates.contains_key(TOOL_LIMIT) {
            self.apply.enter().await;
            return Ok(vec![TOOL_LIMIT.into()]);
        }
        Ok(Vec::new())
    }
}

#[async_trait]
impl EmbeddingAdapter for GatedEmbedding {
    async fn embed(&self, _texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        panic!("authorization fixture must not call an embedding service");
    }

    async fn preview_profile_settings(
        &self,
        _updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Option<EmbeddingProfile>> {
        self.preview.enter().await;
        Ok(None)
    }

    fn dimension(&self) -> usize {
        0
    }
}

#[async_trait]
impl ModuleLifecycle for ApplyProbe {
    fn name(&self) -> &'static str {
        "settings-authorization-probe"
    }

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        if updates.contains_key(TOOL_LIMIT) {
            self.0.fetch_add(1, Ordering::SeqCst);
            return Ok(vec![TOOL_LIMIT.into()]);
        }
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn non_policy_patch_rechecks_admin_after_waiting_for_policy() {
    with_fixture(|f| async move {
        let applies = Arc::new(AtomicUsize::new(0));
        let runtime = f
            .builder()
            .with_extension(ApplyProbe(applies.clone()))
            .build()
            .await
            .unwrap();
        let state = runtime.state().clone();
        let url = format!("http://{}/api/v1/settings", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let assertions = AssertUnwindSafe(async {
            let (actor, token) = delivery::account(&state, "admin").await;
            let desired_before = state.settings.get(TOOL_LIMIT).await;
            let live_before = state
                .interaction_policies
                .read()
                .await
                .tools
                .max_tools_per_turn;
            state.settings.save().await.unwrap();
            let path = f.dir.join("settings.toml");
            let file_before = std::fs::read(&path).unwrap();
            applies.store(0, Ordering::SeqCst);

            // Client construction may synchronously initialize local TLS or
            // proxy state. Keep it outside the bounded SQL witness interval.
            let client = skills::client();

            let barrier = state.db.begin().await.unwrap();
            let mut request = None;
            let gated = AssertUnwindSafe(async {
                barrier
                    .execute_unprepared("SELECT version FROM context_policy FOR UPDATE")
                    .await
                    .unwrap();
                let barrier_pid: i32 = barrier
                    .query_one(Statement::from_string(
                        DatabaseBackend::Postgres,
                        "SELECT pg_backend_pid() AS pid",
                    ))
                    .await
                    .unwrap()
                    .unwrap()
                    .try_get("", "pid")
                    .unwrap();
                let body = json!({(TOOL_LIMIT): live_before + 1});
                request = Some(tokio::spawn(async move {
                    let response = client
                        .patch(&url)
                        .bearer_auth(&token)
                        .json(&body)
                        .send()
                        .await
                        .unwrap();
                    let status = response.status();
                    let body = response.json::<Value>().await.ok();
                    (status, body)
                }));

                // Preserve production's 2 s policy deadline. Observe a real SQL
                // waiter, and prove this HTTP handler owns the settings gate so
                // an unrelated background policy read cannot satisfy the witness.
                let mut polls = 0;
                let mut last_counts = [0_i64; 6];
                let mut gate_held_seen = false;
                let mut request_finished_seen = false;
                let witness = tokio::time::timeout(Duration::from_secs(1), async {
                    loop {
                        let activity = f
                            .admin
                            .query_one(Statement::from_sql_and_values(
                                DatabaseBackend::Postgres,
                                "WITH activity AS (
                                   SELECT pid,state,wait_event_type,
                                     $1=ANY(pg_blocking_pids(pid)) AS blocked,
                                     query LIKE '%FROM context_policy WHERE singleton FOR UPDATE%' AS policy_read
                                   FROM pg_stat_activity
                                   WHERE datname=current_database() AND pid<>pg_backend_pid()
                                 ) SELECT
                                   count(*) FILTER (WHERE blocked) AS blocked,
                                   count(*) FILTER (WHERE blocked AND policy_read AND wait_event_type='Lock') AS policy_blocked,
                                   count(*) FILTER (WHERE policy_read) AS policy_reads,
                                   count(*) FILTER (WHERE policy_read AND state='active') AS policy_active,
                                   count(*) FILTER (WHERE policy_read AND wait_event_type='Lock') AS policy_locks,
                                   count(*) FILTER (WHERE pid=$1 AND state='idle in transaction') AS barrier_idle
                                 FROM activity",
                                [barrier_pid.into()],
                            ))
                            .await
                            .unwrap()
                            .unwrap();
                        for (index, column) in [
                            "blocked",
                            "policy_blocked",
                            "policy_reads",
                            "policy_active",
                            "policy_locks",
                            "barrier_idle",
                        ]
                        .iter()
                        .enumerate()
                        {
                            last_counts[index] = activity.try_get("", column).unwrap();
                        }
                        polls += 1;
                        let gate_held = state.settings.lock_updates().now_or_never().is_none();
                        gate_held_seen |= gate_held;
                        request_finished_seen |= request.as_ref().unwrap().is_finished();
                        if last_counts[1] > 0 && gate_held {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await;
                eprintln!(
                    "[settings-auth-witness] reached={} polls={polls} blocked={} policy_blocked={} policy_reads={} policy_active={} policy_locks={} barrier_idle={} gate_held_seen={gate_held_seen} request_finished_seen={request_finished_seen}",
                    witness.is_ok(),
                    last_counts[0],
                    last_counts[1],
                    last_counts[2],
                    last_counts[3],
                    last_counts[4],
                    last_counts[5],
                );
                witness.expect("authenticated PATCH did not reach the policy row barrier");

                // The barrier transaction owns a different pooled connection.
                // Commit the role change before letting the original PATCH run.
                let changed = state
                    .db
                    .execute(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres,
                        "UPDATE users SET role='user' WHERE id=$1 AND role='admin'",
                        [actor.into()],
                    ))
                    .await
                    .unwrap();
                assert_eq!(changed.rows_affected(), 1);
            })
            .catch_unwind()
            .await;

            let released = barrier.rollback().await;
            let response = if let Some(mut request) = request {
                let joined = tokio::time::timeout(Duration::from_secs(5), &mut request).await;
                if joined.is_err() {
                    request.abort();
                    let _ = request.await;
                }
                Some(
                    joined
                        .expect("PATCH must finish after the SQL barrier is released")
                        .unwrap(),
                )
            } else {
                None
            };
            released.unwrap();
            let (status, body) = response.expect("PATCH request was spawned");
            let error_code = match body
                .as_ref()
                .and_then(|body| body.pointer("/error/code"))
                .and_then(Value::as_str)
            {
                Some(code @ ("AUTH_INVALID_CREDENTIALS" | "AUTH_FORBIDDEN" | "NOT_FOUND"
                | "VALIDATION_ERROR" | "VERSION_CONFLICT" | "SERVICE_UNAVAILABLE"
                | "RATE_LIMITED" | "INTERNAL_DB_ERROR" | "INTERNAL_ERROR")) => code,
                Some(_) => "unrecognized",
                None => "none",
            };

            let desired_after = state.settings.get(TOOL_LIMIT).await;
            let live_after = state
                .interaction_policies
                .read()
                .await
                .tools
                .max_tools_per_turn;
            let file_changed = std::fs::read(&path).unwrap() != file_before;
            let apply_count = applies.load(Ordering::SeqCst);
            // Record only this controlled numeric setting and change flags. Never
            // print the settings file, credentials, JWT or raw response body.
            eprintln!(
                "[settings-auth-wait] status={} error_code={error_code} json_body={} desired_before={desired_before:?} desired_after={desired_after:?} live_before={live_before} live_after={live_after} file_changed={file_changed} module_applies={apply_count}",
                status.as_u16(),
                body.is_some(),
            );
            if let Err(error) = gated {
                std::panic::resume_unwind(error);
            }
            assert!(body.is_some(), "PATCH response must be JSON");
            assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
            assert_eq!(desired_after, desired_before);
            assert_eq!(live_after, live_before);
            assert!(!file_changed);
            assert_eq!(apply_count, 0);
        })
        .catch_unwind()
        .await;

        stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        if joined.is_err() {
            serving.abort();
            let _ = serving.await;
        }
        joined
            .expect("settings authorization HTTP runtime must join")
            .unwrap()
            .unwrap();
        if let Err(error) = assertions {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}

#[derive(Clone, Copy, Debug)]
enum AccountChange {
    Demote,
    Disable,
    Delete,
    LookupError,
    LookupTimeout,
    KeepAdmin,
    DemoteAfterAdmission,
}

async fn preview_case(preview_call: usize, change: AccountChange) {
    with_fixture(|f| async move {
        let preview = Arc::new(PhaseGate::default());
        let apply = Arc::new(PhaseGate::default());
        let runtime = f
            .builder()
            .with_embedding(GatedEmbedding {
                preview: preview.clone(),
                apply: apply.clone(),
            })
            .build()
            .await
            .unwrap();
        let state = runtime.state().clone();
        let url = format!("http://{}/api/v1/settings", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let mut request = None;
        let mut blocker = None;
        let mut renamed = false;
        let assertions = AssertUnwindSafe(async {
            let (actor, token) = delivery::account(&state, "admin").await;
            let client = skills::client();
            let desired_before = state.settings.get(TOOL_LIMIT).await;
            let live_before = state.interaction_policies.read().await.tools.max_tools_per_turn;
            let policy_before = state.modules.context.policy_store().current().await.unwrap();
            state.settings.save().await.unwrap();
            let path = f.dir.join("settings.toml");
            let file_before = std::fs::read(&path).unwrap();
            apply.arm(0);
            let gate = if matches!(change, AccountChange::DemoteAfterAdmission) {
                apply.arm(1);
                &apply
            } else {
                preview.arm(preview_call);
                &preview
            };
            let body = json!({(TOOL_LIMIT): live_before + 1});
            let outgoing = client.patch(&url).bearer_auth(&token).json(&body);
            request = Some(tokio::spawn(async move {
                let response = outgoing.send().await.unwrap();
                let status = response.status();
                (status, response.json::<Value>().await.unwrap())
            }));
            signalled(&gate.reached).await;
            assert!(!request.as_ref().unwrap().is_finished());
            assert!(state.settings.lock_updates().now_or_never().is_none());

            match change {
                AccountChange::Demote | AccountChange::DemoteAfterAdmission => {
                    let changed = state.db.execute(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres,
                        "UPDATE users SET role='user' WHERE id=$1 AND role='admin'",
                        [actor.into()],
                    )).await.unwrap();
                    assert_eq!(changed.rows_affected(), 1);
                }
                AccountChange::Disable => {
                    let changed = state.db.execute(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres,
                        "UPDATE users SET status='disabled' WHERE id=$1 AND status='active'",
                        [actor.into()],
                    )).await.unwrap();
                    assert_eq!(changed.rows_affected(), 1);
                }
                AccountChange::Delete => {
                    let changed = state.db.execute(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres,
                        "DELETE FROM users WHERE id=$1",
                        [actor.into()],
                    )).await.unwrap();
                    assert_eq!(changed.rows_affected(), 1);
                }
                AccountChange::LookupError => {
                    // Entry authentication and both previews have already run.
                    state.db.execute_unprepared("ALTER TABLE users RENAME TO settings_auth_users").await.unwrap();
                    renamed = true;
                }
                AccountChange::LookupTimeout => {
                    blocker = Some(state.db.begin().await.unwrap());
                    blocker.as_ref().unwrap().execute_unprepared("LOCK TABLE users IN ACCESS EXCLUSIVE MODE").await.unwrap();
                }
                AccountChange::KeepAdmin => {}
            }
            let released_at = Instant::now();
            gate.release.add_permits(1);
            if let Some(barrier) = &blocker {
                let barrier_pid: i32 = barrier.query_one(Statement::from_string(
                    DatabaseBackend::Postgres, "SELECT pg_backend_pid() AS pid",
                )).await.unwrap().unwrap().try_get("", "pid").unwrap();
                tokio::time::timeout(Duration::from_secs(1), async {
                    loop {
                        let waiting: i64 = f.admin.query_one(Statement::from_sql_and_values(
                            DatabaseBackend::Postgres,
                            "SELECT count(*) AS n FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND wait_event_type='Lock' AND query LIKE '%FROM \"users\"%'",
                            [barrier_pid.into()],
                        )).await.unwrap().unwrap().try_get("", "n").unwrap();
                        if waiting > 0 { break; }
                        tokio::task::yield_now().await;
                    }
                }).await.expect("fresh account lookup must wait on the users table lock");
            }
            let joined = tokio::time::timeout(Duration::from_secs(5), request.as_mut().unwrap()).await;
            if joined.is_ok() {
                // Cleanup must never poll a completed JoinHandle after an inner panic.
                request.take();
            }
            let (status, response) = joined.expect("PATCH must finish within its authorization deadline").unwrap();
            let elapsed = released_at.elapsed();
            if let Some(barrier) = blocker.take() {
                barrier.rollback().await.unwrap();
            }
            if renamed {
                state.db.execute_unprepared("ALTER TABLE settings_auth_users RENAME TO users").await.unwrap();
                renamed = false;
            }
            let policy_after = state.modules.context.policy_store().current().await.unwrap();
            assert_eq!(policy_after.version, policy_before.version, "a non-policy PATCH must not perform policy CAS");
            assert_eq!(policy_after.policy, policy_before.policy);
            let approved = matches!(change, AccountChange::KeepAdmin | AccountChange::DemoteAfterAdmission);
            let desired_after = state.settings.get(TOOL_LIMIT).await;
            let live_after = state.interaction_policies.read().await.tools.max_tools_per_turn;
            let file_changed = std::fs::read(&path).unwrap() != file_before;
            let apply_count = apply.calls.load(Ordering::SeqCst);
            eprintln!("[settings-auth-preview] call={preview_call} change={change:?} status={} elapsed_ms={} desired_changed={} live_changed={} file_changed={file_changed} module_applies={apply_count}",
                status.as_u16(), elapsed.as_millis(), desired_after != desired_before, live_after != live_before);
            if approved {
                assert_eq!(status, reqwest::StatusCode::OK);
                assert_eq!(response["success"], true);
                assert_eq!(response["context_policy_version"], policy_before.version);
                assert_eq!(desired_after, Some(json!(live_before + 1)));
                assert_eq!(live_after, live_before + 1);
                assert!(file_changed);
                assert_eq!(apply_count, 1);
                if matches!(change, AccountChange::KeepAdmin) {
                    let retry = skills::response(client.patch(&url).bearer_auth(&token).json(&body), 200).await;
                    assert_eq!(retry["success"], true);
                    assert_eq!(retry["changed"], json!([]));
                    assert_eq!(retry["context_policy_version"], policy_before.version);
                    assert_eq!(apply.calls.load(Ordering::SeqCst), 2);
                    let policy_retried = state.modules.context.policy_store().current().await.unwrap();
                    assert_eq!(policy_retried.version, policy_before.version);
                    assert_eq!(policy_retried.policy, policy_before.policy);
                }
            } else {
                let unavailable = matches!(change, AccountChange::LookupError | AccountChange::LookupTimeout);
                assert_eq!(status, if unavailable { reqwest::StatusCode::SERVICE_UNAVAILABLE } else { reqwest::StatusCode::FORBIDDEN });
                assert_eq!(response["error"]["code"], if unavailable { "SERVICE_UNAVAILABLE" } else { "AUTH_FORBIDDEN" });
                assert_eq!(desired_after, desired_before);
                assert_eq!(live_after, live_before);
                assert!(!file_changed);
                assert_eq!(apply_count, 0);
                if matches!(change, AccountChange::LookupTimeout) {
                    assert!(elapsed >= Duration::from_millis(1900), "lookup returned before its 2 s deadline");
                }
                if unavailable {
                    let message = response["error"]["message"].as_str().unwrap();
                    assert!(!message.contains(&f.schema));
                    assert!(!message.contains("users"));
                    assert!(!message.contains(&actor.to_string()));
                }
            }
        }).catch_unwind().await;

        preview.release.add_permits(1);
        apply.release.add_permits(1);
        if let Some(barrier) = blocker.take() {
            barrier.rollback().await.unwrap();
        }
        if renamed {
            state.db.execute_unprepared("ALTER TABLE settings_auth_users RENAME TO users").await.unwrap();
        }
        if let Some(mut request) = request.take() {
            if tokio::time::timeout(Duration::from_secs(5), &mut request).await.is_err() {
                request.abort();
                let _ = request.await;
            }
        }
        stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        if joined.is_err() {
            serving.abort();
            let _ = serving.await;
        }
        joined.expect("authorization runtime must join").unwrap().unwrap();
        if let Err(error) = assertions {
            std::panic::resume_unwind(error);
        }
    }).await;
}

#[tokio::test]
async fn demotion_during_first_preview_prevents_non_policy_settings() {
    preview_case(1, AccountChange::Demote).await;
}

#[tokio::test]
async fn disabling_during_second_preview_prevents_non_policy_settings() {
    preview_case(2, AccountChange::Disable).await;
}

#[tokio::test]
async fn deletion_during_second_preview_prevents_non_policy_settings() {
    preview_case(2, AccountChange::Delete).await;
}

#[tokio::test]
async fn fresh_account_lookup_error_is_unavailable_without_settings_changes() {
    preview_case(2, AccountChange::LookupError).await;
}

#[tokio::test]
async fn fresh_account_lookup_timeout_is_bounded_without_settings_changes() {
    preview_case(2, AccountChange::LookupTimeout).await;
}

#[tokio::test]
async fn active_admin_patch_and_same_value_retry_do_not_advance_policy() {
    preview_case(2, AccountChange::KeepAdmin).await;
}

#[tokio::test]
async fn demotion_after_apply_admission_does_not_claim_cross_module_rollback() {
    preview_case(0, AccountChange::DemoteAfterAdmission).await;
}
