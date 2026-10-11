//! Real HTTP partial settings outcomes, explicit retries and SQL privacy authority.
use super::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

const TOOL_LIMIT: &str = "core.policies.tools.max_tools_per_turn";

#[derive(Default)]
struct SettingsProbe {
    values: Mutex<Vec<Value>>,
    fail_next: AtomicBool,
}

struct SettingsModule(Arc<SettingsProbe>);

#[async_trait]
impl ModuleLifecycle for SettingsModule {
    fn name(&self) -> &'static str {
        "settings-regression"
    }

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let Some(value) = updates.get(TOOL_LIMIT) else {
            return Ok(Vec::new());
        };
        // The mutation precedes the error: the API must not claim global rollback.
        self.0.values.lock().unwrap().push(value.clone());
        anyhow::ensure!(
            !self.0.fail_next.swap(false, Ordering::SeqCst),
            "injected settings apply failure"
        );
        Ok(vec![TOOL_LIMIT.into()])
    }
}

async fn stop_runtime(
    stop: &CancellationToken,
    mut serving: tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    stop.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
    if joined.is_err() {
        serving.abort();
        let _ = serving.await;
    }
    joined
        .expect("settings HTTP runtime must join")
        .unwrap()
        .unwrap();
}

fn saved_values(path: &std::path::Path) -> BTreeMap<String, Value> {
    toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn file_failure_keeps_live_settings_and_same_value_retry_persists_for_restart() {
    with_fixture(|f| async move {
        let probe = Arc::new(SettingsProbe::default());
        let runtime = f
            .builder()
            .with_extension(SettingsModule(probe.clone()))
            .build()
            .await
            .unwrap();
        let state = runtime.state().clone();
        let url = format!("http://{}/api/v1/settings", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let (_, admin) = delivery::account(&state, "admin").await;
            let client = skills::client();
            let initial = state
                .modules
                .context
                .policy_store()
                .current()
                .await
                .unwrap();
            let before = state
                .interaction_policies
                .read()
                .await
                .tools
                .max_tools_per_turn;
            let next = before + 1;
            let body = json!({(TOOL_LIMIT): next});
            let path = f.dir.join("settings.toml");
            state.settings.save().await.unwrap();
            let original = std::fs::read(&path).unwrap();
            probe.values.lock().unwrap().clear();

            // An existing regular file cannot become the parent of another file.
            // This fails before publication without chmod or shared environment changes.
            state
                .settings
                .set_persist_path(path.join("blocked.toml"))
                .await;
            let failed =
                skills::response(client.patch(&url).bearer_auth(&admin).json(&body), 200).await;
            assert_eq!(failed["success"], false, "{failed}");
            assert!(!failed["errors"].as_array().unwrap().is_empty());
            assert!(
                !failed
                    .to_string()
                    .contains(f.dir.to_string_lossy().as_ref())
            );
            assert!(
                failed["changed"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(TOOL_LIMIT))
            );
            assert!(
                failed["applied"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(TOOL_LIMIT))
            );
            assert_eq!(state.settings.get(TOOL_LIMIT).await, Some(json!(next)));
            assert_eq!(
                state
                    .interaction_policies
                    .read()
                    .await
                    .tools
                    .max_tools_per_turn,
                next
            );
            assert_eq!(*probe.values.lock().unwrap(), [json!(next)]);
            assert_eq!(std::fs::read(&path).unwrap(), original);
            assert_eq!(
                state
                    .modules
                    .context
                    .policy_store()
                    .current()
                    .await
                    .unwrap()
                    .version,
                initial.version
            );

            state.settings.set_persist_path(path.clone()).await;
            let retried =
                skills::response(client.patch(&url).bearer_auth(&admin).json(&body), 200).await;
            assert_eq!(retried["success"], true, "{retried}");
            assert!(
                retried["changed"].as_array().unwrap().is_empty(),
                "desired settings already matched: {retried}"
            );
            assert!(
                retried["applied"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(TOOL_LIMIT))
            );
            assert_eq!(*probe.values.lock().unwrap(), [json!(next), json!(next)]);
            assert_eq!(saved_values(&path).get(TOOL_LIMIT), Some(&json!(next)));
            assert_eq!(retried["context_policy_version"], initial.version);
            next
        })
        .catch_unwind()
        .await;
        stop_runtime(&stop, serving).await;
        let next = match result {
            Ok(next) => next,
            Err(error) => std::panic::resume_unwind(error),
        };
        let restarted = f.builder().build().await.unwrap();
        let recovered = AssertUnwindSafe(async {
            assert_eq!(
                restarted.state().settings.get(TOOL_LIMIT).await,
                Some(json!(next))
            );
            assert_eq!(
                restarted
                    .state()
                    .interaction_policies
                    .read()
                    .await
                    .tools
                    .max_tools_per_turn,
                next
            );
        })
        .catch_unwind()
        .await;
        restarted.shutdown().await.unwrap();
        if let Err(error) = recovered {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}

#[tokio::test]
async fn module_partial_failure_saves_desired_settings_and_explicit_retry_applies_again() {
    with_fixture(|f| async move {
        let probe = Arc::new(SettingsProbe::default());
        let runtime = f
            .builder()
            .with_extension(SettingsModule(probe.clone()))
            .build()
            .await
            .unwrap();
        let state = runtime.state().clone();
        let url = format!("http://{}/api/v1/settings", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let (_, admin) = delivery::account(&state, "admin").await;
            let client = skills::client();
            let next = state
                .interaction_policies
                .read()
                .await
                .tools
                .max_tools_per_turn
                + 2;
            let body = json!({(TOOL_LIMIT): next});
            probe.values.lock().unwrap().clear();
            probe.fail_next.store(true, Ordering::SeqCst);

            let failed =
                skills::response(client.patch(&url).bearer_auth(&admin).json(&body), 200).await;
            assert_eq!(failed["success"], false, "{failed}");
            assert!(
                failed["module_reports"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|report| {
                        report["module"] == "settings-regression" && report["error"].is_string()
                    })
            );
            assert_eq!(*probe.values.lock().unwrap(), [json!(next)]);
            assert_eq!(state.settings.get(TOOL_LIMIT).await, Some(json!(next)));
            assert_eq!(
                state
                    .interaction_policies
                    .read()
                    .await
                    .tools
                    .max_tools_per_turn,
                next
            );
            assert_eq!(
                saved_values(&f.dir.join("settings.toml")).get(TOOL_LIMIT),
                Some(&json!(next))
            );

            let retried =
                skills::response(client.patch(&url).bearer_auth(&admin).json(&body), 200).await;
            assert_eq!(retried["success"], true, "{retried}");
            assert!(retried["changed"].as_array().unwrap().is_empty());
            assert!(
                retried["module_reports"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|report| {
                        report["module"] == "settings-regression"
                            && report["applied"]
                                .as_array()
                                .is_some_and(|keys| keys.contains(&json!(TOOL_LIMIT)))
                            && report["error"].is_null()
                    })
            );
            assert_eq!(*probe.values.lock().unwrap(), [json!(next), json!(next)]);
        })
        .catch_unwind()
        .await;
        stop_runtime(&stop, serving).await;
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}

#[tokio::test]
async fn privacy_patch_uses_admin_sql_cas_and_all_instances_read_committed_policy() {
    with_fixture(|f| async move {
        let first = f.builder().build().await.unwrap();
        let first_state = first.state().clone();
        let first_url = format!("http://{}/api/v1", first.http_addr());
        let first_stop = first_state.cancel_token.clone();
        let first_serving = tokio::spawn(first.run());
        let result = AssertUnwindSafe(async {
            let second = f.builder().build().await.unwrap();
            let second_state = second.state().clone();
            let second_url = format!("http://{}/api/v1", second.http_addr());
            let second_stop = second_state.cancel_token.clone();
            let second_serving = tokio::spawn(second.run());
            let assertions = AssertUnwindSafe(async {
                let (_, admin) = delivery::account(&first_state, "admin").await;
                let (_, user) = delivery::account(&first_state, "user").await;
                let client = skills::client();
                let privacy = format!("{first_url}/memories/privacy");
                let settings = format!("{first_url}/settings");
                let settings_response = client
                    .get(&settings)
                    .bearer_auth(&admin)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(settings_response.status(), 200);
                let old_tag = settings_response.headers()["etag"]
                    .to_str()
                    .unwrap()
                    .to_owned();
                let initial = first_state
                    .modules
                    .context
                    .policy_store()
                    .current()
                    .await
                    .unwrap();
                let before = first_state
                    .settings
                    .get("core.policies.memory.enabled")
                    .await;
                assert_eq!(before, Some(json!(false)));
                let original_file = std::fs::read(f.dir.join("settings.toml")).ok();
                let body =
                    json!({"enabled":true,"capture_mode":"explicit_tool_only","clear_pause":true});
                skills::response(client.patch(&privacy).bearer_auth(&user).json(&body), 403).await;
                skills::response(client.get(&privacy).bearer_auth(&user), 200).await;
                skills::response(
                    client
                        .patch(&privacy)
                        .bearer_auth(&admin)
                        .header("if-match", "unquoted")
                        .json(&body),
                    400,
                )
                .await;

                first_state.db.execute_unprepared(
                    "CREATE FUNCTION fail_settings_privacy() RETURNS trigger LANGUAGE plpgsql AS $$
                     BEGIN RAISE EXCEPTION 'private settings fixture SQL failure'; END $$;
                     CREATE TRIGGER fail_settings_privacy BEFORE UPDATE ON context_policy
                     FOR EACH ROW EXECUTE FUNCTION fail_settings_privacy()",
                ).await.unwrap();
                let failed = skills::response(
                    client
                        .patch(&privacy)
                        .bearer_auth(&admin)
                        .header("if-match", &old_tag)
                        .json(&body),
                    503,
                )
                .await;
                assert!(!failed.to_string().contains("private settings fixture"));
                assert_eq!(
                    first_state
                        .settings
                        .get("core.policies.memory.enabled")
                        .await,
                    before
                );
                assert!(!first_state.interaction_policies.read().await.memory.enabled);
                assert_eq!(
                    first_state
                        .modules
                        .context
                        .policy_store()
                        .current()
                        .await
                        .unwrap()
                        .version,
                    initial.version
                );
                assert_eq!(
                    std::fs::read(f.dir.join("settings.toml")).ok(),
                    original_file
                );
                first_state
                    .db
                    .execute_unprepared(
                        "DROP TRIGGER fail_settings_privacy ON context_policy;
                     DROP FUNCTION fail_settings_privacy()",
                    )
                    .await
                    .unwrap();

                let updated = skills::response(
                    client
                        .patch(&privacy)
                        .bearer_auth(&admin)
                        .header("if-match", &old_tag)
                        .json(&body),
                    200,
                )
                .await;
                assert_eq!(
                    updated.as_object().unwrap().len(),
                    8,
                    "privacy response shape must remain stable"
                );
                assert_eq!(updated["enabled"], true);
                assert_eq!(updated["capture_mode"], "explicit_tool_only");
                assert_eq!(updated["automatic_capture_enabled"], false);
                assert_eq!(updated["paused_until"], Value::Null);
                assert_eq!(updated["explicit_memory_enabled"], true);
                assert_eq!(updated["sensitive_auto_capture_allowed"], false);
                assert_eq!(updated["sensitive_direct_entry_allowed"], true);
                assert!(
                    updated["supported_capture_modes"]
                        .as_array()
                        .unwrap()
                        .contains(&json!("explicit_tool_only"))
                );
                assert_eq!(
                    first_state
                        .modules
                        .context
                        .policy_store()
                        .current()
                        .await
                        .unwrap()
                        .version,
                    initial.version + 1
                );
                assert_eq!(
                    std::fs::read(f.dir.join("settings.toml")).ok(),
                    original_file,
                    "SQL-only privacy must not invoke unrelated file saving"
                );
                skills::response(
                    client
                        .patch(&privacy)
                        .bearer_auth(&admin)
                        .header("if-match", &old_tag)
                        .json(&json!({"enabled":false})),
                    409,
                )
                .await;

                assert!(
                    !second_state
                        .interaction_policies
                        .read()
                        .await
                        .memory
                        .enabled,
                    "second runtime intentionally retains its original RAM projection"
                );
                let remote = skills::response(
                    client
                        .get(format!("{second_url}/memories/privacy"))
                        .bearer_auth(&user),
                    200,
                )
                .await;
                assert_eq!(
                    remote, updated,
                    "privacy GET must read committed SQL across instances"
                );
                let audit = skills::response(
                    client
                        .get(format!("{second_url}/memories/audit"))
                        .bearer_auth(&user),
                    200,
                )
                .await;
                assert_eq!(audit["privacy"], updated);
                let paused = skills::response(
                    client
                        .patch(&privacy)
                        .bearer_auth(&admin)
                        .json(&json!({"pause_days":1})),
                    200,
                )
                .await;
                assert_eq!(paused["enabled"], true);
                assert_eq!(paused["explicit_memory_enabled"], false);
                let until =
                    chrono::DateTime::parse_from_rfc3339(paused["paused_until"].as_str().unwrap())
                        .unwrap();
                assert!(until > chrono::Utc::now());
                let remote_paused = skills::response(
                    client
                        .get(format!("{second_url}/memories/privacy"))
                        .bearer_auth(&user),
                    200,
                )
                .await;
                assert_eq!(remote_paused, paused);

                let cleared = skills::response(
                    client
                        .patch(&privacy)
                        .bearer_auth(&admin)
                        .json(&json!({"clear_pause":true,"pause_days":1})),
                    200,
                )
                .await;
                assert_eq!(
                    cleared["paused_until"],
                    Value::Null,
                    "clear_pause takes precedence over pause_days"
                );
                assert_eq!(cleared["explicit_memory_enabled"], true);

                // The policy-store suite covers expiry CAS races. Here the actual
                // HTTP read must use that store, not retain an expired RAM string.
                let current = first_state
                    .modules
                    .context
                    .policy_store()
                    .current()
                    .await
                    .unwrap();
                let mut due = current.policy;
                due.paused_until = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
                let due = first_state
                    .modules
                    .context
                    .policy_store()
                    .replace(current.version, due)
                    .await
                    .unwrap();
                let resumed = skills::response(
                    client
                        .get(format!("{second_url}/memories/privacy"))
                        .bearer_auth(&user),
                    200,
                )
                .await;
                assert_eq!(resumed["paused_until"], Value::Null);
                assert_eq!(resumed["explicit_memory_enabled"], true);
                let persisted = first_state
                    .modules
                    .context
                    .policy_store()
                    .current()
                    .await
                    .unwrap();
                assert_eq!(persisted.version, due.version + 1);
                assert!(persisted.policy.paused_until.is_none());
                assert_eq!(
                    std::fs::read(f.dir.join("settings.toml")).ok(),
                    original_file
                );
                persisted.version
            })
            .catch_unwind()
            .await;
            stop_runtime(&second_stop, second_serving).await;
            match assertions {
                Ok(version) => version,
                Err(error) => std::panic::resume_unwind(error),
            }
        })
        .catch_unwind()
        .await;
        stop_runtime(&first_stop, first_serving).await;
        let version = match result {
            Ok(version) => version,
            Err(error) => std::panic::resume_unwind(error),
        };
        let restarted = f.builder().build().await.unwrap();
        let recovered = AssertUnwindSafe(async {
            let policy = restarted
                .state()
                .modules
                .context
                .policy_store()
                .current()
                .await
                .unwrap();
            assert_eq!(policy.version, version);
            assert!(policy.policy.memory_enabled);
            assert_eq!(
                policy.policy.capture_mode,
                alia_server::config::MemoryCaptureMode::ExplicitToolOnly
            );
            assert!(
                restarted
                    .state()
                    .interaction_policies
                    .read()
                    .await
                    .memory
                    .enabled
            );
        })
        .catch_unwind()
        .await;
        restarted.shutdown().await.unwrap();
        if let Err(error) = recovered {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}
