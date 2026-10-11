//! Delayed settings reads must not expose credentials to revoked administrators.
use super::*;
use sea_orm::{DatabaseBackend, Statement, TransactionTrait};
use serde_json::Value;

const SECRET_KEY: &str = "core.providers.llm.openai.api_key";

#[derive(Clone, Copy, Debug)]
enum ReadChange {
    Demote,
    Disable,
    Delete,
    LookupError,
    LookupTimeout,
    KeepAdmin,
}

async fn read_case(change: ReadChange) {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let url = format!("http://{}/api/v1/settings", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let mut request = None;
        let mut blocker = None;
        let mut account_blocker = None;
        let mut renamed = false;
        let assertions = AssertUnwindSafe(async {
            let (actor, token) = delivery::account(&state, "admin").await;
            let client = skills::client();
            let sentinel = format!("nonfunctional-settings-read-fixture-{}", Uuid::new_v4());
            state.settings.set(SECRET_KEY, Value::String(sentinel.clone())).await;
            assert!(state.settings.get_entry(SECRET_KEY).await.unwrap().schema.unwrap().secret);
            state.settings.save().await.unwrap();
            let path = f.dir.join("settings.toml");
            let file_before = std::fs::read(&path).unwrap();
            let policy_before = state.modules.context.policy_store().current().await.unwrap();

            blocker = Some(state.db.begin().await.unwrap());
            let barrier = blocker.as_ref().unwrap();
            barrier.execute_unprepared("SELECT version FROM context_policy FOR UPDATE").await.unwrap();
            let barrier_pid: i32 = barrier.query_one(Statement::from_string(
                DatabaseBackend::Postgres, "SELECT pg_backend_pid() AS pid",
            )).await.unwrap().unwrap().try_get("", "pid").unwrap();
            let outgoing = client.get(&url).bearer_auth(&token);
            request = Some(tokio::spawn(async move {
                let response = outgoing.send().await.unwrap();
                let status = response.status();
                let etag = response.headers().get("etag").map(|value| value.to_str().unwrap().to_owned());
                let response = response.json::<Value>().await.unwrap();
                (status, etag, response)
            }));
            let mut polls = 0;
            let mut policy_waiters = 0_i64;
            let mut gate_held = false;
            let witness = tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    policy_waiters = f.admin.query_one(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres,
                        "SELECT count(*) AS n FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND wait_event_type='Lock' AND query LIKE '%FROM context_policy WHERE singleton FOR UPDATE%'",
                        [barrier_pid.into()],
                    )).await.unwrap().unwrap().try_get("", "n").unwrap();
                    gate_held = state.settings.lock_updates().now_or_never().is_none();
                    polls += 1;
                    if policy_waiters > 0 && gate_held { break; }
                    tokio::task::yield_now().await;
                }
            }).await;
            eprintln!("[settings-read-witness] reached={} polls={polls} policy_waiters={policy_waiters} gate_held={gate_held} request_finished={}", witness.is_ok(), request.as_ref().unwrap().is_finished());
            witness.expect("authenticated GET must reach its policy wait while holding the update gate");
            match change {
                ReadChange::Demote | ReadChange::Disable | ReadChange::Delete => {
                    let sql = match change {
                        ReadChange::Demote => "UPDATE users SET role='user' WHERE id=$1 AND role='admin'",
                        ReadChange::Disable => "UPDATE users SET status='disabled' WHERE id=$1 AND status='active'",
                        ReadChange::Delete => "DELETE FROM users WHERE id=$1",
                        _ => unreachable!(),
                    };
                    let changed = state.db.execute(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres, sql, [actor.into()],
                    )).await.unwrap();
                    assert_eq!(changed.rows_affected(), 1);
                }
                ReadChange::LookupError => {
                    state.db.execute_unprepared("ALTER TABLE users RENAME TO settings_read_users").await.unwrap();
                    renamed = true;
                }
                ReadChange::LookupTimeout => {
                    account_blocker = Some(state.db.begin().await.unwrap());
                    account_blocker.as_ref().unwrap().execute_unprepared("LOCK TABLE users IN ACCESS EXCLUSIVE MODE").await.unwrap();
                }
                ReadChange::KeepAdmin => {}
            }
            blocker.take().unwrap().rollback().await.unwrap();
            let released_at = Instant::now();
            if let Some(account_barrier) = &account_blocker {
                let account_pid: i32 = account_barrier.query_one(Statement::from_string(
                    DatabaseBackend::Postgres, "SELECT pg_backend_pid() AS pid",
                )).await.unwrap().unwrap().try_get("", "pid").unwrap();
                tokio::time::timeout(Duration::from_secs(1), async {
                    loop {
                        let waiting: i64 = f.admin.query_one(Statement::from_sql_and_values(
                            DatabaseBackend::Postgres,
                            "SELECT count(*) AS n FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND wait_event_type='Lock' AND query LIKE '%FROM \"users\"%'",
                            [account_pid.into()],
                        )).await.unwrap().unwrap().try_get("", "n").unwrap();
                        if waiting > 0 { break; }
                        tokio::task::yield_now().await;
                    }
                }).await.expect("final settings read authorization must reach its users table lock");
            }
            let joined = tokio::time::timeout(Duration::from_secs(5), request.as_mut().unwrap()).await;
            if joined.is_ok() { request.take(); }
            let (status, etag, response) = joined.expect("GET must finish after policy barrier release").unwrap();
            let elapsed = released_at.elapsed();
            if let Some(account_barrier) = account_blocker.take() {
                account_barrier.rollback().await.unwrap();
            }
            if renamed {
                state.db.execute_unprepared("ALTER TABLE settings_read_users RENAME TO users").await.unwrap();
                renamed = false;
            }
            let exposed = response.to_string().contains(&sentinel);
            let file_changed = std::fs::read(&path).unwrap() != file_before;
            let policy_after = state.modules.context.policy_store().current().await.unwrap();
            eprintln!("[settings-read-result] change={change:?} status={} elapsed_ms={} has_etag={} sentinel_exposed={exposed} file_changed={file_changed}", status.as_u16(), elapsed.as_millis(), etag.is_some());
            assert!(!file_changed);
            assert_eq!(policy_after.version, policy_before.version);
            assert_eq!(policy_after.policy, policy_before.policy);

            if matches!(change, ReadChange::KeepAdmin) {
                assert_eq!(status, reqwest::StatusCode::OK);
                assert!(exposed, "authorized GET must retain the existing unmasked secret API");
                let expected_tag = format!("\"{}\"", policy_before.version);
                assert_eq!(etag.as_deref(), Some(expected_tag.as_str()));
                assert!(response[SECRET_KEY]["schema"]["secret"] == true);
                assert!(response[SECRET_KEY]["schema"]["type"] == "secret");
                // Compare both complete maps across the same JSON wire roundtrip.
                let expected: Value = serde_json::from_slice(&serde_json::to_vec(&state.settings.all().await).unwrap()).unwrap();
                assert!(response == expected, "authorized GET must retain the full entry JSON contract");
                for prefix in ["core.providers.llm.openai", "core.providers.llm.openai.", SECRET_KEY, "", "fixture.unknown"] {
                    let filtered = client.get(&url).bearer_auth(&token).query(&[("prefix", prefix)]).send().await.unwrap();
                    assert_eq!(filtered.status(), reqwest::StatusCode::OK);
                    assert!(filtered.headers().get("etag").is_some_and(|value| value == expected_tag.as_str()));
                    let filtered = filtered.json::<Value>().await.unwrap();
                    let expected: Value = serde_json::from_slice(&serde_json::to_vec(&state.settings.list(prefix).await).unwrap()).unwrap();
                    assert!(filtered == expected, "prefix filtering must retain the existing JSON contract");
                }
                let (_, user) = delivery::account(&state, "user").await;
                for (outgoing, expected_status, expected_code) in [
                    (client.get(&url), reqwest::StatusCode::UNAUTHORIZED, "AUTH_INVALID_CREDENTIALS"),
                    (client.get(&url).bearer_auth(&user), reqwest::StatusCode::FORBIDDEN, "AUTH_FORBIDDEN"),
                ] {
                    let denied = outgoing.send().await.unwrap();
                    assert_eq!(denied.status(), expected_status);
                    assert!(!denied.headers().contains_key("etag"));
                    let denied = denied.json::<Value>().await.unwrap();
                    assert!(denied["error"]["code"] == expected_code);
                    assert!(!denied.to_string().contains(&sentinel));
                    assert_eq!(denied.as_object().unwrap().len(), 1);
                }
                assert!(std::fs::read(&path).unwrap() == file_before, "GET must not persist settings");
                assert_eq!(state.modules.context.policy_store().current().await.unwrap().version, policy_before.version);
            } else {
                let unavailable = matches!(change, ReadChange::LookupError | ReadChange::LookupTimeout);
                assert_eq!(status, if unavailable { reqwest::StatusCode::SERVICE_UNAVAILABLE } else { reqwest::StatusCode::FORBIDDEN });
                let expected_code = if unavailable { "SERVICE_UNAVAILABLE" } else { "AUTH_FORBIDDEN" };
                assert!(response["error"]["code"] == expected_code, "expected existing error envelope");
                assert_eq!(response.as_object().unwrap().len(), 1);
                assert!(etag.is_none());
                assert!(!exposed, "unapproved actor received the controlled secret sentinel");
                if unavailable {
                    let message = response["error"]["message"].as_str().unwrap();
                    assert!(!message.contains(&f.schema));
                    assert!(!message.contains("users"));
                    assert!(!message.contains(&actor.to_string()));
                }
                if matches!(change, ReadChange::LookupTimeout) {
                    assert!(elapsed >= Duration::from_millis(1900), "account lookup must reach its 2 s deadline");
                }
            }
        }).catch_unwind().await;

        if let Some(barrier) = blocker.take() {
            barrier.rollback().await.unwrap();
        }
        if let Some(account_barrier) = account_blocker.take() {
            account_barrier.rollback().await.unwrap();
        }
        if renamed {
            state.db.execute_unprepared("ALTER TABLE settings_read_users RENAME TO users").await.unwrap();
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
        joined.expect("settings read runtime must join").unwrap().unwrap();
        if let Err(error) = assertions {
            std::panic::resume_unwind(error);
        }
    }).await;
}

#[tokio::test]
async fn settings_get_rechecks_admin_after_policy_wait_before_returning_secrets() {
    read_case(ReadChange::Demote).await;
}

#[tokio::test]
async fn settings_get_rejects_disabled_account_after_policy_wait() {
    read_case(ReadChange::Disable).await;
}

#[tokio::test]
async fn settings_get_rejects_deleted_account_after_policy_wait() {
    read_case(ReadChange::Delete).await;
}

#[tokio::test]
async fn settings_get_account_lookup_error_returns_no_settings_or_etag() {
    read_case(ReadChange::LookupError).await;
}

#[tokio::test]
async fn settings_get_account_lookup_timeout_is_bounded_and_returns_no_settings_or_etag() {
    read_case(ReadChange::LookupTimeout).await;
}

#[tokio::test]
async fn settings_get_preserves_admin_json_secret_prefix_and_etag_contracts() {
    read_case(ReadChange::KeepAdmin).await;
}
