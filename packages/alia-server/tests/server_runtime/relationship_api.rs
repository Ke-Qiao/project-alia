//! Exercise relationship authority through the actual HTTP/runtime/SQL path.
use super::*;
use alia_server::modules::memory::context::{ContextSource, GenerationContext};
use alia_server::modules::memory::source::capture_source_epochs;
use alia_server::providers::adapter::{ChatMessage, MemoryNode};
use alia_server::state::AppState;
use sea_orm::{DatabaseBackend, Statement, TransactionTrait};
use serde_json::{Value, json};

pub(super) async fn generation(state: &AppState, owner: Uuid, text: &str) -> GenerationContext {
    let channel = "relationship-api";
    let session = state
        .modules
        .conversation
        .create_session(channel, &owner.to_string())
        .await
        .unwrap();
    let stamps = capture_source_epochs(&state.db, owner).await.unwrap();
    let received = chrono::Utc::now();
    let source = state
        .modules
        .conversation
        .append_sourced_message(
            owner,
            session,
            ChatMessage::new("user", text),
            received,
            stamps.privacy_epoch,
            stamps.relationship_epoch,
        )
        .await
        .unwrap();
    state
        .modules
        .context
        .materialize(
            ContextSource {
                owner_id: owner,
                request_id: Uuid::new_v4(),
                channel_id: channel.into(),
                conversation_id: Some(session),
                source_event_id: Some(source),
                source_received_at: received,
                source_privacy_epoch: stamps.privacy_epoch,
            },
            text,
        )
        .await
        .unwrap()
}

async fn count(state: &AppState, owner: Uuid, table: &str) -> i64 {
    // The table name is supplied only by this test, never by HTTP input.
    state
        .db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!("SELECT count(*) AS n FROM {table} WHERE owner_id=$1"),
            [owner.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test]
async fn relationship_clear_has_owner_scope_cas_and_durable_retry_identity() {
    with_fixture(|mut f| async move {
        f.config.policies.memory.enabled = true;
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let (other, other_token) = delivery::account(&state, "user").await;
        let original = generation(&state, owner, "thank you").await;
        let other_original = generation(&state, other, "thank you").await;
        state.modules.expression.observe_context_user(&state.modules.context, &original).await.unwrap();
        state.modules.expression.observe_context_user(&state.modules.context, &other_original).await.unwrap();
        let memory = state.modules.memory.store(owner, MemoryNode::new("core", "fact", "an independent fact", 0.8)).await.unwrap();
        let stamps = capture_source_epochs(&state.db, owner).await.unwrap();
        let url = format!("http://{}/api/v1/expression/relationship", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            let before: Value = client.get(&url).bearer_auth(&token).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(before["relationship_epoch"], stamps.relationship_epoch);
            assert_eq!(before["user_id"], owner.to_string());
            assert_eq!(client.delete(&url).bearer_auth(&token).send().await.unwrap().status(), 400);
            for body in [
                json!({"operation_id":Uuid::nil(),"expected_epoch":0}),
                json!({"operation_id":Uuid::new_v4(),"expected_epoch":-1}),
                json!({"operation_id":Uuid::new_v4(),"expected_epoch":0,"owner_id":other}),
            ] {
                assert_eq!(client.delete(&url).bearer_auth(&token).json(&body).send().await.unwrap().status(), 400);
            }
            for limit in [0, 257] {
                assert_eq!(client.post(format!("{url}/prune")).bearer_auth(&token).json(&json!({"limit":limit})).send().await.unwrap().status(), 400);
            }
            assert_eq!(client.post(format!("{url}/prune")).bearer_auth(&token).json(&json!({"limit":1,"owner_id":other})).send().await.unwrap().status(), 400);
            assert_eq!(client.post(format!("{url}/prune")).bearer_auth(&token).json(&json!({"limit":1})).send().await.unwrap().status(), 200);
            let operation = Uuid::new_v4();
            let body = json!({"operation_id":operation,"expected_epoch":stamps.relationship_epoch});
            let (left, right) = tokio::join!(
                client.delete(&url).bearer_auth(&token).json(&body).send(),
                client.delete(&url).bearer_auth(&token).json(&body).send(),
            );
            let receipt: Value = left.unwrap().error_for_status().unwrap().json().await.unwrap();
            let duplicate: Value = right.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(receipt, duplicate);
            assert_eq!(receipt["operation_id"], operation.to_string());
            assert_eq!(receipt["owner_id"], owner.to_string());
            assert_eq!(receipt["relationship_epoch"], stamps.relationship_epoch + 1);
            assert_eq!(receipt["removed_appraisals"], 1);
            assert_eq!(count(&state, owner, "appraisal_records").await, 0);
            assert_eq!(count(&state, other, "appraisal_records").await, 1);
            assert_eq!(count(&state, owner, "relationship_clear_operations").await, 1);
            let after = capture_source_epochs(&state.db, owner).await.unwrap();
            assert_eq!(after.privacy_epoch, stamps.privacy_epoch);
            assert_eq!(after.relationship_epoch, stamps.relationship_epoch + 1);
            assert!(state.modules.memory.get(owner, &memory).await.unwrap().is_some());
            let other_profile: Value = client.get(&url).bearer_auth(&other_token).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(other_profile["relationship_epoch"], 0);
            assert_eq!(client.delete(&url).bearer_auth(&token).json(&json!({"operation_id":Uuid::new_v4(),"expected_epoch":stamps.relationship_epoch})).send().await.unwrap().status(), 409);
            assert_eq!(client.delete(&url).bearer_auth(&token).json(&json!({"operation_id":operation,"expected_epoch":stamps.relationship_epoch+1})).send().await.unwrap().status(), 409);
            assert!(state.modules.context.prepare_user_appraisal(&original).await.is_err());
            // Account authority is read from SQL even when the JWT is valid.
            state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "UPDATE users SET status='disabled' WHERE id=$1", [owner.into()],
            )).await.unwrap();
            assert_eq!(client.get(&url).bearer_auth(&token).send().await.unwrap().status(), 401);
            assert_eq!(client.delete(&url).bearer_auth(&token).json(&body).send().await.unwrap().status(), 401);
            assert_eq!(client.post(format!("{url}/prune")).bearer_auth(&token).json(&json!({"limit":1})).send().await.unwrap().status(), 401);
        }).catch_unwind().await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving).await.unwrap().unwrap().unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }).await;
}

#[tokio::test]
async fn relationship_clear_timeout_retains_original_intent_for_retry() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let initial = capture_source_epochs(&state.db, owner).await.unwrap();
        let url = format!(
            "http://{}/api/v1/expression/relationship",
            runtime.http_addr()
        );
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            // Hold actual policy authority longer than the HTTP deadline. The
            // operation has no permission to switch to a fresh identity later.
            let lock = state.db.begin().await.unwrap();
            lock.execute_unprepared("SELECT version FROM context_policy FOR UPDATE")
                .await
                .unwrap();
            let operation = Uuid::new_v4();
            let body =
                json!({"operation_id":operation,"expected_epoch":initial.relationship_epoch});
            let start = Instant::now();
            let failed = client
                .delete(&url)
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(failed.status(), 503);
            assert!(start.elapsed() < Duration::from_secs(4));
            let unconfirmed: Value = failed.json().await.unwrap();
            assert_eq!(unconfirmed["error"]["operation_id"], operation.to_string());
            assert_eq!(
                unconfirmed["error"]["expected_epoch"],
                initial.relationship_epoch
            );
            lock.rollback().await.unwrap();
            assert_eq!(
                capture_source_epochs(&state.db, owner)
                    .await
                    .unwrap()
                    .relationship_epoch,
                initial.relationship_epoch
            );
            assert_eq!(
                count(&state, owner, "relationship_clear_operations").await,
                0
            );
            let receipt: Value = client
                .delete(&url)
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(receipt["operation_id"], operation.to_string());
            assert_eq!(
                receipt["relationship_epoch"],
                initial.relationship_epoch + 1
            );
        })
        .catch_unwind()
        .await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}

#[tokio::test]
async fn appraisal_policy_patch_is_durable_hot_and_preserves_cas_on_failure() {
    with_fixture(|mut f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, admin) = delivery::account(&state, "admin").await;
        let (_, user) = delivery::account(&state, "user").await;
        let settings = format!("http://{}/api/v1/settings", runtime.http_addr());
        let relationship = format!("http://{}/api/v1/expression/relationship", runtime.http_addr());
        let initial = state.modules.context.policy_store().current().await.unwrap();
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            let response = client.get(&settings).bearer_auth(&admin).send().await.unwrap();
            let etag = response.headers()["etag"].to_str().unwrap().to_owned();
            let key = "core.policies.affect.baseline_trust";
            let before = state.settings.get(key).await;
            let change = json!({
                "core.policies.affect.baseline_trust":0.375,
                "core.policies.appraisal.interpretation_timeout_ms":250,
                "core.policies.affect.relationship_profile_user_visible":false,
                "core.policies.affect.relationship_profile_user_clearable":false,
                "core.resources.appraisal_lexicons":{},
            });
            assert_eq!(client.patch(&settings).bearer_auth(&user).json(&change).send().await.unwrap().status(), 403);
            for body in [
                json!({"core.policies.affect.unknown_flag":true}),
                json!({"core.policies.affect.max_active_appraisal_events":65}),
                json!({"core.policies.appraisal.interpretation_timeout_ms":10001}),
                json!({"core.policies.appraisal.interpreter_version":"unreviewed"}),
                json!({"core.policies.appraisal.reducer_version":"unreviewed"}),
            ] {
                assert_eq!(client.patch(&settings).bearer_auth(&admin).json(&body).send().await.unwrap().status(), 400);
            }
            state.db.execute_unprepared("CREATE FUNCTION fail_appraisal_policy() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture-only policy failure'; END $$; CREATE TRIGGER fail_appraisal_policy BEFORE UPDATE ON context_policy FOR EACH ROW EXECUTE FUNCTION fail_appraisal_policy()").await.unwrap();
            let failure = client.patch(&settings).bearer_auth(&admin).header("if-match", &etag).json(&change).send().await.unwrap();
            assert_eq!(failure.status(), 503);
            assert!(!failure.text().await.unwrap().contains("fixture-only"));
            assert_eq!(state.settings.get(key).await, before);
            assert_eq!(state.modules.context.policy_store().current().await.unwrap().version, initial.version);
            state.db.execute_unprepared("DROP TRIGGER fail_appraisal_policy ON context_policy; DROP FUNCTION fail_appraisal_policy()").await.unwrap();
            let success = client.patch(&settings).bearer_auth(&admin).header("if-match", &etag).json(&change).send().await.unwrap().error_for_status().unwrap();
            let report: Value = success.json().await.unwrap();
            assert_eq!(report["context_policy_version"], initial.version+1);
            for key in change.as_object().unwrap().keys() {
                assert!(report["applied"].as_array().unwrap().contains(&json!(key)), "policy should govern new work immediately: {key}: {report}");
                assert!(!report["requires_restart"].as_array().unwrap().contains(&json!(key)));
                assert!(!report["ignored_runtime"].as_array().unwrap().contains(&json!(key)));
            }
            assert_eq!(client.patch(&settings).bearer_auth(&admin).header("if-match", &etag).json(&json!({"core.policies.affect.enabled":false})).send().await.unwrap().status(), 409);
            assert_eq!(state.settings.get(key).await, Some(json!(0.375)));
            assert_eq!(client.get(&relationship).bearer_auth(&admin).send().await.unwrap().status(), 403);
            assert_eq!(client.delete(&relationship).bearer_auth(&admin).json(&json!({"operation_id":Uuid::new_v4(),"expected_epoch":0})).send().await.unwrap().status(), 403);
            assert_eq!(count(&state, owner, "relationship_clear_operations").await, 0);
        }).catch_unwind().await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving).await.unwrap().unwrap().unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
        // Deployment files remain a seed; existing SQL policy survives a
        // conflicting seed and is projected before the writer can accept work.
        f.config.policies.affect.baseline_trust = 0.125;
        f.config.policies.affect.relationship_profile_user_visible = true;
        let restarted = f.builder().build().await.unwrap();
        let current = restarted.state().modules.context.policy_store().current().await.unwrap();
        assert_eq!(current.version, initial.version+1);
        assert_eq!(current.policy.appraisal.affect.baseline_trust, 0.375);
        assert!(!current.policy.appraisal.affect.relationship_profile_user_visible);
        assert_eq!(restarted.state().settings.get("core.policies.affect.baseline_trust").await, Some(json!(0.375)));
        restarted.shutdown().await.unwrap();
    }).await;
}
