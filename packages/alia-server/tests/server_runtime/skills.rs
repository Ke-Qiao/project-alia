//! Skills HTTP writes must publish a complete index without changing tool authority.
use super::*;
use alia_server::registry::ArtifactRef;
use alia_server::state::AppState;
use sea_orm::{DatabaseBackend, Statement, TransactionTrait};
use serde_json::{Value, json};

pub(super) fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

pub(super) async fn response(request: reqwest::RequestBuilder, expected: u16) -> Value {
    let response = request.send().await.unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status.as_u16(), expected, "HTTP {status}: {body}");
    serde_json::from_str(&body).unwrap()
}

pub(super) fn definition(name: &str, keyword: &str) -> Value {
    json!({
        "name": name, "display_name": "HTTP skill", "category": "test",
        "intent_patterns": [keyword], "prompt_template": "original prompt",
        "preferred_tools": ["schedule_task"], "priority": 100
    })
}

async fn resolution(state: &AppState, name: &str) -> Value {
    let catalog = state.modules.skills.catalog().await.unwrap();
    serde_json::to_value(
        state
            .modules
            .skills
            .resolve(&catalog, name)
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

async fn persisted(state: &AppState, name: &str) -> Value {
    state.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "SELECT jsonb_build_object('skill',(SELECT to_jsonb(s) FROM skills s WHERE name=$1), \
         'bindings',(SELECT jsonb_agg(to_jsonb(b) ORDER BY b.tool_id) FROM skill_preferred_tools b \
         JOIN skills s ON s.id=b.skill_id WHERE s.name=$1), \
         'artifact',(SELECT to_jsonb(a) FROM registry_artifacts a WHERE artifact_kind='skill' AND artifact_name=$1), \
         'dependencies',(SELECT jsonb_agg(to_jsonb(d) ORDER BY dependency_kind,dependency_name) \
         FROM registry_dependencies d WHERE dependent_kind='skill' AND dependent_name=$1)) AS snapshot",
        [name.into()],
    )).await.unwrap().unwrap().try_get("", "snapshot").unwrap()
}

#[tokio::test]
async fn http_crud_publishes_skill_context_without_granting_tool_authority() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (_, token) = delivery::account(&state, "user").await;
        let (_, other_token) = delivery::account(&state, "user").await;
        let base = format!("http://{}/api/v1", runtime.http_addr());
        let url = format!("{base}/skills");
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            let name = "http_skill_crud";
            let old_keyword = "skillcrudoriginalkeyword";
            let new_keyword = "skillcrudreplacementkeyword";
            let item = format!("{url}/{name}");
            let tool_before = response(client.get(format!("{base}/tools/schedule_task")).bearer_auth(&token), 200).await;
            assert_eq!(tool_before["require_confirmation"], true);
            state.db.execute_unprepared("INSERT INTO tools(name,source,enabled) VALUES('disabled_skill_fixture','api',false)").await.unwrap();
            let mut body = definition(name, old_keyword);
            body["preferred_tools"] = json!(["schedule_task", "disabled_skill_fixture", "schedule_task"]);
            assert_eq!(client.post(&url).json(&body).send().await.unwrap().status(), 401);
            assert_eq!(persisted(&state, name).await["skill"], Value::Null);
            let created = response(client.post(&url).bearer_auth(&token).json(&body), 200).await;
            assert_eq!(created["preferred_tools"], json!(["schedule_task"]));
            let matched = response(client.post(format!("{url}/match")).bearer_auth(&token).json(&json!({"intent":old_keyword})), 200).await;
            assert_eq!(matched["skill"], name);
            assert_eq!(resolution(&state, name).await, json!({"skill_name":name,"system_prompt":"original prompt","preferred_tools":["schedule_task"],"cacheable":false}));
            assert!(state.modules.skills.list_skills().await.unwrap().iter().any(|skill| skill.name == name));

            // A skill preference neither installs an executor nor approves a side effect.
            assert_eq!(client.get(format!("{base}/tools/disabled_skill_fixture")).bearer_auth(&token).send().await.unwrap().status(), 404);
            assert_eq!(response(client.get(format!("{base}/tools/schedule_task")).bearer_auth(&token), 200).await, tool_before);
            let effect = response(client.post(format!("{base}/tools/execute")).bearer_auth(&token).json(&json!({
                "request_id":Uuid::new_v4(), "tool_name":"schedule_task",
                "arguments":{"name":"must remain a proposal","schedule_type":"once","run_at":"2099-01-01T00:00:00Z","action":{"type":"notification","message":"not dispatched"},"confirmed":true}
            })), 200).await;
            assert_eq!(effect["status"], "proposed");
            assert_eq!(effect["requires_confirmation"], true);
            let effect_id = effect["effect_id"].as_str().unwrap();
            response(client.post(format!("{base}/effects/{effect_id}/confirm")).bearer_auth(&other_token).json(&json!({"intent_digest":effect["intent_digest"]})), 404).await;
            let count: i64 = state.db.query_one(Statement::from_string(DatabaseBackend::Postgres, "SELECT count(*) AS n FROM scheduled_tasks".to_string())).await.unwrap().unwrap().try_get("", "n").unwrap();
            assert_eq!(count, 0);
            assert_eq!(response(client.post(format!("{base}/effects/{effect_id}/cancel")).bearer_auth(&token), 200).await["status"], "cancelled");

            let before = persisted(&state, name).await;
            let before_index = resolution(&state, name).await;
            response(client.put(&item).bearer_auth(&token).json(&json!({"prompt_template":"must roll back","preferred_tools":["no_such_skill_tool"]})), 400).await;
            assert_eq!(persisted(&state, name).await, before);
            assert_eq!(resolution(&state, name).await, before_index);
            let mut invalid = definition("invalid_http_skill", "neverpublishedkeyword");
            invalid["preferred_tools"] = json!(["no_such_skill_tool"]);
            response(client.post(&url).bearer_auth(&token).json(&invalid), 400).await;
            assert_eq!(persisted(&state, "invalid_http_skill").await, json!({"skill":null,"bindings":null,"artifact":null,"dependencies":null}));

            response(client.put(&item).bearer_auth(&token).json(&json!({"intent_patterns":[new_keyword],"prompt_template":"updated prompt","preferred_tools":[],"cacheable":true})), 200).await;
            assert_eq!(response(client.post(format!("{url}/match")).bearer_auth(&token).json(&json!({"intent":old_keyword})), 200).await["matched"], false);
            assert_eq!(response(client.post(format!("{url}/match")).bearer_auth(&token).json(&json!({"intent":new_keyword})), 200).await["skill"], name);
            assert_eq!(resolution(&state, name).await, json!({"skill_name":name,"system_prompt":"updated prompt","preferred_tools":[],"cacheable":true}));
            response(client.put(&item).bearer_auth(&token).json(&json!({"enabled":false})), 200).await;
            assert!(state.modules.skills.match_skill(new_keyword).await.unwrap().is_none());
            assert!(!state.modules.skills.list_skills().await.unwrap().iter().any(|skill| skill.name == name));
            let catalog = state.modules.skills.catalog().await.unwrap();
            assert!(state.modules.skills.resolve(&catalog, name).await.unwrap().is_none());
            response(client.put(&item).bearer_auth(&token).json(&json!({"enabled":true})), 200).await;
            assert_eq!(state.modules.skills.match_skill(new_keyword).await.unwrap().unwrap().name, name);

            let dependent = ArtifactRef::skill("dependent_skill_fixture");
            state.artifacts.upsert_artifact(&dependent, "api").await.unwrap();
            state.artifacts.replace_dependencies(&dependent, &[ArtifactRef::skill(name)]).await.unwrap();
            let before = persisted(&state, name).await;
            response(client.delete(&item).bearer_auth(&token), 500).await;
            assert_eq!(persisted(&state, name).await, before);
            assert_eq!(resolution(&state, name).await["system_prompt"], "updated prompt");
            state.artifacts.delete_artifact(&dependent).await.unwrap();
            assert_eq!(response(client.delete(&item).bearer_auth(&token), 200).await["success"], true);
            assert_eq!(persisted(&state, name).await, json!({"skill":null,"bindings":null,"artifact":null,"dependencies":null}));
            assert!(state.modules.skills.match_skill(new_keyword).await.unwrap().is_none());
            assert!(!state.modules.skills.list_skills().await.unwrap().iter().any(|skill| skill.name == name));
            let catalog = state.modules.skills.catalog().await.unwrap();
            assert!(state.modules.skills.resolve(&catalog, name).await.unwrap().is_none());
            response(client.delete(&item).bearer_auth(&token), 404).await;
            response(client.put(&item).bearer_auth(&token).json(&json!({"enabled":true})), 404).await;
            assert_eq!(response(client.get(format!("{base}/tools/schedule_task")).bearer_auth(&token), 200).await, tool_before);
        }).catch_unwind().await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving).await.unwrap().unwrap().unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }).await;
}

#[tokio::test]
async fn failed_skill_snapshot_keeps_the_complete_index_and_rolls_back_http_changes() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (_, token) = delivery::account(&state, "user").await;
        let url = format!("http://{}/api/v1/skills", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            let name = "snapshot_failure_skill";
            let keyword = "snapshotfailurekeyword";
            response(client.post(&url).bearer_auth(&token).json(&definition(name, keyword)), 200).await;
            let before = persisted(&state, name).await;
            let before_index = resolution(&state, name).await;
            let mut names: Vec<_> = state.modules.skills.list_skills().await.unwrap().into_iter().map(|skill| skill.name).collect();
            names.sort();
            state.db.execute_unprepared("ALTER TABLE skill_preferred_tools RENAME TO unavailable_skill_preferred_tools").await.unwrap();
            let failed = AssertUnwindSafe(async {
                assert!(state.modules.skills.reload().await.is_err());
                assert_eq!(resolution(&state, name).await, before_index);
                let mut retained: Vec<_> = state.modules.skills.list_skills().await.unwrap().into_iter().map(|skill| skill.name).collect();
                retained.sort();
                assert_eq!(retained, names);
                assert_eq!(state.modules.skills.match_skill(keyword).await.unwrap().unwrap().name, name);
                response(client.put(format!("{url}/{name}")).bearer_auth(&token).json(&json!({"prompt_template":"must not commit","display_name":"must roll back"})), 500).await;
                assert_eq!(resolution(&state, name).await, before_index);
            }).catch_unwind().await;
            state.db.execute_unprepared("ALTER TABLE unavailable_skill_preferred_tools RENAME TO skill_preferred_tools").await.unwrap();
            if let Err(error) = failed { std::panic::resume_unwind(error); }
            assert_eq!(persisted(&state, name).await, before);
            state.modules.skills.reload().await.unwrap();
            assert_eq!(resolution(&state, name).await, before_index);

            // The response has no remaining tool bindings to read; the failure
            // therefore occurs while preparing the complete replacement index.
            state.db.execute_unprepared("ALTER TABLE tools RENAME TO unavailable_skill_tools").await.unwrap();
            let failed = AssertUnwindSafe(async {
                response(client.put(format!("{url}/{name}")).bearer_auth(&token).json(&json!({"prompt_template":"must not commit","preferred_tools":[]})), 500).await;
                assert_eq!(resolution(&state, name).await, before_index);
            }).catch_unwind().await;
            state.db.execute_unprepared("ALTER TABLE unavailable_skill_tools RENAME TO tools").await.unwrap();
            if let Err(error) = failed { std::panic::resume_unwind(error); }
            assert_eq!(persisted(&state, name).await, before);
            response(client.put(format!("{url}/{name}")).bearer_auth(&token).json(&json!({"prompt_template":"recovered"})), 200).await;
            assert_eq!(resolution(&state, name).await["system_prompt"], "recovered");
        }).catch_unwind().await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving).await.unwrap().unwrap().unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }).await;
}

async fn table_waiter(db: &DatabaseConnection, schema: &str, table: &str, mode: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: bool = db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation=to_regclass($1) AND mode=$2 AND NOT granted) AS blocked",
                [format!("{schema}.{table}").into(), mode.into()],
            )).await.unwrap().unwrap().try_get("", "blocked").unwrap();
            if blocked { return; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap_or_else(|_| panic!("{table} did not reach the {mode} gate"));
}

#[tokio::test]
async fn concurrent_skill_reload_retains_readers_and_publishes_the_latest_snapshot() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (_, token) = delivery::account(&state, "user").await;
        let url = format!("http://{}/api/v1/skills", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            let name = "concurrent_reload_skill";
            let keyword = "concurrentreloadkeyword";
            response(
                client
                    .post(&url)
                    .bearer_auth(&token)
                    .json(&definition(name, keyword)),
                200,
            )
            .await;
            let lock = state.db.begin().await.unwrap();
            lock.execute_unprepared("LOCK TABLE skill_preferred_tools IN ACCESS EXCLUSIVE MODE")
                .await
                .unwrap();
            let registry = state.modules.skills.clone();
            let old_reload = tokio::spawn(async move { registry.reload().await });
            let mut writer = None;
            let staged = AssertUnwindSafe(async {
                table_waiter(
                    &f.admin,
                    &f.schema,
                    "skill_preferred_tools",
                    "AccessShareLock",
                )
                .await;
                let retained =
                    tokio::time::timeout(Duration::from_secs(1), resolution(&state, name))
                        .await
                        .expect("snapshot loading must not hold the published-index write lock");
                assert_eq!(retained["system_prompt"], "original prompt");
                let writer_db = state.db.clone();
                writer = Some(tokio::spawn(async move {
                    writer_db.execute(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres,
                        "UPDATE skills SET prompt_template='new database snapshot' WHERE name=$1",
                        [name.into()],
                    )).await
                }));
                table_waiter(&f.admin, &f.schema, "skills", "RowExclusiveLock").await;
                assert!(!writer.as_ref().unwrap().is_finished());
            })
            .catch_unwind()
            .await;
            let mut latest_reload = Box::pin(state.modules.skills.reload());
            let latest_waited = futures::poll!(&mut latest_reload).is_pending();
            lock.rollback().await.unwrap();
            old_reload.await.unwrap().unwrap();
            if let Some(writer) = writer {
                writer.await.unwrap().unwrap();
            }
            latest_reload.await.unwrap();
            assert!(latest_waited);
            if let Err(error) = staged {
                std::panic::resume_unwind(error);
            }
            assert_eq!(
                resolution(&state, name).await["system_prompt"],
                "new database snapshot"
            );
            let item = format!("{url}/{name}");
            let (left, right) = tokio::join!(
                response(
                    client
                        .put(&item)
                        .bearer_auth(&token)
                        .json(&json!({"display_name":"concurrent display"})),
                    200
                ),
                response(
                    client
                        .put(&item)
                        .bearer_auth(&token)
                        .json(&json!({"cacheable":true})),
                    200
                ),
            );
            assert_eq!(left["display_name"], "concurrent display");
            assert_eq!(right["cacheable"], true);
            let stored = response(client.get(&item).bearer_auth(&token), 200).await;
            assert_eq!(stored["display_name"], "concurrent display");
            assert_eq!(stored["cacheable"], true);
            assert_eq!(resolution(&state, name).await["cacheable"], true);
            assert_eq!(
                state
                    .modules
                    .skills
                    .list_skills()
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|skill| skill.name == name)
                    .unwrap()
                    .display_name,
                "concurrent display"
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
