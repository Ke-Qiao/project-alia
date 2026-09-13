//! Real PostgreSQL context authority, snapshot and revocation contracts.
//! Missing infrastructure fails; no in-memory lease substitute is permitted.
use alia_server::{
    config::{ConversationSection, MemorySection},
    modules::{memory::context::*, platform::migrations},
    providers::{
        adapter::{MemoryBackend, MemoryWriteOrigin},
        embedding::NoopEmbedding,
        memory::AliaMemory,
    },
};
use alia_server::{
    module::ModuleLifecycle,
    providers::adapter::{
        EmbeddingAdapter, EmbeddingProfile, EmbeddingVector, ProfiledEmbeddings,
        embedding_text_digest,
    },
};
use async_trait::async_trait;
use chrono::Utc;
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
    TransactionTrait, Value as SqlValue,
};
use serde_json::json;
use std::{future::Future, panic::AssertUnwindSafe, sync::Arc, time::Duration};
use uuid::Uuid;

fn stmt(sql: &str, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}
#[derive(Clone)]
struct Fixture {
    db: DatabaseConnection,
    admin: DatabaseConnection,
    schema: String,
    owner: Uuid,
    session: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("set ALIA_TEST_DATABASE_URL to disposable PostgreSQL");
        let mut options = ConnectOptions::new(url.clone());
        options.max_connections(1).sqlx_logging(false);
        let admin = Database::connect(options)
            .await
            .expect("test PostgreSQL must be reachable");
        let schema = format!("context_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(5)
            .sqlx_logging(false)
            .set_schema_search_path(format!("{schema},public"));
        let db = Database::connect(options).await.unwrap();
        let f = Self {
            db,
            admin,
            schema,
            owner: Uuid::new_v4(),
            session: Uuid::new_v4(),
        };
        if let Err(error) = migrations::run(&f.db).await {
            f.cleanup().await;
            panic!("migrations: {error:#}");
        }
        f.db.execute(stmt("INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Context test','not-a-login','user','active')",vec![f.owner.into(),format!("{}@context.test",f.owner).into()])).await.unwrap();
        f.db.execute(stmt(
            "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'context-device',$2)",
            vec![f.session.into(), f.owner.to_string().into()],
        ))
        .await
        .unwrap();
        let mut policy =
            ContextPolicy::from_config(&MemorySection::default(), &ConversationSection::default())
                .unwrap();
        policy.retrieval_layers = vec!["core".into()];
        f.service().initialize_policy(policy).await.unwrap();
        alia_server::modules::memory::source::capture_source_epoch(&f.db, f.owner)
            .await
            .unwrap();
        f
    }
    fn service(&self) -> ContextService {
        ContextService::new(self.db.clone(), Arc::new(NoopEmbedding))
    }
    async fn memory_backend(&self) -> AliaMemory {
        let memory = AliaMemory::new();
        memory
            .attach_runtime_for_test(self.db.clone(), None, MemorySection::default())
            .await
            .unwrap();
        memory
    }
    async fn source(&self, text: &str) -> ContextSource {
        let epoch =
            alia_server::modules::memory::source::capture_source_epoch(&self.db, self.owner)
                .await
                .unwrap();
        let id = Uuid::new_v4();
        let row=self.db.query_one(stmt("INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch,created_at) VALUES($1,$2,$3,'user',$4,clock_timestamp(),$5,clock_timestamp()) RETURNING source_received_at",vec![id.into(),self.session.into(),self.owner.to_string().into(),text.into(),epoch.into()])).await.unwrap().unwrap();
        ContextSource {
            owner_id: self.owner,
            request_id: Uuid::new_v4(),
            channel_id: "context-device".into(),
            conversation_id: Some(self.session),
            source_event_id: Some(id),
            source_received_at: row.try_get("", "source_received_at").unwrap(),
            source_privacy_epoch: epoch,
        }
    }
    async fn item(&self, text: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.db.execute(stmt("INSERT INTO memory_items(id,tenant_id,user_id,kind,scope,memory_layer,node_type,content) VALUES($1,$2,$3,'semantic','user','core','fact',$4)",vec![id.into(),Uuid::nil().into(),self.owner.into(),text.into()])).await.unwrap();
        id
    }
    async fn graph(&self, evidence: Uuid) -> (Uuid, Uuid) {
        let src = Uuid::new_v4();
        let dst = Uuid::new_v4();
        let edge = Uuid::new_v4();
        for (id, label, properties) in [
            (src, "Alice", json!({})),
            (dst, "tea", json!({"memory_id":evidence})),
        ] {
            self.db.execute(stmt("INSERT INTO memory_nodes(id,tenant_id,user_id,node_type,node_key,label,properties) VALUES($1,$2,$3,'entity',$4,$5,$6)",vec![id.into(),Uuid::nil().into(),self.owner.into(),id.to_string().into(),label.into(),properties.into()])).await.unwrap();
        }
        self.db.execute(stmt("INSERT INTO memory_edges(id,tenant_id,user_id,src_node_id,dst_node_id,relation,evidence_memory_id,source_revision) VALUES($1,$2,$3,$4,$5,'PREFERS',$6,1)",vec![edge.into(),Uuid::nil().into(),self.owner.into(),src.into(),dst.into(),evidence.into()])).await.unwrap();
        (edge, src)
    }
    async fn committed_history(&self, context: &GenerationContext, text: &str) -> Uuid {
        let id = Uuid::new_v4();
        let response = Uuid::new_v4();
        let s = context.source();
        self.db.execute(stmt("INSERT INTO chat_messages(id,session_id,role,content,created_at) VALUES($1,$2,'assistant',$3,clock_timestamp())",vec![id.into(),self.session.into(),text.into()])).await.unwrap();
        self.db.execute(stmt("INSERT INTO delivery_responses(response_id,user_id,request_id,output_key,conversation_id,assistant_message_id,source_event_id,source_received_at,source_privacy_epoch,channel_id,kind,content,final_content,intent_digest,status,context_id) VALUES($1,$2,$3,'answer',$4,$5,$6,$7,$8,$9,'conversation','{}',$10,$11,'committed',$12)",vec![response.into(),s.owner_id.into(),s.request_id.into(),self.session.into(),id.into(),s.source_event_id.into(),s.source_received_at.into(),s.source_privacy_epoch.into(),s.channel_id.clone().into(),json!({"text":text}).into(),"0".repeat(64).into(),context.lease().id().into()])).await.unwrap();
        response
    }
    async fn count(&self, table: &str) -> i64 {
        assert!(table.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        self.db
            .query_one(stmt(&format!("SELECT COUNT(*) AS n FROM {table}"), vec![]))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }
    async fn sql(&self, sql: &str) {
        self.db.execute_unprepared(sql).await.unwrap();
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
async fn with_db<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(45),
        test(f.clone()),
    ))
    .catch_unwind()
    .await;
    f.cleanup().await;
    match result {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("context test exceeded 45s"),
        Err(e) => std::panic::resume_unwind(e),
    }
}

#[tokio::test]
async fn materializes_actual_trimmed_inputs_and_preserves_original_identity() {
    with_db(|f| async move {
        let service = f.service();
        let old = f.source("previous user fact").await;
        let memory = f.item(&format!("tea {}", "叶".repeat(400))).await;
        let hidden = f.item("tea hidden").await;
        f.sql(&format!(
            "UPDATE memory_items SET sensitive=TRUE WHERE id='{hidden}'"
        ))
        .await;
        let mut policy = service.policy_store().current().await.unwrap();
        policy.policy.max_memory_chars = 256;
        service
            .policy_store()
            .replace(policy.version, policy.policy)
            .await
            .unwrap();
        let source = f.source("tea").await;
        let context = service.materialize(source.clone(), "tea").await.unwrap();
        assert_eq!(context.source(), &source);
        assert!(context.memory_prompt().chars().count() <= 256);
        assert!(context.memory_prompt().contains("tea"));
        assert!(!context.memory_prompt().contains("hidden"));
        let roots = context.lease().dependencies();
        assert!(
            roots
                .iter()
                .any(|r| matches!(r,ContextEvidence::Memory{id,..} if *id==memory))
        );
        assert!(
            !roots
                .iter()
                .any(|r| matches!(r,ContextEvidence::Memory{id,..} if *id==hidden))
        );
        assert_eq!(
            context.history().iter().map(|m| m.id()).collect::<Vec<_>>(),
            vec![old.source_event_id.unwrap()]
        );
        assert!(context.memory().project_context[0].content.chars().count() <= 240);
        let repeated = service.materialize(source, "tea").await.unwrap();
        assert_eq!(context.lease().id(), repeated.lease().id());
        assert_eq!(f.count("context_leases").await, 1);
        service
            .validate(&context.lease().binding(), ContextUse::ResponseCommit)
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
async fn graph_projection_and_memory_roots_are_revalidated_before_use() {
    with_db(|f| async move {
        let service = f.service();
        let memory = f.item("tea evidence").await;
        let (edge, node) = f.graph(memory).await;
        let context = service
            .materialize(f.source("tea").await, "tea")
            .await
            .unwrap();
        assert!(context.memory_prompt().contains("Alice PREFERS tea"));
        assert!(
            context
                .lease()
                .dependencies()
                .iter()
                .any(|r| matches!(r,ContextEvidence::Graph{edge_id,..} if *edge_id==edge))
        );
        service
            .validate(&context.lease().binding(), ContextUse::OutputAdmission)
            .await
            .unwrap();
        f.sql(&format!(
            "UPDATE memory_nodes SET label='Bob' WHERE id='{node}'"
        ))
        .await;
        assert!(
            service
                .validate(&context.lease().binding(), ContextUse::OutputAdmission)
                .await
                .is_err()
        );
        f.sql(&format!(
            "UPDATE memory_nodes SET label='Alice' WHERE id='{node}'"
        ))
        .await;
        service
            .validate(&context.lease().binding(), ContextUse::OutputAdmission)
            .await
            .unwrap();
        f.sql(&format!(
            "UPDATE memory_items SET revision=revision+1,content='coffee' WHERE id='{memory}'"
        ))
        .await;
        assert!(
            service
                .validate(&context.lease().binding(), ContextUse::ResponseCommit)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn clear_retains_display_history_but_revokes_generation_and_capture_sources() {
    with_db(|f| async move {
        let service = f.service();
        let source = f.source("old private material").await;
        let context = service
            .materialize(source.clone(), "old private material")
            .await
            .unwrap();
        f.committed_history(&context, "old answer").await;
        let memory = f.memory_backend().await;
        memory.clear(f.owner).await.unwrap();
        assert!(
            service
                .validate(&context.lease().binding(), ContextUse::ResponseCommit)
                .await
                .is_err()
        );
        assert!(
            service
                .extend_memory(&context, "another query")
                .await
                .is_err()
        );
        let new_source = f.source("new turn").await;
        assert_eq!(
            new_source.source_privacy_epoch,
            source.source_privacy_epoch + 1
        );
        let new_context = service.materialize(new_source, "new turn").await.unwrap();
        assert!(new_context.history().is_empty());
        assert_eq!(f.count("chat_messages").await, 3);
        let snapshot = memory.write_snapshot(f.owner).await.unwrap();
        assert!(
            context
                .lease()
                .memory_write_context(snapshot, MemoryWriteOrigin::Automatic)
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn historical_assistant_contributes_only_with_complete_live_roots() {
    with_db(|f| async move {
        let service = f.service();
        let memory = f.item("tea").await;
        let first = service
            .materialize(f.source("tea").await, "tea")
            .await
            .unwrap();
        f.committed_history(&first, "derived tea answer").await;
        let second = service
            .materialize(f.source("follow up").await, "follow up")
            .await
            .unwrap();
        assert!(
            second
                .history()
                .iter()
                .any(|m| m.message().content == "derived tea answer")
        );
        assert!(
            second
                .lease()
                .dependencies()
                .iter()
                .any(|r| matches!(r,ContextEvidence::Memory{id,..} if *id==memory))
        );
        f.sql(&format!(
            "UPDATE memory_items SET revision=revision+1,content='changed' WHERE id='{memory}'"
        ))
        .await;
        assert!(
            service
                .validate(&second.lease().binding(), ContextUse::OutputAdmission)
                .await
                .is_err()
        );
        let third = service
            .materialize(f.source("new follow up").await, "new follow up")
            .await
            .unwrap();
        assert!(
            !third
                .history()
                .iter()
                .any(|m| m.message().content == "derived tea answer")
        );
    })
    .await;
}

#[tokio::test]
async fn policy_cas_is_persistent_and_pause_does_not_disable_manual_forget() {
    with_db(|f| async move {
        let service = f.service();
        let store = service.policy_store();
        let context = service
            .materialize(f.source("hello").await, "hello")
            .await
            .unwrap();
        let old = store.current().await.unwrap();
        let mut next = old.policy.clone();
        next.paused_until = Some(Utc::now() + chrono::Duration::hours(1));
        assert!(matches!(
            store
                .replace_for_actor(old.version, next.clone(), f.owner)
                .await
                .unwrap_err()
                .downcast_ref::<ContextError>(),
            Some(ContextError::Forbidden)
        ));
        f.sql(&format!(
            "UPDATE users SET role='admin' WHERE id='{}'",
            f.owner
        ))
        .await;
        let changed = store
            .replace_for_actor(old.version, next.clone(), f.owner)
            .await
            .unwrap();
        assert_eq!(changed.version, old.version + 1);
        assert!(matches!(
            store
                .replace(old.version, next)
                .await
                .unwrap_err()
                .downcast_ref::<ContextError>(),
            Some(ContextError::PolicyChanged)
        ));
        assert!(matches!(
            service
                .validate(&context.lease().binding(), ContextUse::OutputAdmission)
                .await
                .unwrap_err()
                .downcast_ref::<ContextError>(),
            Some(ContextError::PolicyChanged)
        ));
        let paused = service
            .materialize(f.source("paused request").await, "paused request")
            .await
            .unwrap();
        assert!(paused.memory_prompt().is_empty());
        assert!(matches!(
            service
                .validate(&paused.lease().binding(), ContextUse::AutomaticMemoryWrite)
                .await
                .unwrap_err()
                .downcast_ref::<ContextError>(),
            Some(ContextError::Revoked(_))
        ));
        service
            .validate(&paused.lease().binding(), ContextUse::ManualMemoryWrite)
            .await
            .unwrap();
        let loaded = service.initialize_policy(old.policy).await.unwrap();
        assert_eq!(loaded.version, changed.version);
        assert!(loaded.policy.paused_until.is_some());
        f.memory_backend().await.clear(f.owner).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn authority_cannot_be_refreshed_or_rebound_and_notice_is_fixed() {
    with_db(|f| async move {
        let service = f.service();
        let source = f.source("raw speech").await;
        assert!(
            service
                .materialize(source.clone(), "model corrected speech")
                .await
                .is_err()
        );
        assert_eq!(f.count("context_leases").await, 0);
        let context = service
            .materialize(source.clone(), "raw speech")
            .await
            .unwrap();
        let mut wrong = context.lease().binding();
        wrong.request_id = Uuid::new_v4();
        assert!(
            service
                .validate(&wrong, ContextUse::OutputAdmission)
                .await
                .is_err()
        );
        wrong = context.lease().binding();
        wrong.owner_id = Uuid::new_v4();
        assert!(
            service
                .validate(&wrong, ContextUse::OutputAdmission)
                .await
                .is_err()
        );
        let notice = service
            .source_notice_for(source, SourceNotice::ProcessingFailedEn)
            .await
            .unwrap();
        assert_eq!(
            notice.notice_text(),
            Some(SourceNotice::ProcessingFailedEn.text())
        );
        service
            .validate(&notice.binding(), ContextUse::OutputAdmission)
            .await
            .unwrap();
        for purpose in [
            ContextUse::ResponseCommit,
            ContextUse::CacheWrite,
            ContextUse::AutomaticMemoryWrite,
            ContextUse::Execution,
        ] {
            assert!(service.validate(&notice.binding(), purpose).await.is_err());
        }
        assert!(
            f.db.execute(stmt(
                "UPDATE context_leases SET policy_version=policy_version+1 WHERE context_id=$1",
                vec![context.lease().id().into()]
            ))
            .await
            .is_err()
        );
        f.sql(&format!(
            "UPDATE users SET status='disabled' WHERE id='{}'",
            f.owner
        ))
        .await;
        assert!(
            service
                .validate(&context.lease().binding(), ContextUse::OutputAdmission)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn memory_binding_copies_graph_and_historical_user_roots() {
    with_db(|f| async move {
        let service = f.service();
        let first = f.source("earlier input").await;
        let memory = f.item("tea").await;
        f.graph(memory).await;
        let source = f.source("tea").await;
        let context = service.materialize(source.clone(), "tea").await.unwrap();
        let backend = f.memory_backend().await;
        let bound = backend
            .bind_context(
                backend.write_snapshot(f.owner).await.unwrap(),
                context.lease().id(),
                MemoryWriteOrigin::Ai,
            )
            .await
            .unwrap();
        assert_eq!(bound.context_id, Some(context.lease().id()));
        assert_eq!(bound.policy_version, Some(context.lease().policy_version()));
        assert_eq!(bound.source_memories, vec![(memory.to_string(), 1)]);
        assert!(
            bound
                .source_event_ids
                .contains(&first.source_event_id.unwrap())
        );
        assert!(
            bound
                .source_event_ids
                .contains(&source.source_event_id.unwrap())
        );
        assert_eq!(bound.source_received_at, source.source_received_at);
        let fixture = AliaMemory::new();
        assert!(
            fixture
                .bind_context(bound, context.lease().id(), MemoryWriteOrigin::Ai)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn validated_transaction_serializes_clear_until_effect_commit() {
    with_db(|f|async move {
        let service=f.service();
        let context=service.materialize(f.source("turn").await,"turn").await.unwrap();
        let memory=f.memory_backend().await;
        let tx=f.db.begin().await.unwrap();
        ContextService::validate_in_transaction(&tx,&context.lease().binding(),ContextUse::ResponseCommit).await.unwrap();
        let owner=f.owner;
        let mut clear=tokio::spawn(async move{memory.clear(owner).await});
        assert!(tokio::time::timeout(Duration::from_millis(100),&mut clear).await.is_err());
        tx.execute(stmt("INSERT INTO chat_messages(id,session_id,role,content) VALUES($1,$2,'assistant','atomic output')",vec![Uuid::new_v4().into(),f.session.into()])).await.unwrap();
        tx.commit().await.unwrap();
        clear.await.unwrap().unwrap();
        assert!(service.validate(&context.lease().binding(),ContextUse::OutputAdmission).await.is_err());
        assert_eq!(f.count("chat_messages").await,2);
    }).await;
}

#[tokio::test]
async fn rollback_or_missing_canonical_storage_never_returns_empty_authority() {
    with_db(|f|async move {
        let service=f.service();let source=f.source("turn").await;
        f.sql("CREATE FUNCTION fail_context_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected lease write failure'; END $$").await;
        f.sql("CREATE TRIGGER fail_context BEFORE INSERT ON context_leases FOR EACH ROW EXECUTE FUNCTION fail_context_insert()").await;
        assert!(service.materialize(source.clone(),"turn").await.is_err());
        assert_eq!(f.count("context_leases").await,0);
        f.sql("DROP TRIGGER fail_context ON context_leases").await;
        f.sql("CREATE VIEW memory_context_items_backup AS SELECT * FROM memory_items").await;
        // Rename alone could fall back to public.memory_items. An explicit error
        // trigger on the canonical source cannot be masked by the search path.
        f.sql("ALTER TABLE memory_items RENAME TO memory_items_hidden").await;
        let sql=format!("CREATE VIEW {}.memory_items AS SELECT * FROM {}.memory_items_hidden WHERE (1 / (SELECT 0))=0",f.schema,f.schema);
        f.sql(&sql).await;
        assert!(service.materialize(source,"turn").await.is_err());
        assert_eq!(f.count("context_leases").await,0);
    }).await;
}

#[tokio::test]
async fn actual_history_is_bounded_and_unsourced_messages_stay_out() {
    with_db(|f|async move {
        let service=f.service();
        f.db.execute(stmt("INSERT INTO chat_messages(id,session_id,user_id,role,content) VALUES($1,$2,$3,'user','unsourced legacy')",vec![Uuid::new_v4().into(),f.session.into(),f.owner.to_string().into()])).await.unwrap();
        for index in 0..70 {f.source(&format!("history {index}")).await;}
        let mut policy=service.policy_store().current().await.unwrap();policy.policy.max_history_messages=4;
        service.policy_store().replace(policy.version,policy.policy).await.unwrap();
        let context=service.materialize(f.source("current").await,"current").await.unwrap();
        assert_eq!(context.history().len(),4);
        assert_eq!(context.history()[0].message().content,"history 66");
        assert_eq!(context.lease().dependencies().len(),5);
        let huge="x".repeat(MAX_CONTEXT_BYTES+1);let source=f.source(&huge).await;
        assert!(matches!(service.materialize(source,&huge).await.unwrap_err().downcast_ref::<ContextError>(),Some(ContextError::BoundsExceeded)));
    }).await;
}

#[tokio::test]
async fn execution_projection_is_canonical_and_status_change_revokes_it() {
    with_db(|f|async move {
        let service=f.service();let context=service.materialize(f.source("tool request").await,"tool request").await.unwrap();
        let s=context.source();let effect=Uuid::new_v4();
        f.db.execute(stmt("INSERT INTO execution_effects(effect_id,user_id,request_id,invocation_key,model_call_id,source_received_at,source_event_id,source_privacy_epoch,source,conversation_id,channel_id,tool_name,arguments,tool_digest,policy_digest,catalog_digest,intent_digest,requires_confirmation,status,context_id) VALUES($1,$2,$3,'call:0','model:0',$4,$5,$6,'test',$7,$8,'test_tool','{}',$9,$9,$9,$9,TRUE,'proposed',$10)",vec![effect.into(),s.owner_id.into(),s.request_id.into(),s.source_received_at.into(),s.source_event_id.into(),s.source_privacy_epoch.into(),s.conversation_id.into(),s.channel_id.clone().into(),"a".repeat(64).into(),context.lease().id().into()])).await.unwrap();
        let (extended,projection)=service.extend_execution(&context,effect).await.unwrap();
        assert_eq!(projection["effect_id"],json!(effect));assert_eq!(projection["status"],"proposed");assert_eq!(projection["tool_name"],"test_tool");assert!(projection["completion"].is_null());
        service.validate(&extended.lease().binding(),ContextUse::ResponseCommit).await.unwrap();
        f.sql(&format!("UPDATE execution_effects SET status='cancelled' WHERE effect_id='{effect}'")).await;
        assert!(service.validate(&extended.lease().binding(),ContextUse::OutputAdmission).await.is_err());
        let (_,cancelled)=service.extend_execution(&context,effect).await.unwrap();
        assert_eq!(cancelled["status"],"cancelled");
    }).await;
}

#[tokio::test]
async fn capture_materializes_canonical_rows_and_has_no_output_authority() {
    with_db(|f| async move {
        let service = f.service();
        let source = f.source("original capture message").await;
        let id = f.item("tea memory").await;
        let backend = f.memory_backend().await;
        let mut snapshot = backend.write_snapshot(f.owner).await.unwrap();
        snapshot
            .source_event_ids
            .push(source.source_event_id.unwrap());
        snapshot.source_memories.push((id.to_string(), 1));
        f.sql(&format!(
            "UPDATE chat_messages SET content='canonical current message' WHERE id='{}'",
            source.source_event_id.unwrap()
        ))
        .await;
        let captured = service
            .materialize_memory_capture(&snapshot, MemoryWriteOrigin::Automatic)
            .await
            .unwrap();
        assert_eq!(
            captured.messages()[0].message().content,
            "canonical current message"
        );
        assert_eq!(captured.memories()[0].content, "tea memory");
        assert_eq!(
            captured.lease().source().source_received_at,
            snapshot.source_received_at
        );
        assert_eq!(
            captured.lease().policy_version(),
            snapshot.policy_version.unwrap()
        );
        let bound = captured
            .lease()
            .memory_write_context(snapshot.clone(), MemoryWriteOrigin::Automatic)
            .unwrap();
        backend.validate_write_context(&bound).await.unwrap();
        for purpose in [
            ContextUse::OutputAdmission,
            ContextUse::ResponseCommit,
            ContextUse::CacheWrite,
            ContextUse::Execution,
            ContextUse::MemoryMaintenance,
            ContextUse::AiMemoryWrite,
        ] {
            assert!(
                service
                    .validate(&captured.lease().binding(), purpose)
                    .await
                    .is_err()
            );
        }
        assert!(
            captured
                .lease()
                .memory_write_context(snapshot.clone(), MemoryWriteOrigin::Maintenance)
                .is_err()
        );
        let maintenance = service
            .materialize_memory_capture(&snapshot, MemoryWriteOrigin::Maintenance)
            .await
            .unwrap();
        service
            .validate(
                &maintenance.lease().binding(),
                ContextUse::MemoryMaintenance,
            )
            .await
            .unwrap();
        assert!(
            service
                .validate(
                    &maintenance.lease().binding(),
                    ContextUse::AutomaticMemoryWrite
                )
                .await
                .is_err()
        );
        f.sql(&format!(
            "UPDATE memory_items SET revision=revision+1 WHERE id='{id}'"
        ))
        .await;
        assert!(
            service
                .materialize_memory_capture(&snapshot, MemoryWriteOrigin::Automatic)
                .await
                .is_err()
        );
        assert!(
            service
                .validate(
                    &captured.lease().binding(),
                    ContextUse::AutomaticMemoryWrite
                )
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn cache_merges_origin_roots_keeps_new_ingress_and_rechecks_row_version() {
    with_db(|f|async move {
        use alia_server::providers::adapter::embedding_text_digest;
        let service=f.service();let root=f.item("tea fact").await;
        let origin=service.materialize(f.source("tea").await,"tea").await.unwrap();
        let response=f.committed_history(&origin,"cached tea answer").await;
        let cache=Uuid::new_v4();
        f.db.execute(stmt("INSERT INTO llm_cache(id,user_id,scope,query_hash,query_text,response_text,metadata,revision,context_id,origin_response_id,status,content_digest,expires_at) VALUES($1,$2,'conversation',$3,'tea','cached tea answer','{}',1,$4,$5,'active',$6,clock_timestamp()+interval '1 hour')",vec![cache.into(),f.owner.into(),embedding_text_digest("tea").into(),origin.lease().id().into(),response.into(),evidence_digest(&json!({"response_text":"cached tea answer"})).unwrap().into()])).await.unwrap();
        let current=service.materialize(f.source("fresh query").await,"fresh query").await.unwrap();
        let merged=service.merge_cache(&current,CacheIdentity{cache_id:cache,revision:1,context_id:origin.lease().id(),scope:"conversation".into()}).await.unwrap();
        assert_eq!(merged.source(),current.source());
        assert_eq!(merged.lease().policy_version(),current.lease().policy_version());
        assert!(merged.lease().valid_until().is_some());
        assert!(merged.lease().dependencies().iter().any(|r|matches!(r,ContextEvidence::Memory{id,..} if *id==root)));
        service.validate(&merged.lease().binding(),ContextUse::OutputAdmission).await.unwrap();
        f.sql(&format!("UPDATE llm_cache SET revision=revision+1 WHERE id='{cache}'")).await;
        assert!(service.validate(&merged.lease().binding(),ContextUse::OutputAdmission).await.is_err());
        assert!(service.merge_cache(&current,CacheIdentity{cache_id:cache,revision:1,context_id:origin.lease().id(),scope:"conversation".into()}).await.is_err());
    }).await;
}

#[tokio::test]
async fn timed_resume_is_one_persistent_cas_and_does_not_resign_old_work() {
    with_db(|f| async move {
        let service = f.service();
        let backend = f.memory_backend().await;
        let old = service.policy_store().current().await.unwrap();
        let mut due = old.policy.clone();
        due.paused_until = Some(Utc::now() - chrono::Duration::seconds(1));
        let paused = service
            .policy_store()
            .replace(old.version, due)
            .await
            .unwrap();
        let tx = f.db.begin().await.unwrap();
        assert!(matches!(
            ContextService::policy_in_transaction(
                &tx,
                Some(paused.version),
                ContextUse::AutomaticMemoryWrite
            )
            .await
            .unwrap_err()
            .downcast_ref::<ContextError>(),
            Some(ContextError::PolicyChanged)
        ));
        ContextService::policy_in_transaction(
            &tx,
            Some(paused.version),
            ContextUse::ManualMemoryWrite,
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        backend.clear(f.owner).await.unwrap();
        let resumed = service.policy_store().current().await.unwrap();
        assert_eq!(resumed.version, paused.version + 1);
        assert!(resumed.policy.paused_until.is_none());
        assert_eq!(
            service.policy_store().current().await.unwrap().version,
            resumed.version
        );
    })
    .await;
}

struct GateEmbedding {
    profile: EmbeddingProfile,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}
#[async_trait]
impl ModuleLifecycle for GateEmbedding {
    fn name(&self) -> &'static str {
        "context_profile_fixture"
    }
}
#[async_trait]
impl EmbeddingAdapter for GateEmbedding {
    async fn embed(&self, _: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        anyhow::bail!("unprofiled embedding must not be used by Context")
    }
    async fn embed_profiled(&self, texts: &[String]) -> anyhow::Result<Option<ProfiledEmbeddings>> {
        self.entered.notify_one();
        self.release.acquire().await?.forget();
        Ok(Some(ProfiledEmbeddings {
            profile: self.profile.clone(),
            vectors: texts
                .iter()
                .map(|text| EmbeddingVector {
                    input_digest: embedding_text_digest(text),
                    values: vec![1.0; 1536],
                })
                .collect(),
        }))
    }
    fn dimension(&self) -> usize {
        1536
    }
}
async fn gated_service(f: &Fixture) -> (ContextService, Arc<GateEmbedding>) {
    let profile = EmbeddingProfile {
        provider: "fixture".into(),
        endpoint: "http://test.invalid".into(),
        model: "fixture-1536".into(),
        revision: "fixture.v1".into(),
        dimensions: 1536,
        distance: "cosine".into(),
        vector_type: "vector".into(),
    };
    let id = profile.id().unwrap();
    f.db.execute(stmt("INSERT INTO embedding_profiles(id,provider,model,dimensions,distance,vector_type,request_descriptor) VALUES($1,$2,$3,1536,'cosine','vector',$4)",vec![id.clone().into(),profile.provider.clone().into(),profile.model.clone().into(),json!(profile).into()])).await.unwrap();
    let mut policy = f.service().policy_store().current().await.unwrap();
    policy.policy.active_embedding_profile = id.clone();
    policy.policy.cache_embedding_profile_id = Some(id);
    f.service()
        .policy_store()
        .replace(policy.version, policy.policy)
        .await
        .unwrap();
    let adapter = Arc::new(GateEmbedding {
        profile,
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Semaphore::new(0),
    });
    (ContextService::new(f.db.clone(), adapter.clone()), adapter)
}

#[tokio::test]
async fn clear_during_embedding_cannot_refresh_original_epoch() {
    with_db(|f| async move {
        let (service, adapter) = gated_service(&f).await;
        let source = f.source("query").await;
        let task = tokio::spawn(async move { service.materialize(source, "query").await });
        tokio::time::timeout(Duration::from_secs(2), adapter.entered.notified())
            .await
            .unwrap();
        f.memory_backend().await.clear(f.owner).await.unwrap();
        adapter.release.add_permits(1);
        assert!(matches!(
            task.await
                .unwrap()
                .unwrap_err()
                .downcast_ref::<ContextError>(),
            Some(ContextError::Revoked(_))
        ));
        assert_eq!(f.count("context_leases").await, 0);
    })
    .await;
}

#[tokio::test]
async fn policy_change_during_embedding_cannot_refresh_original_policy() {
    with_db(|f| async move {
        let (service, adapter) = gated_service(&f).await;
        let source = f.source("query").await;
        let task = tokio::spawn(async move { service.materialize(source, "query").await });
        tokio::time::timeout(Duration::from_secs(2), adapter.entered.notified())
            .await
            .unwrap();
        let mut next = f.service().policy_store().current().await.unwrap();
        next.policy.memory_enabled = false;
        f.service()
            .policy_store()
            .replace(next.version, next.policy)
            .await
            .unwrap();
        adapter.release.add_permits(1);
        assert!(matches!(
            task.await
                .unwrap()
                .unwrap_err()
                .downcast_ref::<ContextError>(),
            Some(ContextError::PolicyChanged)
        ));
        assert_eq!(f.count("context_leases").await, 0);
    })
    .await;
}

#[tokio::test]
async fn policy_digest_survives_jsonb_float_roundtrip() {
    with_db(|f| async move {
        let service = f.service();
        let mut current = service.policy_store().current().await.unwrap();
        current.policy.cache_semantic_threshold = 0.92;
        current.policy.similarity_threshold = 0.37;
        let updated = service
            .policy_store()
            .replace(current.version, current.policy.clone())
            .await
            .unwrap();
        let loaded = service.policy_store().current().await.unwrap();
        assert_eq!(updated.version, loaded.version);
        assert_eq!(updated.policy, loaded.policy);
        let context = service
            .materialize(f.source("hello").await, "hello")
            .await
            .unwrap();
        service
            .validate(&context.lease().binding(), ContextUse::ResponseCommit)
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
async fn memory_extension_retains_all_previously_read_material_and_roots() {
    with_db(|f| async move {
        let service = f.service();
        let alpha = f.item("alpha fact").await;
        let beta = f.item("beta fact").await;
        let mut policy = service.policy_store().current().await.unwrap();
        policy.policy.max_memory_results = 1;
        policy.policy.similarity_threshold = 0.6;
        service
            .policy_store()
            .replace(policy.version, policy.policy)
            .await
            .unwrap();
        let first = service
            .materialize(f.source("alpha").await, "alpha")
            .await
            .unwrap();
        assert!(first.memory_prompt().contains("alpha fact"));
        assert!(!first.memory_prompt().contains("beta fact"));
        let second = service.extend_memory(&first, "beta").await.unwrap();
        assert!(second.memory_prompt().contains("beta fact"));
        assert!(!second.memory_prompt().contains("alpha fact"));
        let all = second.all_materialized_memory();
        assert_eq!(all.project_context.len(), 2);
        for id in [alpha, beta] {
            assert!(
                second
                    .lease()
                    .dependencies()
                    .iter()
                    .any(|r| matches!(r,ContextEvidence::Memory{id:root,..} if *root==id))
            );
        }
        f.sql(&format!(
            "UPDATE memory_items SET revision=revision+1 WHERE id='{alpha}'"
        ))
        .await;
        assert!(
            service
                .validate(&second.lease().binding(), ContextUse::OutputAdmission)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn postgres_memory_write_enforces_current_policy_without_ram_projection() {
    with_db(|f| async move {
        use alia_server::providers::adapter::{MemoryBatch, MemoryNode, MemoryPatch};
        let service = f.service();
        let backend = f.memory_backend().await;
        let mut current = service.policy_store().current().await.unwrap();
        current.policy.capture_min_confidence = 0.95;
        current.policy.capture_min_importance = 0.9;
        current.policy.sensitive_filter_enabled = true;
        service
            .policy_store()
            .replace(current.version, current.policy)
            .await
            .unwrap();
        // The backend remains attached to the older MemorySection projection.
        let context = service
            .materialize(f.source("remember this turn").await, "remember this turn")
            .await
            .unwrap();
        let snapshot = backend
            .bind_context(
                backend.write_snapshot(f.owner).await.unwrap(),
                context.lease().id(),
                MemoryWriteOrigin::Automatic,
            )
            .await
            .unwrap();
        let node = |text: &str, confidence: f32, importance: f32| {
            MemoryNode::new("core", "fact", text, importance)
                .with_runtime_fields("semantic", "user", confidence, 0.8, "internal")
        };
        assert!(
            backend
                .store_guarded(&snapshot, node("low confidence", 0.8, 0.99))
                .await
                .is_err()
        );
        assert_eq!(f.count("memory_items").await, 0);
        assert!(
            backend
                .store_guarded(&snapshot, node("my password is secret123", 0.99, 0.99))
                .await
                .is_err()
        );
        assert_eq!(f.count("memory_items").await, 0);
        let id = backend
            .store_guarded(&snapshot, node("safe fact", 0.99, 0.99))
            .await
            .unwrap();
        assert!(
            backend
                .update_guarded(
                    &snapshot,
                    &id,
                    MemoryPatch {
                        expected_revision: Some(1),
                        content: Some("my password is secret123".into()),
                        ..Default::default()
                    }
                )
                .await
                .is_err()
        );
        let unchanged = backend.get(f.owner, &id).await.unwrap().unwrap();
        assert_eq!(unchanged.revision, 1);
        assert_eq!(unchanged.content, "safe fact");
        assert!(
            backend
                .apply_batch_guarded(
                    &snapshot,
                    MemoryBatch {
                        updates: vec![],
                        creates: vec![
                            node("another safe fact", 0.99, 0.99),
                            node("my password is secret123", 0.99, 0.99)
                        ]
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(f.count("memory_items").await, 1);
    })
    .await;
}

#[tokio::test]
async fn graph_depth_and_consolidation_flag_are_real_persistent_policy() {
    with_db(|f|async move {
        let service=f.service();let alpha=f.item("alpha fact").await;let beta=f.item("beta fact").await;
        let (_,alice)=f.graph(alpha).await;let (edge,_) =f.graph(beta).await;
        f.sql(&format!("UPDATE memory_edges SET src_node_id='{alice}' WHERE id='{edge}'")).await;
        f.sql(&format!("UPDATE memory_nodes SET label='coffee' WHERE id=(SELECT dst_node_id FROM memory_edges WHERE id='{edge}')")).await;
        let mut policy=service.policy_store().current().await.unwrap();policy.policy.graph_depth=1;policy.policy.similarity_threshold=0.6;
        let policy=service.policy_store().replace(policy.version,policy.policy).await.unwrap();
        let first=service.materialize(f.source("alpha").await,"alpha").await.unwrap();assert_eq!(first.memory().graph_context.len(),1);
        let mut next=policy.policy;next.graph_depth=2;
        let next=service.policy_store().replace(policy.version,next).await.unwrap();
        let second=service.materialize(f.source("alpha").await,"alpha").await.unwrap();assert_eq!(second.memory().graph_context.len(),2);
        assert!(second.lease().dependencies().iter().any(|r|matches!(r,ContextEvidence::Memory{id,..} if *id==beta)));
        let backend=f.memory_backend().await;let mut snapshot=backend.write_snapshot(f.owner).await.unwrap();snapshot.source_memories=vec![(alpha.to_string(),1)];
        let capture=service.materialize_memory_capture(&snapshot,MemoryWriteOrigin::Maintenance).await.unwrap();
        let mut disabled=next.policy;disabled.consolidation_enabled=false;
        service.policy_store().replace(next.version,disabled).await.unwrap();
        assert!(service.validate(&capture.lease().binding(),ContextUse::MemoryMaintenance).await.is_err());
        let mut snapshot=backend.write_snapshot(f.owner).await.unwrap();snapshot.source_memories=vec![(alpha.to_string(),1)];
        assert!(service.materialize_memory_capture(&snapshot,MemoryWriteOrigin::Maintenance).await.is_err());
    }).await;
}

#[tokio::test]
async fn memory_search_options_only_narrow_policy_and_empty_layers_do_not_expand() {
    with_db(|f| async move {
        use alia_server::providers::adapter::RetrievalOpts;
        let service = f.service();
        f.item("alpha one").await;
        f.item("alpha two").await;
        let mut policy = service.policy_store().current().await.unwrap();
        policy.policy.max_memory_results = 1;
        policy.policy.similarity_threshold = 0.6;
        service
            .policy_store()
            .replace(policy.version, policy.policy)
            .await
            .unwrap();
        let original = service
            .materialize(f.source("alpha").await, "alpha")
            .await
            .unwrap();
        let capped = service
            .extend_memory_with_opts(
                &original,
                "alpha",
                RetrievalOpts {
                    max_results: Some(20),
                    min_similarity: Some(0.0),
                    layers: Some(vec!["core".into()]),
                },
            )
            .await
            .unwrap();
        assert_eq!(capped.memory().project_context.len(), 1);
        let strict = service
            .extend_memory_with_opts(
                &original,
                "alpha",
                RetrievalOpts {
                    min_similarity: Some(1.0),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(strict.memory().is_empty());
        for layers in [vec!["episodic".into()], vec![]] {
            let empty = service
                .extend_memory_with_opts(
                    &original,
                    "alpha",
                    RetrievalOpts {
                        layers: Some(layers),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            assert!(empty.memory().is_empty());
            assert!(empty.memory_prompt().is_empty());
            assert!(!empty.all_materialized_memory().is_empty());
        }
        assert!(
            service
                .extend_memory_with_opts(
                    &original,
                    "alpha",
                    RetrievalOpts {
                        max_results: Some(0),
                        ..Default::default()
                    }
                )
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn source_clock_skew_never_changes_capture_epoch_authority() {
    with_db(|f|async move {
        let service=f.service();let backend=f.memory_backend().await;
        let old=f.source("source clock is ahead").await;
        let row=f.db.query_one(stmt("UPDATE chat_messages SET source_received_at=clock_timestamp()+interval '1 hour' WHERE id=$1 RETURNING source_received_at",vec![old.source_event_id.into()])).await.unwrap().unwrap();
        let ahead=row.try_get::<chrono::DateTime<Utc>>("","source_received_at").unwrap();
        let mut snapshot=backend.write_snapshot(f.owner).await.unwrap();snapshot.source_event_ids=vec![old.source_event_id.unwrap()];snapshot.exact_source_received_at=Some((old.source_event_id.unwrap(),ahead));
        assert!(ahead>snapshot.source_received_at);
        let captured=service.materialize_memory_capture(&snapshot,MemoryWriteOrigin::Automatic).await.unwrap();
        assert_eq!(captured.messages()[0].id(),old.source_event_id.unwrap());
        let mut wrong=snapshot.clone();wrong.exact_source_received_at=Some((old.source_event_id.unwrap(),old.source_received_at));
        assert!(service.materialize_memory_capture(&wrong,MemoryWriteOrigin::Automatic).await.is_err());
        backend.clear(f.owner).await.unwrap();
        assert!(service.materialize_memory_capture(&snapshot,MemoryWriteOrigin::Automatic).await.is_err());
        let new=f.source("fresh expression after clear").await;
        f.sql(&format!("UPDATE chat_messages SET source_received_at=clock_timestamp()+interval '1 hour' WHERE id='{}'",new.source_event_id.unwrap())).await;
        let mut fresh=backend.write_snapshot(f.owner).await.unwrap();fresh.source_event_ids=vec![new.source_event_id.unwrap()];
        let captured=service.materialize_memory_capture(&fresh,MemoryWriteOrigin::Automatic).await.unwrap();
        assert_eq!(captured.lease().source().source_privacy_epoch,old.source_privacy_epoch+1);
        assert_eq!(captured.messages()[0].id(),new.source_event_id.unwrap());
    }).await;
}

#[tokio::test]
async fn profiled_embedding_reads_matching_memory_vector_in_the_context_snapshot() {
    with_db(|f|async move {
        let (service,adapter)=gated_service(&f).await;let memory=f.item("no lexical match").await;
        let vector=format!("[{}]",vec!["1";1536].join(","));
        f.db.execute(stmt("INSERT INTO memory_embeddings_1536(memory_id,profile_id,embedding,source_revision) VALUES($1,$2,$3::vector,1)",vec![memory.into(),adapter.profile.id().unwrap().into(),vector.into()])).await.unwrap();
        let source=f.source("query").await;
        let task=tokio::spawn(async move{service.materialize(source,"query").await});
        tokio::time::timeout(Duration::from_secs(2),adapter.entered.notified()).await.unwrap();adapter.release.add_permits(1);
        let context=task.await.unwrap().unwrap();
        assert_eq!(context.memory().project_context[0].id,memory.to_string());
        assert_eq!(context.memory().retrieval_trace[0].retrieval_path,"vector_hnsw");
    }).await;
}
