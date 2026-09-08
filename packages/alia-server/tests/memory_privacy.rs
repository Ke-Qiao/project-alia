//! Real PostgreSQL privacy/transaction tests. No environment means failure,
//! never a successful early return. Each database case owns a random schema.
use alia_server::{
    config::MemorySection,
    module::ModuleLifecycle,
    modules::platform::migrations,
    providers::{
        MemoryBackend,
        adapter::{
            EmbeddingAdapter, MemoryBatch, MemoryListOpts, MemoryNode, MemoryPatch,
            MemoryWriteContext, RetrievalOpts,
        },
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

async fn selected_context(memory: &AliaMemory, user: Uuid, ids: &[&str]) -> MemoryWriteContext {
    let mut context = memory.write_snapshot(user).await.unwrap();
    for id in ids {
        let item = memory.get(user, id).await.unwrap().unwrap();
        context
            .source_memories
            .push((id.to_string(), item.revision));
    }
    context
}

fn batch_update(id: &str, revision: i64, content: &str) -> (String, MemoryPatch) {
    (
        id.to_string(),
        MemoryPatch {
            expected_revision: Some(revision),
            content: Some(content.into()),
            ..Default::default()
        },
    )
}

async fn atomic_batch_and_lineage_contract(memory: &AliaMemory) {
    let user = Uuid::new_v4();
    let source = memory
        .store(user, node("original source before replacement metadata"))
        .await
        .unwrap();
    let first = derived(memory, user, &source, "first interpretation").await;
    let second = memory
        .store(user, node("second interpretation"))
        .await
        .unwrap();
    let context = selected_context(memory, user, &[&first, &second]).await;
    let created_id = Uuid::new_v4().to_string();
    let mut created = node("combined interpretation");
    created.id = Some(created_id.clone());
    let mut update = batch_update(&first, 1, "corrected first interpretation");
    update.1.metadata =
        Some(serde_json::json!({"reason":"replace metadata, retain old provenance"}));
    let result = memory
        .apply_batch_guarded(
            &context,
            MemoryBatch {
                updates: vec![
                    update,
                    (
                        second.clone(),
                        MemoryPatch {
                            expected_revision: Some(1),
                            status: Some("superseded".into()),
                            superseded_by: Some(created_id.clone()),
                            ..Default::default()
                        },
                    ),
                ],
                creates: vec![created],
            },
        )
        .await
        .unwrap();
    assert_eq!(result.updated, 2);
    assert_eq!(result.created_ids, vec![created_id.clone()]);
    assert_eq!(memory.get(user, &first).await.unwrap().unwrap().revision, 2);
    let retired = memory.get(user, &second).await.unwrap().unwrap();
    assert_eq!(retired.revision, 2);
    assert_eq!(retired.status, "superseded");
    assert_eq!(retired.content, "second interpretation");
    assert_eq!(
        memory.write_snapshot(user).await.unwrap().privacy_epoch,
        context.privacy_epoch
    );

    // Replacement metadata must not remove the original source->first edge.
    // The new read set also links the combined record and retired source.
    let forget = selected_context(memory, user, &[&source]).await;
    let later = memory
        .store(user, node("later unrelated original source"))
        .await
        .unwrap();
    assert_eq!(
        memory
            .forget_many_guarded(&forget, &forget.source_memories)
            .await
            .unwrap(),
        4
    );
    for id in [&source, &first, &second, &created_id] {
        assert!(memory.get(user, id).await.unwrap().is_none());
    }
    assert!(memory.get(user, &later).await.unwrap().is_some());
    assert_eq!(
        memory.write_snapshot(user).await.unwrap().privacy_epoch,
        context.privacy_epoch + 1
    );
    assert!(
        memory
            .apply_batch_guarded(
                &context,
                MemoryBatch {
                    creates: vec![node("late old interpretation")],
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
}

async fn exact_forget_selection_contract(memory: &AliaMemory) {
    let user = Uuid::new_v4();
    let first = memory
        .store(user, node("selected first fact"))
        .await
        .unwrap();
    let second = memory
        .store(user, node("selected second fact"))
        .await
        .unwrap();
    let original = selected_context(memory, user, &[&first, &second]).await;
    let later = memory
        .store(user, node("selected later fact"))
        .await
        .unwrap();
    assert!(
        memory
            .forget_many_guarded(&original, &[(later.clone(), 1)])
            .await
            .is_err()
    );
    memory
        .update(
            user,
            &second,
            MemoryPatch {
                content: Some("corrected after selection".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        memory
            .forget_many_guarded(&original, &original.source_memories)
            .await
            .is_err()
    );
    assert!(memory.get(user, &first).await.unwrap().is_some());
    assert_eq!(
        memory.write_snapshot(user).await.unwrap().privacy_epoch,
        original.privacy_epoch
    );
    let fresh = selected_context(memory, user, &[&first, &second]).await;
    assert_eq!(
        memory
            .forget_many_guarded(&fresh, &fresh.source_memories)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        memory.write_snapshot(user).await.unwrap().privacy_epoch,
        fresh.privacy_epoch + 1
    );
    assert!(memory.get(user, &later).await.unwrap().is_some());
    assert!(
        memory
            .forget_many_guarded(&fresh, &fresh.source_memories)
            .await
            .is_err()
    );
    assert_eq!(memory.forget_many_guarded(&fresh, &[]).await.unwrap(), 0);
    assert_eq!(
        memory.write_snapshot(user).await.unwrap().privacy_epoch,
        fresh.privacy_epoch + 1
    );

    // A logically retired source remains erasable, and forgetting it must
    // erase its already committed derivative too.
    let retirement = selected_context(memory, user, &[&later]).await;
    let result = memory
        .apply_batch_guarded(
            &retirement,
            MemoryBatch {
                updates: vec![(
                    later.clone(),
                    MemoryPatch {
                        expected_revision: Some(1),
                        status: Some("rejected".into()),
                        ..Default::default()
                    },
                )],
                creates: vec![node("derived before logical source retirement")],
            },
        )
        .await
        .unwrap();
    let retired_selection = selected_context(memory, user, &[&later]).await;
    assert!(
        memory
            .store_guarded(
                &retired_selection,
                node("cannot newly derive from retired source")
            )
            .await
            .is_err()
    );
    assert_eq!(
        memory
            .forget_many_guarded(&retired_selection, &retired_selection.source_memories)
            .await
            .unwrap(),
        2
    );
    assert!(
        memory
            .get(user, &result.created_ids[0])
            .await
            .unwrap()
            .is_none()
    );
}

async fn invalid_batch_contract(memory: &AliaMemory) {
    let user = Uuid::new_v4();
    let other = Uuid::new_v4();
    let existing = memory
        .store(user, node("unchanged valid source"))
        .await
        .unwrap();
    let foreign = memory
        .store(other, node("other owner source"))
        .await
        .unwrap();
    let context = selected_context(memory, user, &[&existing]).await;
    let mut colliding = node("cannot overwrite existing identity");
    colliding.id = Some(existing.clone());
    assert!(
        memory
            .apply_batch_guarded(
                &context,
                MemoryBatch {
                    creates: vec![node("first creation must roll back"), colliding],
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    for status in ["deleted", "expired"] {
        assert!(
            memory
                .apply_batch_guarded(
                    &context,
                    MemoryBatch {
                        updates: vec![(
                            existing.clone(),
                            MemoryPatch {
                                expected_revision: Some(1),
                                status: Some(status.into()),
                                ..Default::default()
                            }
                        )],
                        ..Default::default()
                    }
                )
                .await
                .is_err()
        );
    }
    assert!(
        memory
            .apply_batch_guarded(
                &context,
                MemoryBatch {
                    updates: vec![
                        batch_update(&existing, 1, "first"),
                        batch_update(&existing, 1, "duplicate")
                    ],
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let mut forged = context.clone();
    forged.source_memories.push((foreign.clone(), 1));
    assert!(
        memory
            .apply_batch_guarded(
                &forged,
                MemoryBatch {
                    updates: vec![batch_update(&foreign, 1, "foreign replacement")],
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert!(
        memory
            .forget_many_guarded(&forged, &[(existing.clone(), 1), (foreign.clone(), 1)])
            .await
            .is_err()
    );
    assert_eq!(
        memory
            .list(user, MemoryListOpts::default())
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        memory.get(user, &existing).await.unwrap().unwrap().revision,
        1
    );
    assert_eq!(
        memory.get(other, &foreign).await.unwrap().unwrap().content,
        "other owner source"
    );
    assert_eq!(
        memory.write_snapshot(user).await.unwrap().privacy_epoch,
        context.privacy_epoch
    );
}

#[tokio::test]
async fn postgres_batches_preserve_original_snapshot_and_prior_lineage() {
    with_database(|database| async move {
        atomic_batch_and_lineage_contract(database.memory(None).await.as_ref()).await;
    })
    .await;
}

#[tokio::test]
async fn postgres_forget_uses_one_epoch_and_never_reselects_later_records() {
    with_database(|database| async move {
        exact_forget_selection_contract(database.memory(None).await.as_ref()).await;
    })
    .await;
}

#[tokio::test]
async fn postgres_invalid_batches_and_foreign_forget_never_partially_commit() {
    with_database(|database| async move {
        invalid_batch_contract(database.memory(None).await.as_ref()).await;
    })
    .await;
}

#[tokio::test]
async fn batch_update_keeps_old_conversation_sources_and_adds_new_verified_sources() {
    with_database(|database| async move {
        let memory = database.memory(None).await;
        let user = Uuid::new_v4();
        let session = Uuid::new_v4();
        database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'batch-source',$2)",
            vec![session.into(),user.to_string().into()])).await.unwrap();
        let mut events = Vec::new();
        let epoch = memory.write_snapshot(user).await.unwrap().privacy_epoch;
        for content in ["first observed source", "second observed source"] {
            let id = Uuid::new_v4();
            database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user',$4,clock_timestamp(),$5)",
                vec![id.into(),session.into(),user.to_string().into(),content.into(),epoch.into()])).await.unwrap();
            events.push(id);
        }
        let mut original = memory.write_snapshot(user).await.unwrap();
        original.source_event_ids.push(events[0]);
        let id = memory.store_guarded(&original, node("original source fact")).await.unwrap();
        let mut next = selected_context(&memory, user, &[&id]).await;
        next.source_event_ids.push(events[1]);
        memory.apply_batch_guarded(&next, MemoryBatch {
            updates: vec![batch_update(&id, 1, "fact corrected by new source")],
            ..Default::default()
        }).await.unwrap();
        let stored = memory.get(user, &id).await.unwrap().unwrap();
        assert_eq!(stored.revision, 2);
        assert_eq!(stored.source_event_ids.len(), 2);
        for id in events { assert!(stored.source_event_ids.contains(&id)); }
    }).await;
}

#[tokio::test]
async fn memory_mode_atomic_batches_and_exact_forget_match_persistent_contract() {
    let memory = AliaMemory::new();
    atomic_batch_and_lineage_contract(&memory).await;
    exact_forget_selection_contract(&memory).await;
    invalid_batch_contract(&memory).await;
}

#[tokio::test]
async fn sql_failure_rolls_back_entire_batch_and_multi_target_forget() {
    with_database(|database| async move {
        let memory = database.memory(None).await;
        let user = Uuid::new_v4();
        let first = memory.store(user, node("first unchanged fact")).await.unwrap();
        let second = memory.store(user, node("second unchanged fact")).await.unwrap();
        let context = selected_context(&memory, user, &[&first, &second]).await;
        let graph_count = database.count("memory_edges").await;
        database.sql("CREATE FUNCTION reject_batch_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.content='injected failing update' OR NEW.status='deleted' THEN RAISE EXCEPTION 'injected batch rejection'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_batch_update BEFORE UPDATE ON memory_items FOR EACH ROW EXECUTE FUNCTION reject_batch_update()").await;
        let mut ordered = [first.clone(), second.clone()]; ordered.sort();
        assert!(memory.apply_batch_guarded(&context, MemoryBatch {
            updates: vec![batch_update(&ordered[0], 1, "first update before failure"), batch_update(&ordered[1], 1, "injected failing update")],
            creates: vec![node("new record before failing update")],
        }).await.is_err());
        assert_eq!(database.count("memory_items").await, 2);
        assert_eq!(database.count("memory_edges").await, graph_count);
        assert_eq!(database.count("memory_sources").await, 0);
        assert_eq!(memory.get(user, &first).await.unwrap().unwrap().content, "first unchanged fact");
        assert_eq!(memory.get(user, &second).await.unwrap().unwrap().revision, 1);
        assert!(memory.forget_many_guarded(&context, &context.source_memories).await.is_err());
        assert_eq!(memory.write_snapshot(user).await.unwrap().privacy_epoch, context.privacy_epoch);
        assert_eq!(memory.list(user, MemoryListOpts::default()).await.unwrap().len(), 2);
        assert_eq!(database.count("memory_edges").await, graph_count);
        database.sql("DROP TRIGGER reject_batch_update ON memory_items").await;
        database.sql("CREATE FUNCTION reject_batch_source() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected provenance rejection'; END $$; CREATE TRIGGER reject_batch_source BEFORE INSERT ON memory_sources FOR EACH ROW EXECUTE FUNCTION reject_batch_source()").await;
        assert!(memory.apply_batch_guarded(&context, MemoryBatch {
            updates: vec![batch_update(&first, 1, "update before provenance failure")],
            creates: vec![node("create before provenance failure")],
        }).await.is_err());
        assert_eq!(database.count("memory_items").await, 2);
        assert_eq!(database.count("memory_sources").await, 0);
        assert_eq!(database.count("memory_edges").await, graph_count);
        assert_eq!(memory.get(user, &first).await.unwrap().unwrap().revision, 1);
        database.sql("DROP TRIGGER reject_batch_source ON memory_sources").await;
        assert_eq!(memory.forget_many_guarded(&context, &context.source_memories).await.unwrap(), 2);
    }).await;
}

#[tokio::test]
async fn concurrent_batch_and_forget_cannot_commit_against_a_changed_original() {
    with_database(|database| async move {
        let first = database.memory(None).await;
        let second = database.memory(None).await;
        let user = Uuid::new_v4();
        for index in 0..8 {
            let source = first
                .store(user, node(&format!("batch race source {index}")))
                .await
                .unwrap();
            let context = selected_context(&first, user, &[&source]).await;
            let batch = MemoryBatch {
                updates: vec![batch_update(&source, 1, "updated while forget races")],
                creates: vec![node("derived while forget races")],
            };
            let (applied, forgotten) = tokio::join!(
                first.apply_batch_guarded(&context, batch),
                second.forget_many_guarded(&context, &context.source_memories)
            );
            assert_ne!(
                applied.is_ok(),
                forgotten.is_ok(),
                "one original snapshot must win"
            );
            if let Ok(result) = applied {
                let fresh = selected_context(&second, user, &[&source]).await;
                second
                    .forget_many_guarded(&fresh, &fresh.source_memories)
                    .await
                    .unwrap();
                assert!(
                    first
                        .get(user, &result.created_ids[0])
                        .await
                        .unwrap()
                        .is_none()
                );
            }
            assert!(
                first
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
        database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user','input received before deletion',$4,$5)",vec![source.into(),session.into(),user.to_string().into(),before.source_received_at.into(),before.privacy_epoch.into()])).await.unwrap();
        let mut restarted=memory.write_snapshot(user).await.unwrap();restarted.source_event_ids.push(source);
        assert!(memory.store_guarded(&restarted,node("new job inspecting old late-persisted message")).await.is_err());
        let fresh=Uuid::new_v4();
        database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user','genuinely new input',clock_timestamp(),$4)",vec![fresh.into(),session.into(),user.to_string().into(),restarted.privacy_epoch.into()])).await.unwrap();
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
async fn source_epoch_is_authoritative_with_fast_slow_and_unsourced_messages() {
    with_database(|database| async move {
        use alia_server::modules::memory::source::capture_source_epoch;
        let memory = database.memory(None).await;
        let user = Uuid::new_v4();
        database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Source','test','user','active')",
            vec![user.into(),format!("{user}@source.test").into()],
        )).await.unwrap();
        let epoch = capture_source_epoch(&database.db, user).await.unwrap();
        let session = Uuid::new_v4();
        database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'source-test',$2)",
            vec![session.into(),user.to_string().into()],
        )).await.unwrap();
        for offset in [-300, 300] {
            let source = Uuid::new_v4();
            let received = chrono::DateTime::from_timestamp_micros(chrono::Utc::now().timestamp_micros()).unwrap()
                + chrono::Duration::seconds(offset) + chrono::Duration::nanoseconds(400);
            database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user','trusted input',$4,$5)",
                vec![source.into(),session.into(),user.to_string().into(),received.into(),epoch.into()],
            )).await.unwrap();
            let mut context = memory.write_snapshot(user).await.unwrap();
            context.source_received_at = received;
            context.source_event_ids.push(source);
            context.exact_source_received_at = Some((source, received));
            memory.validate_write_context(&context).await.unwrap();
            memory.store_guarded(&context, node(&format!("clock case {offset}"))).await.unwrap();
            let mut wrong = context.clone();
            wrong.exact_source_received_at = Some((source, received + chrono::Duration::seconds(1)));
            assert!(memory.validate_write_context(&wrong).await.is_err());
            let mut precision = context.clone();
            precision.exact_source_received_at = Some((source, received + chrono::Duration::nanoseconds(1)));
            memory.validate_write_context(&precision).await.unwrap();
            let mut foreign = memory.write_snapshot(Uuid::new_v4()).await.unwrap();
            foreign.source_event_ids.push(source);
            assert!(memory.validate_write_context(&foreign).await.is_err());
        }
        let unsourced = Uuid::new_v4();
        database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at) VALUES($1,$2,$3,'user','no trusted epoch',clock_timestamp())",
            vec![unsourced.into(),session.into(),user.to_string().into()],
        )).await.unwrap();
        let mut context = memory.write_snapshot(user).await.unwrap();
        context.source_event_ids.push(unsourced);
        assert!(memory.validate_write_context(&context).await.is_err());
        assert!(memory.store_guarded(&context, node("null epoch cannot become current")).await.is_err());
        memory.clear(user).await.unwrap();
        assert!(memory.validate_write_context(&context).await.is_err());
        assert_eq!(capture_source_epoch(&database.db, user).await.unwrap(), epoch + 1);
    }).await;
}

#[tokio::test]
async fn source_admission_is_bounded_by_the_privacy_lock_and_current_account() {
    with_database(|database| async move {
        use alia_server::modules::memory::source::capture_source_epoch;
        use sea_orm::TransactionTrait;
        let user = Uuid::new_v4();
        assert!(capture_source_epoch(&database.db, user).await.is_err());
        database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Source','test','user','active')",
            vec![user.into(),format!("{user}@source.test").into()],
        )).await.unwrap();
        capture_source_epoch(&database.db, user).await.unwrap();
        let locked = database.db.begin().await.unwrap();
        locked.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT privacy_epoch FROM memory_privacy_state WHERE user_id=$1 FOR UPDATE", [user.into()],
        )).await.unwrap();
        let start = std::time::Instant::now();
        let error = capture_source_epoch(&database.db, user).await.unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(3));
        locked.rollback().await.unwrap();
        capture_source_epoch(&database.db, user).await.unwrap();
        database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE users SET status='disabled' WHERE id=$1", [user.into()],
        )).await.unwrap();
        assert!(capture_source_epoch(&database.db, user).await.unwrap_err().to_string().contains("currently active account"));
    }).await;
}

#[tokio::test]
async fn ram_epoch_rejects_old_sources_without_wall_clock_comparisons() {
    let memory = AliaMemory::new();
    let user = Uuid::new_v4();
    let mut original = memory.write_snapshot(user).await.unwrap();
    original.source_received_at += chrono::Duration::days(1);
    memory
        .store_guarded(&original, node("initial source"))
        .await
        .unwrap();
    memory.clear(user).await.unwrap();
    assert!(memory.validate_write_context(&original).await.is_err());
    let mut fresh = memory.write_snapshot(user).await.unwrap();
    fresh.source_received_at -= chrono::Duration::days(1);
    memory
        .store_guarded(&fresh, node("new source with a slow clock"))
        .await
        .unwrap();
    fresh.exact_source_received_at = Some((Uuid::new_v4(), fresh.source_received_at));
    assert!(memory.validate_write_context(&fresh).await.is_err());
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
            "rejected"
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
