//! Settings transitions must make a new Memory vector space usable immediately.
use super::*;
use alia_server::{
    modules::memory::{MemoryNode, MemoryPatch, RetrievalOpts},
    providers::adapter::{EmbeddingProfile, embedding_text_digest},
};
use axum::{Json, Router, extract::Path, extract::State, routing::post};
use sea_orm::{DatabaseBackend, Statement};
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

fn vector(dimensions: usize) -> Vec<f32> {
    let mut values = vec![0.25; dimensions];
    values[0] = dimensions as f32;
    values
}

async fn embeddings(
    State(requests): State<Arc<AsyncMutex<Vec<Value>>>>,
    Path(endpoint): Path<String>,
    Json(mut request): Json<Value>,
) -> Json<Value> {
    request["endpoint"] = json!(endpoint);
    requests.lock().await.push(request.clone());
    let dimensions = request["dimensions"].as_u64().unwrap() as usize;
    let data = request["input"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, _)| json!({"index":index,"embedding":vector(dimensions)}))
        .collect::<Vec<_>>();
    Json(json!({"model":request["model"],"data":data}))
}

fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}

async fn descriptor_count(db: &DatabaseConnection, id: &str) -> i64 {
    db.query_one(sql(
        "SELECT count(*) AS n FROM embedding_profiles WHERE id=$1",
        vec![id.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "n")
    .unwrap()
}

async fn assert_projection(
    db: &DatabaseConnection,
    memory: &str,
    profile: &EmbeddingProfile,
    revision: i64,
    text: &str,
) {
    let row = db.query_one(sql(
        "SELECT source_revision,input_digest,dimensions,vector_dims(embedding) AS actual_dimensions,embedding::text AS actual_vector FROM memory_embeddings WHERE memory_id=$1 AND profile_id=$2",
        vec![Uuid::parse_str(memory).unwrap().into(),profile.id().unwrap().into()],
    )).await.unwrap().expect("Memory must project without a restart or Cache registration");
    assert_eq!(row.try_get::<i64>("", "source_revision").unwrap(), revision);
    assert_eq!(
        row.try_get::<String>("", "input_digest").unwrap(),
        embedding_text_digest(text)
    );
    assert_eq!(
        row.try_get::<i32>("", "dimensions").unwrap(),
        profile.dimensions as i32
    );
    assert_eq!(
        row.try_get::<i32>("", "actual_dimensions").unwrap(),
        profile.dimensions as i32
    );
    let actual: Vec<f32> =
        serde_json::from_str(&row.try_get::<String>("", "actual_vector").unwrap()).unwrap();
    assert_eq!(actual, vector(profile.dimensions));
}

#[tokio::test]
async fn cache_disabled_http_profile_switch_is_atomic_and_immediately_projects_memory() {
    with_fixture(|mut f| async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AsyncMutex::new(Vec::<Value>::new()));
        let app = Router::new()
            .route("/{endpoint}/v1/embeddings", post(embeddings))
            .with_state(requests.clone());
        let provider_stop = CancellationToken::new();
        let _provider_drop = provider_stop.clone().drop_guard();
        let provider_cancel = provider_stop.clone();
        let mut provider = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(provider_cancel.cancelled_owned())
                .await
        });

        // Keep cleanup outside the assertion future so a regression does not
        // leave either HTTP server running while its schema is removed.
        let result = AssertUnwindSafe(async {
            f.config.policies.memory.enabled = true;
            f.config.policies.cache.enabled = false;
            f.config.policies.cache.exact_match_enabled = false;
            f.config.policies.cache.semantic_match_enabled = false;
            f.config.llm.roles.embedding = "embedding".into();
            let configured = f.config.profiles.llm.get_mut("embedding").unwrap();
            configured.model = "text-embedding-3-small".into();
            configured.dimensions = Some(4);
            let alia_server::config::LlmProviderProfileSection::OpenAi(configured) =
                f.config.providers.llm.get_mut("openai").unwrap()
            else {
                panic!("OpenAI fixture")
            };
            configured.base_url = format!("{endpoint}/initial/v1");
            configured.api_key = "local-embedding-fixture".into();

            // Use the real builder and OpenAiEmbedding, with no test attach or
            // Cache call that could pre-register the replacement descriptor.
            let runtime = f.builder().build().await.unwrap();
            let state = runtime.state().clone();
            let stop = state.cancel_token.clone();
            let _runtime_drop = stop.clone().drop_guard();
            let url = format!("http://{}/api/v1/settings", runtime.http_addr());
            let mut serving = tokio::spawn(runtime.run());
            let assertions = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(25), async {
                let (owner, admin) = delivery::account(&state, "admin").await;
                let original_policy = state.modules.context.policy_store().current().await.unwrap();
                assert!(!original_policy.policy.cache_enabled);
                let original_profile = state.modules.embedding
                    .preview_profile_settings(&Default::default()).await.unwrap().unwrap();
                assert_eq!(original_profile.dimensions, 4);
                let original_text = "walnut orchard planting preference";
                let original_id = state.modules.memory.store(
                    owner, MemoryNode::new("core", "fact", original_text, 0.9),
                ).await.unwrap();
                assert_projection(&state.db, &original_id, &original_profile, 1, original_text).await;

                let patch = std::collections::BTreeMap::from([
                    ("core.profiles.llm.embedding.model".into(), json!("text-embedding-3-large")),
                    ("core.profiles.llm.embedding.dimensions".into(), json!(6)),
                    ("core.providers.llm.openai.base_url".into(), json!(format!("{endpoint}/replacement/v1"))),
                ]);
                let next_profile = state.modules.embedding.preview_profile_settings(&patch)
                    .await.unwrap().unwrap();
                let next_id = next_profile.id().unwrap();
                assert_ne!(next_id, original_profile.id().unwrap());
                assert_eq!(descriptor_count(&state.db, &next_id).await, 0);
                let client = skills::client();

                state.db.execute_unprepared(
                    "CREATE FUNCTION reject_profile_policy_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected embedding policy failure'; END; $$; CREATE TRIGGER reject_profile_policy_update BEFORE UPDATE ON context_policy FOR EACH ROW EXECUTE FUNCTION reject_profile_policy_update()"
                ).await.unwrap();
                let failed = client.patch(&url).bearer_auth(&admin).json(&patch).send().await.unwrap();
                assert!(failed.status().is_server_error(), "{}", failed.status());
                let unchanged = state.modules.context.policy_store().current().await.unwrap();
                assert_eq!(unchanged.version, original_policy.version);
                assert_eq!(unchanged.policy, original_policy.policy);
                assert_eq!(descriptor_count(&state.db, &next_id).await, 0, "failed policy CAS must roll back its descriptor insert");
                assert_eq!(state.modules.embedding.preview_profile_settings(&Default::default()).await.unwrap(), Some(original_profile.clone()));
                assert_eq!(state.settings.get("core.profiles.llm.embedding.model").await, Some(json!(original_profile.model)));
                assert!(requests.lock().await.iter().all(|r| r["endpoint"] == "initial"));
                state.db.execute_unprepared("DROP TRIGGER reject_profile_policy_update ON context_policy; DROP FUNCTION reject_profile_policy_update()").await.unwrap();

                let response = client.patch(&url).bearer_auth(&admin).json(&patch).send().await.unwrap();
                let status = response.status();
                let body: Value = response.json().await.unwrap();
                assert_eq!(status, 200, "{body}");
                assert_eq!(body["success"], true, "{body}");
                assert_eq!(body["errors"], json!([]));
                let current = state.modules.context.policy_store().current().await.unwrap();
                assert_eq!(current.version, original_policy.version + 1);
                assert!(!current.policy.cache_enabled);
                assert_eq!(current.policy.active_embedding_profile.as_deref(), Some(next_id.as_str()));
                assert_eq!(current.policy.cache_embedding_profile_id.as_deref(), Some(next_id.as_str()));
                let descriptor = state.db.query_one(sql("SELECT request_descriptor FROM embedding_profiles WHERE id=$1", vec![next_id.clone().into()]))
                    .await.unwrap().unwrap().try_get::<Value>("", "request_descriptor").unwrap();
                assert_eq!(descriptor, serde_json::to_value(&next_profile).unwrap());
                assert_eq!(state.modules.embedding.preview_profile_settings(&Default::default()).await.unwrap(), Some(next_profile.clone()));

                let stored_text = "maple grove compass collection";
                let stored_id = state.modules.memory.store(owner, MemoryNode::new("core", "fact", stored_text, 0.9)).await.unwrap();
                assert_projection(&state.db, &stored_id, &next_profile, 1, stored_text).await;
                let corrected_text = "cedar grove corrected observation";
                state.modules.memory.update(owner, &original_id, MemoryPatch {
                    expected_revision: Some(1), content: Some(corrected_text.into()), ..Default::default()
                }).await.unwrap();
                assert_projection(&state.db, &original_id, &next_profile, 2, corrected_text).await;
                let old_count: i64 = state.db.query_one(sql("SELECT count(*) AS n FROM memory_embeddings WHERE memory_id=$1 AND profile_id=$2", vec![Uuid::parse_str(&original_id).unwrap().into(),original_profile.id().unwrap().into()]))
                    .await.unwrap().unwrap().try_get("", "n").unwrap();
                assert_eq!(old_count, 0);

                let query = "unrelated semantic probe";
                let found = state.modules.memory.retrieve(owner, query, RetrievalOpts::default()).await.unwrap();
                for id in [&stored_id, &original_id] {
                    let memory = found.iter().find(|m| m.id.as_ref() == Some(id)).expect("new profile must support vector retrieval immediately");
                    assert_eq!(memory.metadata["retrieval_path"], "profiled_vector");
                }
                let observed = requests.lock().await;
                for text in [stored_text, corrected_text, query] {
                    assert!(observed.iter().any(|r| r["endpoint"] == "replacement"
                        && r["model"] == "text-embedding-3-large" && r["dimensions"] == 6
                        && r["input"] == json!([text])), "missing actual profiled request for {text}: {observed:?}");
                }
                let cached: i64 = state.db.query_one(sql("SELECT count(*) AS n FROM llm_cache", vec![]))
                    .await.unwrap().unwrap().try_get("", "n").unwrap();
                assert_eq!(cached, 0);
            })).catch_unwind().await;
            stop.cancel();
            let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
            if joined.is_err() {
                serving.abort();
                let _ = serving.await;
            }
            joined.expect("runtime must join").unwrap().unwrap();
            match assertions {
                Ok(Ok(())) => {}
                Ok(Err(_)) => panic!("embedding HTTP assertions exceeded deadline"),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }).catch_unwind().await;
        provider_stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut provider).await;
        if joined.is_err() {
            provider.abort();
            let _ = provider.await;
        }
        joined.expect("embedding provider must join").unwrap().unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }).await;
}
