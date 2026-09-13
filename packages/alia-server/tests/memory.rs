use alia_server::providers::MemoryBackend;
use alia_server::providers::adapter::{
    EmbeddingAdapter, MemoryListOpts, MemoryNode, MemoryPatch, RetrievalOpts,
};
use alia_server::providers::memory::NoopMemory;
use alia_server::providers::memory::simple::AliaMemory;
use alia_server::{config::MemorySection, module::ModuleLifecycle};
use async_trait::async_trait;
use futures::FutureExt;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseBackend, Statement};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

// ── AliaMemory ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_store_and_retrieve() {
    let memory = AliaMemory::new();
    let user_id = Uuid::new_v4();

    let node = MemoryNode::new("core", "fact", "User likes coffee", 0.8).with_runtime_fields(
        "preference",
        "user",
        0.9,
        0.8,
        "internal",
    );

    let id = memory.store(user_id, node).await.unwrap();
    assert!(!id.is_empty());

    let results = memory
        .retrieve(user_id, "coffee", RetrievalOpts::default())
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("coffee"));
}

#[tokio::test]
async fn test_update_and_delete() {
    let memory = AliaMemory::new();
    let user_id = Uuid::new_v4();

    let node = MemoryNode::new("core", "fact", "original content", 0.5);

    let id = memory.store(user_id, node).await.unwrap();

    // Update
    memory
        .update(
            user_id,
            &id,
            MemoryPatch {
                content: Some("updated content".to_string()),
                metadata: None,
                importance: Some(0.9),
                ..MemoryPatch::default()
            },
        )
        .await
        .unwrap();

    let results = memory
        .retrieve(user_id, "updated", RetrievalOpts::default())
        .await
        .unwrap();
    assert_eq!(results[0].content, "updated content");
    assert_eq!(results[0].importance, 0.9);

    // Delete
    memory.delete(user_id, &id).await.unwrap();

    let results = memory
        .retrieve(user_id, "updated", RetrievalOpts::default())
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_list_get_and_clear() {
    let memory = AliaMemory::new();
    let user_id = Uuid::new_v4();

    let first = memory
        .store(
            user_id,
            MemoryNode::new("core", "preference", "User prefers warm light", 0.9)
                .with_runtime_fields("preference", "user", 0.9, 0.8, "internal"),
        )
        .await
        .unwrap();
    memory
        .store(
            user_id,
            MemoryNode::new("episodic", "event", "User tested microphone volume", 0.4),
        )
        .await
        .unwrap();

    let listed = memory
        .list(
            user_id,
            MemoryListOpts {
                max_results: Some(10),
                layers: Some(vec!["core".to_string()]),
            },
        )
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id.as_deref(), Some(first.as_str()));

    let found = memory.get(user_id, &first).await.unwrap().unwrap();
    assert_eq!(found.content, "User prefers warm light");

    assert_eq!(memory.clear(user_id).await.unwrap(), 2);
    assert!(
        memory
            .list(user_id, MemoryListOpts::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn memory_capabilities_describe_built_in_store() {
    let simple = AliaMemory::new();
    let simple_caps = simple.memory_capabilities().await;
    assert_eq!(simple_caps.abi_version, 1);
    assert!(simple_caps.supports_store);
    assert!(simple_caps.supports_layer_filters);
    assert!(simple_caps.supports_similarity_threshold);

    let noop = NoopMemory;
    let noop_caps = noop.memory_capabilities().await;
    assert!(!noop_caps.supports_store);
    assert!(!noop_caps.supports_retrieve);
}

#[tokio::test]
async fn context_pack_suppresses_sensitive_and_private_memories() {
    let memory = AliaMemory::new();
    let user_id = Uuid::new_v4();

    memory
        .store(
            user_id,
            MemoryNode::new(
                "preference",
                "preference",
                "The user prefers concise engineering answers.",
                0.8,
            )
            .with_runtime_fields("preference", "user", 0.9, 0.8, "internal"),
        )
        .await
        .unwrap();
    let mut sensitive = MemoryNode::new("core", "fact", "The user's API key is sk-test", 0.9)
        .with_runtime_fields("semantic", "user", 0.9, 0.8, "private_runtime");
    sensitive.sensitive = true;
    memory.store(user_id, sensitive).await.unwrap();

    let pack = memory
        .retrieve_context_pack(
            user_id,
            "concise engineering API key",
            RetrievalOpts::default(),
        )
        .await
        .unwrap();

    assert_eq!(pack.user_preferences.len(), 1);
    assert!(pack.user_preferences[0].content.contains("concise"));
    assert_eq!(pack.suppressed.len(), 1);
    assert!(!serde_json::to_string(&pack).unwrap().contains("sk-test"));
}

#[tokio::test]
async fn memory_runtime_normalizes_evidence_topics_and_retrieval_trace() {
    let memory = AliaMemory::new();
    let user_id = Uuid::new_v4();

    let id = memory
        .store(
            user_id,
            MemoryNode::new(
                "project",
                "project",
                "The user is designing Memory Runtime graph retrieval for Expression Runtime.",
                0.8,
            )
            .with_runtime_fields("project", "user", 0.9, 0.7, "internal"),
        )
        .await
        .unwrap();

    let stored = memory.get(user_id, &id).await.unwrap().unwrap();
    assert!(
        stored
            .topic_tags
            .iter()
            .any(|topic| topic == "memory" || topic == "graph")
    );
    assert_eq!(
        stored.evidence["extractor"],
        serde_json::json!("alia_memory_runtime_v1")
    );
    assert_eq!(
        stored.evidence["policy_result"],
        serde_json::json!("accepted")
    );

    let pack = memory
        .retrieve_context_pack(
            user_id,
            "memory graph retrieval",
            RetrievalOpts {
                max_results: Some(5),
                min_similarity: Some(0.0),
                layers: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(pack.project_context.len(), 1);
    assert_eq!(pack.retrieval_trace.len(), 1);
    assert_eq!(pack.retrieval_trace[0].retrieval_path, "keyword_metadata");
    assert!(pack.retrieval_trace[0].final_score > 0.0);
}

#[tokio::test]
async fn hidden_memory_can_be_explained_and_deleted() {
    let memory = AliaMemory::new();
    let user_id = Uuid::new_v4();
    let mut hidden = MemoryNode::new("core", "fact", "Internal safety note.", 0.6)
        .with_runtime_fields("semantic", "user", 0.8, 0.6, "private_runtime");
    hidden.sensitive = true;
    let hidden_id = memory.store(user_id, hidden).await.unwrap();

    let explanation = memory
        .explain_suppressed(user_id, &hidden_id)
        .await
        .unwrap()
        .unwrap();
    assert!(explanation.suppressed);
    assert!(
        explanation
            .reason
            .as_deref()
            .unwrap()
            .contains("private_runtime")
    );

    assert_eq!(memory.delete_hidden(user_id).await.unwrap(), 1);
    assert!(
        memory
            .list(user_id, MemoryListOpts::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn visible_memory_is_not_reported_as_suppressed() {
    let memory = AliaMemory::new();
    let user_id = Uuid::new_v4();
    let id = memory
        .store(
            user_id,
            MemoryNode::new(
                "preference",
                "preference",
                "The user prefers direct answers.",
                0.8,
            )
            .with_runtime_fields("preference", "user", 0.9, 0.7, "user_visible"),
        )
        .await
        .unwrap();

    let explanation = memory
        .explain_suppressed(user_id, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(!explanation.suppressed);
    assert!(explanation.reason.is_none());
}

#[tokio::test]
async fn repeated_preference_is_consolidated_in_memory_runtime() {
    let memory = AliaMemory::new();
    let user_id = Uuid::new_v4();
    let first = MemoryNode::new(
        "preference",
        "preference",
        "The user prefers concise engineering answers.",
        0.5,
    )
    .with_runtime_fields("preference", "user", 0.7, 0.5, "internal");
    let second = MemoryNode::new(
        "preference",
        "preference",
        "The user prefers concise engineering answers.",
        0.9,
    )
    .with_runtime_fields("preference", "user", 0.9, 0.8, "internal");

    let first_id = memory.store(user_id, first).await.unwrap();
    let second_id = memory.store(user_id, second).await.unwrap();

    assert_eq!(first_id, second_id);
    let listed = memory
        .list(user_id, MemoryListOpts::default())
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].importance >= 0.9);
    assert!(listed[0].confidence >= 0.9);
}

#[tokio::test]
async fn background_consolidation_dedupes_promotes_demotes_and_reframes_affective_memory() {
    let memory = AliaMemory::new();
    let user_id = Uuid::new_v4();

    memory
        .store(
            user_id,
            MemoryNode::new("core", "fact", "The user works on Memory Runtime.", 0.7)
                .with_runtime_fields("semantic", "user", 0.7, 0.6, "internal"),
        )
        .await
        .unwrap();
    memory
        .store(
            user_id,
            MemoryNode::new("core", "fact", "The user works on Memory Runtime.", 0.8)
                .with_runtime_fields("semantic", "user", 0.8, 0.7, "internal"),
        )
        .await
        .unwrap();
    memory
        .store(
            user_id,
            MemoryNode::new(
                "episodic",
                "event",
                "User prefers direct implementation-focused answers.",
                0.72,
            )
            .with_runtime_fields("episodic", "user", 0.8, 0.7, "internal"),
        )
        .await
        .unwrap();
    memory
        .store(
            user_id,
            MemoryNode::new("core", "fact", "Weak outdated guess about the user.", 0.1)
                .with_runtime_fields("semantic", "user", 0.2, 0.2, "internal"),
        )
        .await
        .unwrap();
    let affective_id = memory
        .store(
            user_id,
            MemoryNode::new(
                "reflection",
                "insight",
                "The user was frustrated when the answer stayed abstract.",
                0.75,
            )
            .with_runtime_fields("affective_pattern", "user", 0.78, 0.65, "internal"),
        )
        .await
        .unwrap();

    let result = memory.consolidate_user(user_id).await.unwrap();

    assert_eq!(result.deduplicated, 1);
    assert_eq!(result.promoted, 1);
    assert_eq!(result.demoted, 1);
    assert_eq!(result.transformed_affective_guidance, 1);

    let active = memory
        .list(
            user_id,
            MemoryListOpts {
                max_results: Some(20),
                layers: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        active
            .iter()
            .filter(|memory| memory.content == "The user works on Memory Runtime.")
            .count(),
        1
    );
    assert!(active.iter().any(|memory| memory.kind == "preference"
        && memory.content.contains("direct implementation-focused")));
    let affective = memory.get(user_id, &affective_id).await.unwrap().unwrap();
    assert!(affective.content.starts_with("Constructive guidance:"));
}

#[tokio::test]
async fn memory_user_controls_delete_by_topic_kind_and_export_hidden_reasons() {
    let memory = AliaMemory::new();
    let user_id = Uuid::new_v4();

    let mut project = MemoryNode::new(
        "project",
        "project",
        "The user is building a garden app.",
        0.8,
    )
    .with_runtime_fields("project", "user", 0.9, 0.7, "internal");
    project.topic_tags = vec!["garden".to_string()];
    memory.store(user_id, project).await.unwrap();
    memory
        .store(
            user_id,
            MemoryNode::new("boundary", "boundary", "Do not use playful motion.", 0.8)
                .with_runtime_fields("boundary", "user", 0.9, 0.7, "internal"),
        )
        .await
        .unwrap();
    let mut private = MemoryNode::new("core", "fact", "Private runtime safety note.", 0.8)
        .with_runtime_fields("semantic", "user", 0.9, 0.7, "private_runtime");
    private.sensitive = true;
    memory.store(user_id, private).await.unwrap();

    let export = memory.export_user(user_id).await.unwrap();
    assert_eq!(export.suppressed.len(), 1);
    assert!(
        export.suppressed[0]
            .reason
            .contains("private_runtime_memory_hidden")
    );

    assert_eq!(memory.delete_by_topic(user_id, "garden").await.unwrap(), 1);
    assert_eq!(memory.delete_by_kind(user_id, "boundary").await.unwrap(), 1);
    let active = memory
        .list(user_id, MemoryListOpts::default())
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].visibility, "private_runtime");
}

#[test]
fn memory_runtime_migration_declares_pgvector_profile_and_graph_tables() {
    let sql = include_str!("../migrations/m20260520_000001_create_memory_runtime.sql");
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS embedding_profiles"));
    assert!(sql.contains("openai.text-embedding-3-small.1536.cosine.v1"));
    assert!(sql.contains("openai.text-embedding-3-large.3072.cosine.v1"));
    assert!(sql.contains("generic.embedding.1024.cosine.v1"));
    assert!(sql.contains("memory_embeddings_1536"));
    assert!(sql.contains("USING hnsw (embedding vector_cosine_ops)"));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS memory_nodes"));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS memory_edges"));
    assert!(sql.contains("PREFERS"));
    assert!(sql.contains("BOUNDARY_FOR"));
}

#[tokio::test]
async fn postgres_pgvector_memory_runtime_round_trips_embedding_vector_and_graph() {
    let database_url = std::env::var("ALIA_TEST_DATABASE_URL")
        .expect("set ALIA_TEST_DATABASE_URL to the disposable PostgreSQL/pgvector database");
    let admin = Database::connect(&database_url)
        .await
        .expect("test PostgreSQL must be reachable");
    let schema = format!("memory_runtime_test_{}", Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
        .await
        .unwrap();
    let mut options = ConnectOptions::new(database_url);
    options
        .max_connections(4)
        .set_schema_search_path(format!("{schema},public"));
    let db = Database::connect(options).await.unwrap();
    let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(30), async {
    alia_server::modules::platform::migrations::run(&db)
        .await
        .expect("install packaged schema");

    let user_id = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Memory runtime test','not-a-login-hash','user','active')",
        vec![user_id.into(),format!("{user_id}@memory-runtime.test").into()],
    )).await.unwrap();
    let memory = AliaMemory::new();
    memory
        .attach_runtime_for_test(
            db.clone(),
            Some(Arc::new(DeterministicEmbedding1536)),
            MemorySection::default(),
        )
        .await
        .expect("memory runtime should attach to test database");

    let mut motion = MemoryNode::new(
        "preference",
        "preference",
        "The user prefers low intensity motion and not exaggerated gestures.",
        0.9,
    )
    .with_runtime_fields("preference", "user", 0.92, 0.85, "internal")
    .with_metadata(serde_json::json!({
        "source": "test",
        "topics": ["motion", "expression"],
        "entities": ["Expression Runtime"],
    }));
    motion.topic_tags = vec!["motion".to_string(), "expression".to_string()];
    let motion_id = memory.store(user_id, motion).await.unwrap();

    let mut project = MemoryNode::new(
        "project",
        "project",
        "The user is working on Memory Runtime graph retrieval.",
        0.8,
    )
    .with_runtime_fields("project", "user", 0.9, 0.75, "internal");
    project.topic_tags = vec!["memory runtime".to_string(), "graph retrieval".to_string()];
    memory.store(user_id, project).await.unwrap();

    let embedding_count: i64 = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT COUNT(*)::bigint AS count
            FROM memory_embeddings_1536
            WHERE memory_id = $1::uuid
              AND profile_id = 'openai.text-embedding-3-small.1536.cosine.v1'
            "#,
            vec![motion_id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "count")
        .unwrap();
    assert_eq!(embedding_count, 1);

    let retrieved = memory
        .retrieve(
            user_id,
            "motion gestures",
            RetrievalOpts {
                max_results: Some(3),
                min_similarity: Some(0.0),
                layers: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(retrieved[0].id.as_deref(), Some(motion_id.as_str()));
    assert!(
        retrieved[0]
            .metadata
            .get("retrieval_score")
            .and_then(serde_json::Value::as_f64)
            .is_some()
    );

    let graph = memory
        .graph_for_user(
            user_id,
            MemoryListOpts {
                max_results: Some(50),
                layers: None,
            },
        )
        .await
        .unwrap();
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.relation_type == "PREFERS")
    );
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.relation_type == "RELATED_TO")
    );

    let pack = memory
        .retrieve_context_pack(user_id, "motion expression", RetrievalOpts::default())
        .await
        .unwrap();
    assert!(!pack.graph_context.is_empty());

    })).catch_unwind().await;
    db.close().await.unwrap();
    admin
        .execute_unprepared(&format!("DROP SCHEMA {schema} CASCADE"))
        .await
        .unwrap();
    admin.close().await.unwrap();
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("memory runtime test exceeded deadline"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

struct DeterministicEmbedding1536;

#[async_trait]
impl ModuleLifecycle for DeterministicEmbedding1536 {
    fn name(&self) -> &'static str {
        "deterministic_embedding_1536"
    }
}

#[async_trait]
impl EmbeddingAdapter for DeterministicEmbedding1536 {
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| deterministic_vector(text))
            .collect())
    }

    fn dimension(&self) -> usize {
        1536
    }
}

fn deterministic_vector(text: &str) -> Vec<f32> {
    let lower = text.to_ascii_lowercase();
    let mut vector = vec![0.001; 1536];
    if lower.contains("motion") || lower.contains("gesture") {
        vector[0] = 1.0;
    }
    if lower.contains("memory") || lower.contains("graph") {
        vector[1] = 1.0;
    }
    if lower.contains("coffee") {
        vector[2] = 1.0;
    }
    vector
}
