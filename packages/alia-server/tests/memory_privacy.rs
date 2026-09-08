//! Real PostgreSQL privacy/transaction tests. No environment means failure,
//! never a successful early return. Each database case owns a random schema.
use alia_server::{
    config::MemorySection,
    module::ModuleLifecycle,
    modules::platform::migrations,
    providers::{
        MemoryBackend,
        adapter::{EmbeddingAdapter, MemoryListOpts, MemoryNode, MemoryPatch, RetrievalOpts},
        memory::AliaMemory,
    },
};
use async_trait::async_trait;
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use std::{
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;
use uuid::Uuid;

static DATABASE_CASES: Semaphore = Semaphore::const_new(4);
#[derive(Clone)]
struct TestDatabase {
    admin: DatabaseConnection,
    db: DatabaseConnection,
    schema: String,
}
impl TestDatabase {
    async fn new() -> Self {
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("set ALIA_TEST_DATABASE_URL to a disposable PostgreSQL database");
        let admin = Database::connect(&url)
            .await
            .expect("test PostgreSQL must be reachable");
        assert_eq!(admin.get_database_backend(), DatabaseBackend::Postgres);
        let schema = format!("memory_privacy_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(4)
            .min_connections(1)
            .connect_timeout(Duration::from_secs(5))
            .acquire_timeout(Duration::from_secs(10))
            .set_schema_search_path(format!("{schema},public"))
            .sqlx_logging(false);
        let db = Database::connect(options).await.unwrap();
        let fixture = Self { admin, db, schema };
        if let Err(error) = migrations::run(&fixture.db).await {
            fixture.cleanup().await;
            panic!("install bundled schema: {error:#}");
        }
        fixture
    }
    async fn memory(&self, embedding: Option<Arc<dyn EmbeddingAdapter>>) -> Arc<AliaMemory> {
        let memory = Arc::new(AliaMemory::new());
        memory
            .attach_runtime_for_test(self.db.clone(), embedding, MemorySection::default())
            .await
            .unwrap();
        memory
    }
    async fn sql(&self, sql: &str) {
        self.db.execute_unprepared(sql).await.unwrap();
    }
    async fn count(&self, table: &str) -> i64 {
        assert!(table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        self.db
            .query_one(Statement::from_string(
                DatabaseBackend::Postgres,
                format!("SELECT COUNT(*) AS count FROM {table}"),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "count")
            .unwrap()
    }
    async fn cleanup(self) {
        self.db.close().await.unwrap();
        self.admin
            .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .await
            .unwrap();
        self.admin.close().await.unwrap();
    }
}
async fn with_database<F, Fut>(test: F)
where
    F: FnOnce(TestDatabase) -> Fut,
    Fut: Future<Output = ()>,
{
    let _slot = DATABASE_CASES.acquire().await.unwrap();
    let database = TestDatabase::new().await;
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(45),
        test(database.clone()),
    ))
    .catch_unwind()
    .await;
    database.cleanup().await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("memory test exceeded 45 seconds"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
fn node(content: &str) -> MemoryNode {
    MemoryNode::new("core", "fact", content, 0.8)
        .with_runtime_fields("semantic", "user", 0.8, 0.7, "internal")
}
async fn derived(memory: &AliaMemory, user: Uuid, source: &str, content: &str) -> String {
    let original = memory.get(user, source).await.unwrap().unwrap();
    let mut context = memory.write_snapshot(user).await.unwrap();
    context
        .source_memories
        .push((source.to_string(), original.revision));
    memory.store_guarded(&context, node(content)).await.unwrap()
}

#[tokio::test]
async fn automatic_and_batch_dedup_preserve_unicode_negation_and_punctuation() {
    with_database(|database| async move {
        let memory = database.memory(None).await;
        for (left, right) in [
            ("Project scale 1.5", "Project scale 15"),
            ("Project rule can't", "Project rule cant"),
            ("Project rule can swim", "Project rule can't swim"),
            ("用户喜欢咖啡", "用户不喜欢咖啡"),
            ("用户喜欢咖啡", "用户住在上海"),
        ] {
            let user = Uuid::new_v4();
            let project = |content: &str| {
                MemoryNode::new("core", "fact", content, 0.8)
                    .with_runtime_fields("project", "user", 0.8, 0.7, "internal")
            };
            let first = memory.store(user, project(left)).await.unwrap();
            let second = memory.store(user, project(right)).await.unwrap();
            assert_ne!(first, second, "distinct facts merged: {left:?} / {right:?}");
            assert_eq!(memory.store(user, project(left)).await.unwrap(), first);
            let result = memory.consolidate_user(user).await.unwrap();
            assert_eq!(result.deduplicated, 0, "batch merged {left:?} / {right:?}");
            let items = memory.list(user, MemoryListOpts::default()).await.unwrap();
            assert_eq!(items.len(), 2);
            assert!(items.iter().any(|item| item.content == left));
            assert!(items.iter().any(|item| item.content == right));
        }
    })
    .await;
}

#[tokio::test]
async fn deleting_source_erases_derived_records_and_graph_plaintext_atomically() {
    with_database(|database|async move{
        let memory=database.memory(None).await;
        let user=Uuid::new_v4();let other=Uuid::new_v4();
        let base=memory.store(user,node("private source maple orchard")).await.unwrap();
        let child=derived(&memory,user,&base,"derived maple orchard summary").await;
        let grandchild=derived(&memory,user,&child,"second generation maple orchard insight").await;
        let unrelated=memory.store(other,node("other user's separate fact")).await.unwrap();
        assert!(database.count("memory_edges").await>0);
        memory.delete(user,&base).await.unwrap();
        for id in [&base,&child,&grandchild] {assert!(memory.get(user,id).await.unwrap().is_none());}
        assert!(memory.get(other,&unrelated).await.unwrap().is_some());
        let pack=memory.retrieve_context_pack(user,"maple",RetrievalOpts::default()).await.unwrap();
        assert!(!serde_json::to_string(&pack).unwrap().contains("maple"));
        let graph=memory.graph_for_user(user,MemoryListOpts::default()).await.unwrap();
        assert!(graph.nodes.is_empty()&&graph.edges.is_empty());
        let retained:i64=database.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT COUNT(*) AS count FROM memory_items WHERE user_id=$1 AND status='deleted' AND content='' AND summary IS NULL AND metadata='{}'::jsonb AND evidence='{}'::jsonb AND cardinality(topic_tags)=0",[user.into()])).await.unwrap().unwrap().try_get("","count").unwrap();
        assert_eq!(retained,3);
        assert_eq!(database.count("memory_sources").await,0);
    }).await;
}

#[tokio::test]
async fn source_delete_and_derived_commit_race_cannot_leave_a_live_copy() {
    with_database(|database| async move {
        let first = database.memory(None).await;
        let second = database.memory(None).await;
        let user = Uuid::new_v4();
        for index in 0..8 {
            let base = first
                .store(user, node(&format!("source number {index}")))
                .await
                .unwrap();
            let mut context = second.write_snapshot(user).await.unwrap();
            context.source_memories.push((
                base.clone(),
                first.get(user, &base).await.unwrap().unwrap().revision,
            ));
            let (stored, deleted) = tokio::join!(
                second.store_guarded(&context, node("derived race copy")),
                first.delete(user, &base)
            );
            deleted.unwrap();
            if let Ok(id) = stored {
                assert!(second.get(user, &id).await.unwrap().is_none());
            }
            assert!(
                second
                    .list(user, MemoryListOpts::default())
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    })
    .await;
}

#[tokio::test]
async fn deletion_sql_failure_rolls_back_epoch_and_projections_without_ram_success() {
    with_database(|database|async move{
        let memory=database.memory(None).await;let user=Uuid::new_v4();
        let id=memory.store(user,node("retain until transaction commits")).await.unwrap();
        let epoch=memory.write_snapshot(user).await.unwrap().privacy_epoch;
        let graph_count=database.count("memory_edges").await;
        database.sql("CREATE FUNCTION reject_forget() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.status='deleted' THEN RAISE EXCEPTION 'injected forget failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_forget BEFORE UPDATE ON memory_items FOR EACH ROW EXECUTE FUNCTION reject_forget()").await;
        assert!(memory.delete(user,&id).await.is_err());
        assert!(memory.clear(user).await.is_err());
        assert!(memory.delete_by_topic(user,"transaction").await.is_err());
        assert_eq!(memory.write_snapshot(user).await.unwrap().privacy_epoch,epoch);
        assert_eq!(memory.get(user,&id).await.unwrap().unwrap().content,"retain until transaction commits");
        assert_eq!(database.count("memory_edges").await,graph_count);
        database.sql("DROP TRIGGER reject_forget ON memory_items").await;
        memory.delete(user,&id).await.unwrap();
        assert_eq!(memory.write_snapshot(user).await.unwrap().privacy_epoch,epoch+1);
    }).await;
}

#[tokio::test]
async fn configured_database_failure_never_switches_to_in_memory_storage() {
    with_database(|database| async move {
        let memory = database.memory(None).await;
        let user = Uuid::new_v4();
        let id = memory.store(user, node("durable record")).await.unwrap();
        database.db.clone().close().await.unwrap();
        assert!(memory.store(user, node("not persisted")).await.is_err());
        assert!(memory.delete(user, &id).await.is_err());
        assert!(memory.clear(user).await.is_err());
        assert!(memory.get(user, &id).await.is_err());
        assert!(memory.list(user, MemoryListOpts::default()).await.is_err());
        assert!(
            memory
                .graph_for_user(user, MemoryListOpts::default())
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn old_epoch_is_rejected_across_instances_and_empty_clear_also_invalidates() {
    with_database(|database| async move {
        let first = database.memory(None).await;
        let second = database.memory(None).await;
        let user = Uuid::new_v4();
        let old = first.write_snapshot(user).await.unwrap();
        assert_eq!(second.clear(user).await.unwrap(), 0);
        assert!(
            first
                .store_guarded(&old, node("late extractor result"))
                .await
                .is_err()
        );
        let new = first.write_snapshot(user).await.unwrap();
        assert!(new.privacy_epoch > old.privacy_epoch);
        first
            .store_guarded(&new, node("new user input remains permitted"))
            .await
            .unwrap();
        assert_eq!(
            first
                .list(user, MemoryListOpts::default())
                .await
                .unwrap()
                .len(),
            1
        );
    })
    .await;
}

#[tokio::test]
async fn updates_deletes_ids_and_dependencies_are_owner_scoped() {
    with_database(|database| async move {
        let memory = database.memory(None).await;
        let alice = Uuid::new_v4();
        let bob = Uuid::new_v4();
        let id = memory.store(alice, node("Alice's fact")).await.unwrap();
        assert!(
            memory
                .update(
                    bob,
                    &id,
                    MemoryPatch {
                        content: Some("overwrite".into()),
                        ..Default::default()
                    }
                )
                .await
                .is_err()
        );
        assert!(memory.delete(bob, &id).await.is_err());
        let mut forged = node("foreign ID replacement");
        forged.id = Some(id.clone());
        assert!(memory.store(bob, forged).await.is_err());
        let mut context = memory.write_snapshot(bob).await.unwrap();
        context.source_memories.push((id.clone(), 1));
        assert!(
            memory
                .store_guarded(&context, node("cross-user derivation"))
                .await
                .is_err()
        );
        assert_eq!(
            memory.get(alice, &id).await.unwrap().unwrap().content,
            "Alice's fact"
        );
        assert!(
            memory
                .list(bob, MemoryListOpts::default())
                .await
                .unwrap()
                .is_empty()
        );
        memory.delete(alice, &id).await.unwrap();
        let mut resurrect = node("reuse deleted ID");
        resurrect.id = Some(id);
        assert!(memory.store(alice, resurrect).await.is_err());
    })
    .await;
}

#[tokio::test]
async fn changed_revision_and_expired_sources_reject_background_writes() {
    with_database(|database| async move {
        let memory = database.memory(None).await;
        let user = Uuid::new_v4();
        let id = memory.store(user, node("old content")).await.unwrap();
        let mut context = memory.write_snapshot(user).await.unwrap();
        context.source_memories.push((id.clone(), 1));
        memory
            .update(
                user,
                &id,
                MemoryPatch {
                    content: Some("corrected content".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(
            memory
                .store_guarded(&context, node("stale interpretation"))
                .await
                .is_err()
        );
        assert!(
            memory
                .update_guarded(
                    &memory.write_snapshot(user).await.unwrap(),
                    &id,
                    MemoryPatch {
                        expected_revision: Some(1),
                        content: Some("stale overwrite".into()),
                        ..Default::default()
                    }
                )
                .await
                .is_err()
        );
        let mut fresh = memory.write_snapshot(user).await.unwrap();
        fresh.source_memories.push((id.clone(), 2));
        database
            .sql("UPDATE memory_items SET expires_at=clock_timestamp()-INTERVAL '1 second'")
            .await;
        assert!(
            memory
                .store_guarded(&fresh, node("derived from expired source"))
                .await
                .is_err()
        );
        let pack = memory
            .retrieve_context_pack(user, "content", RetrievalOpts::default())
            .await
            .unwrap();
        assert!(!serde_json::to_string(&pack).unwrap().contains("content"));
        assert!(
            memory
                .graph_for_user(user, MemoryListOpts::default())
                .await
                .unwrap()
                .nodes
                .is_empty()
        );
    })
    .await;
}

#[tokio::test]
async fn late_persisted_conversation_source_cannot_cross_the_forget_boundary() {
    with_database(|database|async move{
        let memory=database.memory(None).await;let user=Uuid::new_v4();let session=Uuid::new_v4();
        let before=memory.write_snapshot(user).await.unwrap();
        memory.clear(user).await.unwrap();
        database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'test',$2)",vec![session.into(),user.to_string().into()])).await.unwrap();
        let source=Uuid::new_v4();
        database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at) VALUES($1,$2,$3,'user','input received before deletion',$4)",vec![source.into(),session.into(),user.to_string().into(),before.source_received_at.into()])).await.unwrap();
        let mut restarted=memory.write_snapshot(user).await.unwrap();restarted.source_event_ids.push(source);
        assert!(memory.store_guarded(&restarted,node("new job inspecting old late-persisted message")).await.is_err());
        let fresh=Uuid::new_v4();
        database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at) VALUES($1,$2,$3,'user','genuinely new input',clock_timestamp())",vec![fresh.into(),session.into(),user.to_string().into()])).await.unwrap();
        let mut current=memory.write_snapshot(user).await.unwrap();current.source_event_ids.push(fresh);
        let id=memory.store_guarded(&current,node("new input accepted")).await.unwrap();
        assert_eq!(memory.get(user,&id).await.unwrap().unwrap().source_event_ids,vec![fresh]);
        let mut foreign=memory.write_snapshot(Uuid::new_v4()).await.unwrap();foreign.source_event_ids.push(fresh);
        assert!(memory.store_guarded(&foreign,node("foreign message source")).await.is_err());
    }).await;
}

struct HeldEmbedding {
    calls: AtomicUsize,
    started: Semaphore,
    release: Semaphore,
}

#[tokio::test]
async fn deterministic_consolidation_commits_functional_batch_and_tracks_promoted_source() {
    with_database(|database| async move {
        let memory = database.memory(None).await;
        let user = Uuid::new_v4();
        memory
            .store(user, node("The user works on Memory Runtime."))
            .await
            .unwrap();
        memory
            .store(user, node("The user works on Memory Runtime."))
            .await
            .unwrap();
        let episode = memory
            .store(
                user,
                MemoryNode::new(
                    "episodic",
                    "event",
                    "User prefers concise engineering answers.",
                    0.8,
                )
                .with_runtime_fields("episodic", "user", 0.8, 0.7, "internal"),
            )
            .await
            .unwrap();
        let weak = memory
            .store(
                user,
                MemoryNode::new("core", "fact", "Weak old guess", 0.1)
                    .with_runtime_fields("semantic", "user", 0.2, 0.2, "internal"),
            )
            .await
            .unwrap();
        let affective = memory
            .store(
                user,
                MemoryNode::new(
                    "reflection",
                    "insight",
                    "The user was frustrated when answers stayed abstract.",
                    0.8,
                )
                .with_runtime_fields(
                    "affective_pattern",
                    "user",
                    0.8,
                    0.7,
                    "internal",
                ),
            )
            .await
            .unwrap();
        let result = memory.consolidate_user(user).await.unwrap();
        assert_eq!(result.deduplicated, 1);
        assert_eq!(result.promoted, 1);
        assert_eq!(result.demoted, 1);
        assert_eq!(result.transformed_affective_guidance, 1);
        assert_eq!(
            memory.get(user, &weak).await.unwrap().unwrap().status,
            "expired"
        );
        assert!(
            memory
                .get(user, &affective)
                .await
                .unwrap()
                .unwrap()
                .content
                .starts_with("Constructive guidance:")
        );
        let active = memory.list(user, MemoryListOpts::default()).await.unwrap();
        let promoted = active
            .iter()
            .find(|node| node.kind == "preference")
            .unwrap()
            .id
            .clone()
            .unwrap();
        memory.delete(user, &episode).await.unwrap();
        assert!(memory.get(user, &promoted).await.unwrap().is_none());
        assert!(
            memory.get(user, &affective).await.unwrap().is_some(),
            "deleting a source must not delete unrelated memories"
        );
    })
    .await;
}

#[tokio::test]
async fn consolidation_of_weak_duplicates_does_not_depend_on_uuid_update_order() {
    with_database(|database| async move {
        let memory = database.memory(None).await;
        let user = Uuid::new_v4();
        let mut keeper = node("Weak duplicated guess");
        keeper.id = Some("00000000-0000-0000-0000-000000000001".into());
        keeper.importance = 0.2;
        keeper.confidence = 0.2;
        let mut duplicate = keeper.clone();
        duplicate.id = Some("00000000-0000-0000-0000-000000000002".into());
        duplicate.importance = 0.1;
        memory.store(user, keeper).await.unwrap();
        memory.store(user, duplicate).await.unwrap();
        let result = memory.consolidate_user(user).await.unwrap();
        assert_eq!(result.deduplicated, 1);
        assert_eq!(result.demoted, 2);
        assert!(
            memory
                .list(user, MemoryListOpts::default())
                .await
                .unwrap()
                .is_empty()
        );
    })
    .await;
}
impl HeldEmbedding {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }
}
#[async_trait]
impl ModuleLifecycle for HeldEmbedding {
    fn name(&self) -> &'static str {
        "held_test_embedding"
    }
}
#[async_trait]
impl EmbeddingAdapter for HeldEmbedding {
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            self.started.add_permits(1);
            self.release.acquire().await?.forget();
        }
        Ok(texts
            .iter()
            .map(|_| {
                let mut vector = vec![0.0; 1536];
                vector[0] = (call + 1) as f32;
                vector
            })
            .collect())
    }
    fn dimension(&self) -> usize {
        1536
    }
}

#[tokio::test]
async fn embedding_returning_after_delete_cannot_restore_a_vector() {
    with_database(|database| async move {
        let embedding = Arc::new(HeldEmbedding::new());
        let memory = database.memory(Some(embedding.clone())).await;
        let user = Uuid::new_v4();
        let id = Uuid::new_v4().to_string();
        let mut input = node("late vector for deleted memory");
        input.id = Some(id.clone());
        let store = tokio::spawn({
            let memory = memory.clone();
            async move { memory.store(user, input).await }
        });
        embedding.started.acquire().await.unwrap().forget();
        memory.delete(user, &id).await.unwrap();
        embedding.release.add_permits(1);
        store.await.unwrap().unwrap();
        assert_eq!(database.count("memory_embeddings_1536").await, 0);
        assert!(memory.get(user, &id).await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
async fn older_embedding_cannot_overwrite_the_corrected_revision() {
    with_database(|database| async move {
        let embedding = Arc::new(HeldEmbedding::new());
        let memory = database.memory(Some(embedding.clone())).await;
        let user = Uuid::new_v4();
        let id = Uuid::new_v4().to_string();
        let mut input = node("original content");
        input.id = Some(id.clone());
        let store = tokio::spawn({
            let memory = memory.clone();
            async move { memory.store(user, input).await }
        });
        embedding.started.acquire().await.unwrap().forget();
        memory
            .update(
                user,
                &id,
                MemoryPatch {
                    content: Some("corrected content".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        embedding.release.add_permits(1);
        store.await.unwrap().unwrap();
        let row = database
            .db
            .query_one(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT source_revision,embedding::text AS vector FROM memory_embeddings_1536",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<i64>("", "source_revision").unwrap(), 2);
        assert!(
            row.try_get::<String>("", "vector")
                .unwrap()
                .starts_with("[2,")
        );
        assert_eq!(
            memory.get(user, &id).await.unwrap().unwrap().content,
            "corrected content"
        );
    })
    .await;
}

#[tokio::test]
async fn memory_mode_enforces_owner_epoch_revision_and_dependency_cascade() {
    let memory = AliaMemory::new();
    let user = Uuid::new_v4();
    let other = Uuid::new_v4();
    let id = memory.store(user, node("source")).await.unwrap();
    let child = derived(&memory, user, &id, "derived").await;
    let old = memory.write_snapshot(user).await.unwrap();
    assert!(memory.delete(other, &id).await.is_err());
    assert!(
        memory
            .update(
                other,
                &id,
                MemoryPatch {
                    content: Some("wrong owner".into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    memory.delete(user, &id).await.unwrap();
    assert!(memory.get(user, &child).await.unwrap().is_none());
    assert!(
        memory
            .store_guarded(&old, node("late in-memory job"))
            .await
            .is_err()
    );
    let fresh = memory.store(user, node("new input")).await.unwrap();
    memory
        .update(
            user,
            &fresh,
            MemoryPatch {
                expected_revision: Some(1),
                content: Some("new revision".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        memory
            .update(
                user,
                &fresh,
                MemoryPatch {
                    expected_revision: Some(1),
                    content: Some("stale".into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        memory.get(user, &fresh).await.unwrap().unwrap().content,
        "new revision"
    );
}
