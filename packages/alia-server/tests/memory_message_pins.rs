//! Canonical Memory retains the source facts it actually admitted.
use alia_server::{
    config::{ConversationSection, MemorySection},
    module::lifecycle::ModuleLifecycle,
    modules::{
        memory::{context::*, source::capture_source_epochs},
        platform::migrations,
    },
    providers::{
        adapter::{MemoryBackend, MemoryNode, MemoryPatch, RetrievalOpts},
        embedding::NoopEmbedding,
        memory::AliaMemory,
    },
};
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::{Value, json};
use std::{future::Future, panic::AssertUnwindSafe, sync::Arc, time::Duration};
use uuid::Uuid;

fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}

#[derive(Clone)]
struct Fixture {
    db: DatabaseConnection,
    admin: DatabaseConnection,
    schema: String,
    owner: Uuid,
    session: Uuid,
    memory: Arc<AliaMemory>,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("source pin tests require the disposable real PostgreSQL database");
        let mut options = ConnectOptions::new(url.clone());
        options.max_connections(1).sqlx_logging(false);
        let admin = Database::connect(options).await.unwrap();
        let schema = format!("memory_message_pin_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(6)
            .sqlx_logging(false)
            .set_schema_search_path(format!("{schema},public"));
        let db = match Database::connect(options).await {
            Ok(db) => db,
            Err(error) => {
                admin
                    .execute_unprepared(&format!("DROP SCHEMA {schema} CASCADE"))
                    .await
                    .unwrap();
                admin.close().await.unwrap();
                panic!("source pin schema connection: {error}");
            }
        };
        let fixture = Self {
            db,
            admin,
            schema,
            owner: Uuid::new_v4(),
            session: Uuid::new_v4(),
            memory: Arc::new(AliaMemory::new()),
        };
        let setup = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(45), async {
            migrations::run(&fixture.db).await.unwrap();
            fixture.db.execute(sql(
                "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Memory pin test','not-a-login','user','active')",
                vec![fixture.owner.into(), format!("{}@memory-pin.test",fixture.owner).into()],
            )).await.unwrap();
            fixture.db.execute(sql(
                "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'memory-pin-device',$2)",
                vec![fixture.session.into(), fixture.owner.to_string().into()],
            )).await.unwrap();
            let config = MemorySection { consolidation_enabled: true, ..Default::default() };
            fixture.service().initialize_policy(
                ContextPolicy::from_config(&config,&ConversationSection::default()).unwrap(),
            ).await.unwrap();
            fixture.memory.attach_runtime_for_test(fixture.db.clone(),None,config).await.unwrap();
        })).catch_unwind().await;
        match setup {
            Ok(Ok(())) => fixture,
            result => {
                fixture.cleanup().await;
                match result {
                    Err(error) => std::panic::resume_unwind(error),
                    _ => panic!("source pin fixture setup exceeded 45 seconds"),
                }
            }
        }
    }
    fn service(&self) -> ContextService {
        ContextService::new(self.db.clone(), Arc::new(NoopEmbedding))
    }
    async fn source(&self, text: &str) -> ContextSource {
        let epochs = capture_source_epochs(&self.db, self.owner).await.unwrap();
        let id = Uuid::new_v4();
        let row=self.db.query_one(sql(
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,created_at,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,clock_timestamp(),clock_timestamp(),$5,$6) RETURNING source_received_at",
            vec![id.into(),self.session.into(),self.owner.to_string().into(),text.into(),epochs.privacy_epoch.into(),epochs.relationship_epoch.into()],
        )).await.unwrap().unwrap();
        ContextSource {
            owner_id: self.owner,
            request_id: Uuid::new_v4(),
            channel_id: "memory-pin-device".into(),
            conversation_id: Some(self.session),
            source_event_id: Some(id),
            source_received_at: row.try_get("", "source_received_at").unwrap(),
            source_privacy_epoch: epochs.privacy_epoch,
        }
    }
    async fn generation(&self, text: &str) -> GenerationContext {
        self.service()
            .materialize(self.source(text).await, text)
            .await
            .unwrap()
    }
    async fn store(&self, source: Uuid, content: &str, merge: bool) -> String {
        let mut write = self.memory.write_snapshot(self.owner).await.unwrap();
        write.source_event_ids.push(source);
        let mut memory = MemoryNode::new("preference", "user", content, 0.9).with_runtime_fields(
            "preference",
            "user",
            0.9,
            0.9,
            "internal",
        );
        if !merge {
            memory.id = Some(Uuid::new_v4().to_string());
        }
        self.memory.store_guarded(&write, memory).await.unwrap()
    }
    async fn edit_source(&self, source: Uuid) {
        assert_eq!(self.db.execute(sql(
            "UPDATE chat_messages SET content='a corrected source that retracts the original preference' WHERE id=$1 AND user_id=$2",
            vec![source.into(),self.owner.to_string().into()],
        )).await.unwrap().rows_affected(),1);
    }
    async fn includes(&self, id: &str, query: &str) -> (bool, bool) {
        let generation = self.generation(query).await;
        let materialized = generation.lease().dependencies().iter().any(
            |root| matches!(root,ContextEvidence::Memory{id:memory,..} if memory.to_string()==id),
        );
        let standalone = self
            .memory
            .retrieve(self.owner, query, RetrievalOpts::default())
            .await
            .unwrap()
            .iter()
            .any(|memory| memory.id.as_deref() == Some(id));
        (materialized, standalone)
    }
    async fn pins(&self, id: &str) -> Value {
        self.db.query_one(sql(
            "SELECT COALESCE(jsonb_agg(jsonb_build_object('id',source_event_id,'epoch',source_privacy_epoch,'digest',content_digest) ORDER BY source_event_id),'[]'::jsonb) AS pins FROM memory_message_sources WHERE memory_id=$1::uuid AND owner_id=$2",
            vec![id.into(),self.owner.into()],
        )).await.unwrap().unwrap().try_get("","pins").unwrap()
    }
    async fn cleanup(self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            self.memory.stop().await.unwrap();
            self.db.close().await.unwrap();
            self.admin
                .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .await
                .unwrap();
            self.admin.close().await.unwrap();
        })
        .await
        .expect("source pin cleanup exceeded 10 seconds");
    }
}
async fn with_fixture<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(30),
        test(f.clone()),
    ))
    .catch_unwind()
    .await;
    f.cleanup().await;
    match result {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("source pin test exceeded 30 seconds"),
        Err(error) => std::panic::resume_unwind(error),
    }
}

#[tokio::test]
async fn edited_message_cannot_reauthorize_memory_in_new_context_retrieval_or_patch() {
    with_fixture(|f| async move {
        let text = "I like green tea";
        let source = f.source(text).await;
        let source_id = source.source_event_id.unwrap();
        let id = f.store(source_id, "I like green tea", false).await;
        assert_eq!(f.includes(&id, "green tea").await, (true, true));
        f.edit_source(source_id).await;
        let (materialized, standalone) = f.includes(&id, "green tea").await;
        let patch = f
            .memory
            .update(
                f.owner,
                &id,
                MemoryPatch {
                    expected_revision: Some(1),
                    content: Some("I still like green tea".into()),
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(
            (materialized, standalone, patch.is_ok()),
            (false, false, false),
            "a new caller and a later patch must not re-sign the edited source"
        );
        assert_eq!(
            f.memory.get(f.owner, &id).await.unwrap().unwrap().revision,
            1
        );
        assert_eq!(
            f.pins(&id).await,
            json!([{
                "id":source_id,"epoch":source.source_privacy_epoch,
                "digest":evidence_digest(&json!(text)).unwrap()
            }])
        );
    })
    .await;
}

#[tokio::test]
async fn incomplete_pins_fail_closed_while_relationship_clear_preserves_independent_memory() {
    with_fixture(|f| async move {
        let source = f.source("I like green tea").await.source_event_id.unwrap();
        let unpinned = f.store(source, "I like green tea", false).await;
        // Simulate a pre-migration Memory whose historical input bytes cannot
        // be proven. Reads and updates must not silently fill the missing pin.
        f.db.execute(sql(
            "DELETE FROM memory_message_sources WHERE memory_id=$1::uuid",
            vec![unpinned.clone().into()],
        ))
        .await
        .unwrap();
        assert_eq!(f.includes(&unpinned, "green tea").await, (false, false));
        assert!(
            f.memory
                .update(
                    f.owner,
                    &unpinned,
                    MemoryPatch {
                        expected_revision: Some(1),
                        content: Some("new green tea text".into()),
                        ..Default::default()
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(f.pins(&unpinned).await, json!([]));
        let source = f
            .source("I like walking in parks")
            .await
            .source_event_id
            .unwrap();
        let kept = f.store(source, "I like walking in parks", false).await;
        let pinned = f.pins(&kept).await;
        let mut plain = MemoryNode::new("fact", "user", "ordinary manual memory", 0.9);
        plain.id = Some(Uuid::new_v4().to_string());
        let plain = f.memory.store(f.owner, plain).await.unwrap();
        assert_eq!(f.pins(&plain).await, json!([]));
        let profile = f.service().relationship_profile(f.owner).await.unwrap();
        let cleared = f
            .service()
            .clear_relationship(f.owner, Uuid::new_v4(), profile.relationship_epoch)
            .await
            .unwrap();
        assert_eq!(cleared.removed_memories, 0);
        assert_eq!(f.pins(&kept).await, pinned);
        assert_eq!(
            f.memory.get(f.owner, &kept).await.unwrap().unwrap().status,
            "active"
        );
        assert_eq!(
            f.memory.get(f.owner, &plain).await.unwrap().unwrap().status,
            "active"
        );
        assert_eq!(f.includes(&kept, "walking parks").await, (true, true));
        f.memory.delete(f.owner, &kept).await.unwrap();
        assert_eq!(
            f.pins(&kept).await,
            json!([]),
            "soft deletion erases source pins"
        );
    })
    .await;
}

#[tokio::test]
async fn consolidation_retains_both_message_pins_and_rejects_either_source_revision() {
    for changed in [0, 1] {
        with_fixture(move |f| async move {
            let first = f.source("I like green tea").await.source_event_id.unwrap();
            let second = f
                .source("Green tea is still my preference")
                .await
                .source_event_id
                .unwrap();
            let id = f.store(first, "I like green tea", true).await;
            assert_eq!(
                f.store(second, "I like green tea", true).await,
                id,
                "the fixture must exercise actual reinforcement of an existing Memory"
            );
            let memory = f.memory.get(f.owner, &id).await.unwrap().unwrap();
            let mut sources = memory.source_event_ids;
            sources.sort();
            let mut expected = vec![first, second];
            expected.sort();
            assert_eq!(sources, expected);
            let pins = f.pins(&id).await;
            assert_eq!(pins.as_array().unwrap().len(), 2);
            assert_eq!(f.includes(&id, "green tea").await, (true, true));
            f.edit_source([first, second][changed]).await;
            assert_eq!(f.includes(&id, "green tea").await, (false, false));
            assert_eq!(f.pins(&id).await, pins);
        })
        .await;
    }
}
