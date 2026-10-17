//! Plugin management and personal data use distinct, current HTTP authority.
use super::*;
use alia_server::plugins::storage::PluginStorage;
use alia_server::plugins::{PluginStorageManifest, StorageScope};
use alia_server::state::AppState;
use sea_orm::{DatabaseBackend, Statement, TransactionTrait};
use serde_json::{Value, json};

const PLUGIN: &str = "http-authorization-fixture";
const NEIGHBOR: &str = "http-authorization-neighbor";
const OTHER_VALUE: &str = "synthetic-other-owner-plugin-value";

async fn seed(state: &AppState, owner: Uuid, other: Uuid) {
    let manifest: PluginStorageManifest = toml::from_str(
        r#"
        [[collections]]
        name = "settings"
        scope = ["global", "user", "channel", "conversation"]
        [[collections]]
        name = "cache"
        scope = ["user"]
        "#,
    )
    .unwrap();
    let storage = state
        .plugin_storage
        .scoped_with_manifest(PLUGIN, manifest.clone())
        .unwrap();
    for (scope, value) in [
        (StorageScope::User(owner.to_string()), "synthetic-own-value"),
        (StorageScope::User(other.to_string()), OTHER_VALUE),
        (StorageScope::Global, "synthetic-global-value"),
        (
            StorageScope::Channel(owner.to_string()),
            "synthetic-channel-value",
        ),
        (
            StorageScope::Conversation(owner.to_string()),
            "synthetic-conversation-value",
        ),
    ] {
        storage
            .put("settings", "record", scope, json!({"fixture": value}))
            .await
            .unwrap();
    }
    for user in [owner, other] {
        storage
            .put(
                "cache",
                "record",
                StorageScope::User(user.to_string()),
                json!({"fixture": "synthetic-cache-value"}),
            )
            .await
            .unwrap();
    }
    storage
        .put(
            "cache",
            "expired",
            StorageScope::User(owner.to_string()),
            json!({"fixture": "synthetic-expired-value"}),
        )
        .await
        .unwrap();
    state.db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE plugin_storage_records SET ttl_expires_at=NOW()-INTERVAL '1 second' WHERE plugin_name=$1 AND key='expired'",
        [PLUGIN.into()],
    )).await.unwrap();
    state
        .plugin_storage
        .scoped_with_manifest(NEIGHBOR, manifest)
        .unwrap()
        .put(
            "settings",
            "record",
            StorageScope::User(owner.to_string()),
            json!({"fixture": "synthetic-neighbor-value"}),
        )
        .await
        .unwrap();
}

async fn snapshot(state: &AppState) -> Value {
    state.db.query_one(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY plugin_name,scope,owner_id,collection,key),'[]'::jsonb) AS records FROM plugin_storage_records r WHERE plugin_name IN ($1,$2)",
        [PLUGIN.into(), NEIGHBOR.into()],
    )).await.unwrap().unwrap().try_get("", "records").unwrap()
}

async fn ordinary_user_management_case(purge: bool) {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let base = format!("http://{}/api/v1/plugins/{PLUGIN}/storage", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let assertions = AssertUnwindSafe(async {
            let (owner, token) = delivery::account(&state, "user").await;
            let (other, _) = delivery::account(&state, "user").await;
            seed(&state, owner, other).await;
            let before = snapshot(&state).await;
            let client = skills::client();
            let outgoing = if purge { client.delete(&base) } else { client.get(format!("{base}/export")) };
            let response = outgoing.bearer_auth(&token).send().await.unwrap();
            let status = response.status();
            let body = response.json::<Value>().await.unwrap();
            let after = snapshot(&state).await;
            let exposed_other = body.to_string().contains(OTHER_VALUE);
            let changed = before != after;
            eprintln!("[plugin-authorization] purge={purge} status={} exposed_other={exposed_other} changed={changed} records_before={} records_after={}", status.as_u16(), before.as_array().unwrap().len(), after.as_array().unwrap().len());
            assert_eq!(status, reqwest::StatusCode::FORBIDDEN, "ordinary user reached global plugin management: exposed_other={exposed_other}, changed={changed}");
            assert_eq!(body["error"]["code"], "AUTH_FORBIDDEN");
            assert!(!exposed_other, "denied export must not return another owner's value");
            assert_eq!(after, before, "denied management must preserve every scope and namespace");
        }).catch_unwind().await;
        stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        if joined.is_err() { serving.abort(); let _ = serving.await; }
        joined.expect("plugin authorization runtime must join").unwrap().unwrap();
        if let Err(error) = assertions { std::panic::resume_unwind(error); }
    }).await;
}

#[tokio::test]
async fn ordinary_user_cannot_export_all_plugin_owners() {
    ordinary_user_management_case(false).await;
}

#[tokio::test]
async fn ordinary_user_cannot_purge_all_plugin_owners() {
    ordinary_user_management_case(true).await;
}

#[tokio::test]
async fn administrator_retains_global_management_contract() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let base = format!("http://{}/api/v1/plugins", runtime.http_addr());
        let storage = format!("{base}/{PLUGIN}/storage");
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let assertions = AssertUnwindSafe(async {
            let (_, admin) = delivery::account(&state, "admin").await;
            let (owner, user) = delivery::account(&state, "user").await;
            let (other, _) = delivery::account(&state, "user").await;
            seed(&state, owner, other).await;
            let before = snapshot(&state).await;
            let client = skills::client();
            for endpoint in [&base, &storage] {
                let denied = skills::response(client.get(endpoint).bearer_auth(&user), 403).await;
                assert_eq!(denied["error"]["code"], "AUTH_FORBIDDEN");
                let unauthenticated = skills::response(client.get(endpoint), 401).await;
                assert_eq!(unauthenticated["error"]["code"], "AUTH_INVALID_CREDENTIALS");
            }
            let expected_usage =
                serde_json::to_value(state.plugin_storage.usage_for_plugin(PLUGIN).await.unwrap())
                    .unwrap();
            let usage = skills::response(client.get(&storage).bearer_auth(&admin), 200).await;
            assert_eq!(usage, expected_usage);
            assert_eq!(usage["record_count"], 8);
            assert_eq!(usage["expired_record_count"], 1);
            let plugins = skills::response(client.get(&base).bearer_auth(&admin), 200).await;
            let listed = plugins
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["name"] == PLUGIN)
                .unwrap();
            assert_eq!(listed["storage"], expected_usage);
            assert_eq!(listed["state"], "registered");
            assert_eq!(listed["capabilities"], json!(["storage.records"]));
            let export = skills::response(
                client.get(format!("{storage}/export")).bearer_auth(&admin),
                200,
            )
            .await;
            assert_eq!(export["plugin_name"], PLUGIN);
            assert!(
                chrono::DateTime::parse_from_rfc3339(export["exported_at"].as_str().unwrap())
                    .is_ok()
            );
            assert_eq!(
                export["records"],
                serde_json::to_value(
                    state
                        .plugin_storage
                        .export_plugin_records(PLUGIN)
                        .await
                        .unwrap()
                )
                .unwrap()
            );
            assert_eq!(export["records"].as_array().unwrap().len(), 7);
            assert!(export.to_string().contains(OTHER_VALUE));
            assert!(!export.to_string().contains("synthetic-expired-value"));
            assert_eq!(
                snapshot(&state).await,
                before,
                "management reads and denials must not mutate storage"
            );

            let purge = skills::response(
                client
                    .delete(&storage)
                    .bearer_auth(&admin)
                    .query(&[("collection", "cache")]),
                200,
            )
            .await;
            assert_eq!(
                purge,
                json!({"plugin_name": PLUGIN, "collection": "cache", "deleted_records": 3})
            );
            let expected: Vec<_> = before
                .as_array()
                .unwrap()
                .iter()
                .filter(|record| {
                    !(record["plugin_name"] == PLUGIN && record["collection"] == "cache")
                })
                .cloned()
                .collect();
            assert_eq!(snapshot(&state).await, json!(expected));
            let purge = skills::response(client.delete(&storage).bearer_auth(&admin), 200).await;
            assert_eq!(
                purge,
                json!({"plugin_name": PLUGIN, "collection": null, "deleted_records": 5})
            );
            let expected: Vec<_> = before
                .as_array()
                .unwrap()
                .iter()
                .filter(|record| record["plugin_name"] == NEIGHBOR)
                .cloned()
                .collect();
            assert_eq!(snapshot(&state).await, json!(expected));
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
            .expect("plugin authorization runtime must join")
            .unwrap()
            .unwrap();
        if let Err(error) = assertions {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}

#[tokio::test]
async fn personal_storage_is_bound_to_authenticated_user_and_scope() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let base = format!(
            "http://{}/api/v1/plugins/{PLUGIN}/storage/me",
            runtime.http_addr()
        );
        let export_url = format!("{base}/export");
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let assertions = AssertUnwindSafe(async {
            let (owner, token) = delivery::account(&state, "user").await;
            let (other, other_token) = delivery::account(&state, "user").await;
            seed(&state, owner, other).await;
            let before = snapshot(&state).await;
            let client = skills::client();
            let usage = skills::response(client.get(&base).bearer_auth(&token), 200).await;
            assert_eq!(usage["plugin_name"], PLUGIN);
            assert_eq!(usage["record_count"], 3);
            assert_eq!(usage["expired_record_count"], 1);
            for (actor, authorization) in [(owner, &token), (other, &other_token)] {
                let export =
                    skills::response(client.get(&export_url).bearer_auth(authorization), 200).await;
                assert_eq!(export["plugin_name"], PLUGIN);
                assert!(
                    chrono::DateTime::parse_from_rfc3339(export["exported_at"].as_str().unwrap())
                        .is_ok()
                );
                let records = export["records"].as_array().unwrap();
                assert_eq!(records.len(), 2);
                assert!(records.iter().all(|record| record["scope"] == "user"
                    && record["owner_id"] == actor.to_string()
                    && record["plugin_name"] == PLUGIN));
                assert!(!export.to_string().contains("synthetic-expired-value"));
                if actor == owner {
                    assert!(!export.to_string().contains(OTHER_VALUE));
                }
            }
            // Even an owner UUID matching a channel/conversation identifier
            // never admits those scopes through the personal-user endpoint.
            for (key, value) in [
                ("owner_id", other.to_string()),
                ("scope", "global".to_owned()),
            ] {
                for outgoing in [
                    client.get(&base),
                    client.get(&export_url),
                    client.delete(&base),
                ] {
                    let denied = outgoing
                        .bearer_auth(&token)
                        .query(&[(key, value.as_str())])
                        .send()
                        .await
                        .unwrap();
                    assert_eq!(denied.status(), reqwest::StatusCode::BAD_REQUEST);
                    assert!(!denied.text().await.unwrap().contains(OTHER_VALUE));
                }
            }
            assert_eq!(
                snapshot(&state).await,
                before,
                "forged owner/scope must leave all records unchanged"
            );
            let purge = skills::response(
                client
                    .delete(&base)
                    .bearer_auth(&token)
                    .query(&[("collection", "cache")]),
                200,
            )
            .await;
            assert_eq!(
                purge,
                json!({"plugin_name": PLUGIN, "collection": "cache", "deleted_records": 2})
            );
            let owns = |record: &Value| {
                record["plugin_name"] == PLUGIN
                    && record["scope"] == "user"
                    && record["owner_id"] == owner.to_string()
            };
            let expected: Vec<_> = before
                .as_array()
                .unwrap()
                .iter()
                .filter(|record| !(owns(record) && record["collection"] == "cache"))
                .cloned()
                .collect();
            assert_eq!(snapshot(&state).await, json!(expected));
            let purge = skills::response(client.delete(&base).bearer_auth(&token), 200).await;
            assert_eq!(
                purge,
                json!({"plugin_name": PLUGIN, "collection": null, "deleted_records": 1})
            );
            let expected: Vec<_> = before
                .as_array()
                .unwrap()
                .iter()
                .filter(|record| !owns(record))
                .cloned()
                .collect();
            assert_eq!(
                snapshot(&state).await,
                json!(expected),
                "personal purge must preserve other owners, non-user scopes and other plugins"
            );
            let remaining =
                skills::response(client.get(&export_url).bearer_auth(&other_token), 200).await;
            assert_eq!(remaining["records"].as_array().unwrap().len(), 2);
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
            .expect("plugin authorization runtime must join")
            .unwrap()
            .unwrap();
        if let Err(error) = assertions {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}

#[derive(Clone, Copy, Debug)]
enum WaitingRequest {
    AdminExport,
    AdminPurge,
    PersonalPurge,
}

async fn revoked_after_storage_wait(case: WaitingRequest) {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let base = format!("http://{}/api/v1/plugins/{PLUGIN}/storage", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let mut blocker = None;
        let mut request = None;
        let assertions = AssertUnwindSafe(async {
            let personal = matches!(case, WaitingRequest::PersonalPurge);
            let (actor, token) = delivery::account(&state, if personal { "user" } else { "admin" }).await;
            let (other, _) = delivery::account(&state, "user").await;
            seed(&state, actor, other).await;
            let before = snapshot(&state).await;
            let client = skills::client();
            blocker = Some(state.db.begin().await.unwrap());
            let barrier = blocker.as_ref().unwrap();
            barrier.execute_unprepared("LOCK TABLE plugin_storage_records IN ACCESS EXCLUSIVE MODE").await.unwrap();
            let blocker_pid: i32 = barrier.query_one(Statement::from_string(DatabaseBackend::Postgres, "SELECT pg_backend_pid() AS pid")).await.unwrap().unwrap().try_get("", "pid").unwrap();
            let outgoing = match case {
                WaitingRequest::AdminExport => client.get(format!("{base}/export")),
                WaitingRequest::AdminPurge => client.delete(&base),
                WaitingRequest::PersonalPurge => client.delete(format!("{base}/me")),
            }.bearer_auth(&token);
            request = Some(tokio::spawn(async move {
                let response = outgoing.send().await.unwrap();
                let status = response.status();
                (status, response.json::<Value>().await.unwrap())
            }));
            let witness = tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    let waiting: i64 = f.admin.query_one(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres,
                        "SELECT count(*) AS n FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND wait_event_type='Lock' AND query LIKE '%plugin_storage_records%'",
                        [blocker_pid.into()],
                    )).await.unwrap().unwrap().try_get("", "n").unwrap();
                    if waiting > 0 { break; }
                    tokio::task::yield_now().await;
                }
            }).await;
            eprintln!("[plugin-authorization-wait] case={case:?} observed={} request_finished={}", witness.is_ok(), request.as_ref().unwrap().is_finished());
            witness.expect("authenticated plugin request must reach actual storage SQL wait");
            assert!(!request.as_ref().unwrap().is_finished());
            let sql = if personal { "UPDATE users SET status='disabled' WHERE id=$1 AND status='active'" } else { "UPDATE users SET role='user' WHERE id=$1 AND role='admin'" };
            assert_eq!(state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, [actor.into()])).await.unwrap().rows_affected(), 1);
            blocker.take().unwrap().rollback().await.unwrap();
            let joined = tokio::time::timeout(Duration::from_secs(5), request.as_mut().unwrap()).await;
            if joined.is_ok() { request.take(); }
            let (status, body) = joined.expect("plugin request must finish after lock release").unwrap();
            let after = snapshot(&state).await;
            let exposed_other = body.to_string().contains(OTHER_VALUE);
            eprintln!("[plugin-authorization-revoked] case={case:?} status={} exposed_other={exposed_other} changed={}", status.as_u16(), before != after);
            assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
            assert_eq!(body["error"]["code"], "AUTH_FORBIDDEN");
            assert!(!exposed_other);
            assert_eq!(after, before, "revoked request must not commit deletion of any scope");
        }).catch_unwind().await;
        if let Some(barrier) = blocker.take() { barrier.rollback().await.unwrap(); }
        if let Some(mut pending) = request.take() {
            if tokio::time::timeout(Duration::from_secs(5), &mut pending).await.is_err() {
                pending.abort();
                let _ = pending.await;
            }
        }
        stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        if joined.is_err() { serving.abort(); let _ = serving.await; }
        joined.expect("plugin authorization runtime must join").unwrap().unwrap();
        if let Err(error) = assertions { std::panic::resume_unwind(error); }
    }).await;
}

#[tokio::test]
async fn global_export_rechecks_admin_after_storage_wait() {
    revoked_after_storage_wait(WaitingRequest::AdminExport).await;
}

#[tokio::test]
async fn global_purge_rechecks_admin_after_storage_wait_without_deleting() {
    revoked_after_storage_wait(WaitingRequest::AdminPurge).await;
}

#[tokio::test]
async fn personal_purge_rechecks_active_account_after_storage_wait_without_deleting() {
    revoked_after_storage_wait(WaitingRequest::PersonalPurge).await;
}
