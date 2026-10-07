//! Real PostgreSQL ownership, revocation and projection behavior.
use alia_server::modules::memory::{MemoryNode, MemoryWriteOrigin};
use alia_server::{
    config::{ConversationSection, MemorySection, ServerConfig},
    expression::{ExpressionRuntime, rule_based_user_appraisals},
    modules::{
        memory::{context::*, source::capture_source_epochs},
        platform::migrations,
    },
    providers::embedding::NoopEmbedding,
};
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
    Value,
};
use std::{future::Future, panic::AssertUnwindSafe, path::Path, sync::Arc, time::Duration};
use uuid::Uuid;

fn sql(text: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, text, values)
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
    async fn new(before_erasure_migration: bool) -> Self {
        let url =
            std::env::var("ALIA_TEST_DATABASE_URL").expect("real disposable PostgreSQL required");
        let mut opts = ConnectOptions::new(url.clone());
        opts.max_connections(1).sqlx_logging(false);
        let admin = Database::connect(opts).await.unwrap();
        let schema = format!("appraisal_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut opts = ConnectOptions::new(url);
        opts.max_connections(6)
            .sqlx_logging(false)
            .set_schema_search_path(format!("{schema},public"));
        let db = Database::connect(opts).await.unwrap();
        let f = Self {
            db,
            admin,
            schema,
            owner: Uuid::new_v4(),
            session: Uuid::new_v4(),
        };
        let setup = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(45), async {
        let migrated = if before_erasure_migration {
            // This is an upgrade of the task's real PG database, whose vector
            // extension is already shared in public by the current server.
            let public_vector = f.db.query_one(sql(
                "SELECT 1 AS ready FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='vector' AND n.nspname='public'",
                vec![],
            )).await.unwrap();
            assert!(
                public_vector.is_some(),
                "legacy upgrade fixture requires public pgvector"
            );
            let next = migrations::MIGRATIONS
                .iter()
                .position(|migration| {
                    migration.version == "m20260920_000012_appraisal_memory_erasure.sql"
                })
                .unwrap();
            migrations::apply(&f.db, &migrations::MIGRATIONS[..next]).await
        } else {
            migrations::run(&f.db).await
        };
        migrated.expect("Appraisal fixture migration failed");
        f.db.execute(sql("INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Appraisal test','not-a-login','user','active')",vec![f.owner.into(),format!("{}@appraisal.test",f.owner).into()])).await.unwrap();
        f.db.execute(sql(
            "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'appraisal-device',$2)",
            vec![f.session.into(), f.owner.to_string().into()],
        ))
        .await
        .unwrap();
        f.service()
            .initialize_policy(
                ContextPolicy::from_config(
                    &MemorySection::default(),
                    &ConversationSection::default(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        })).catch_unwind().await;
        match setup {
            Ok(Ok(())) => f,
            result => {
                f.cleanup().await;
                match result {
                    Err(error) => std::panic::resume_unwind(error),
                    _ => panic!("Appraisal fixture setup exceeded 45 seconds"),
                }
            }
        }
    }
    fn service(&self) -> ContextService {
        ContextService::new(self.db.clone(), Arc::new(NoopEmbedding))
    }
    async fn runtime(&self) -> ExpressionRuntime {
        let config = ServerConfig::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/server.example.toml"),
        )
        .unwrap();
        let runtime = ExpressionRuntime::from_config(&config);
        runtime.attach_db(self.db.clone()).await.unwrap();
        runtime
    }
    async fn source(&self, text: &str) -> ContextSource {
        let stamps = capture_source_epochs(&self.db, self.owner).await.unwrap();
        let id = Uuid::new_v4();
        let row=self.db.query_one(sql("INSERT INTO chat_messages(id,session_id,user_id,role,content,created_at,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,clock_timestamp(),clock_timestamp(),$5,$6) RETURNING source_received_at",vec![id.into(),self.session.into(),self.owner.to_string().into(),text.into(),stamps.privacy_epoch.into(),stamps.relationship_epoch.into()])).await.unwrap().unwrap();
        ContextSource {
            owner_id: self.owner,
            request_id: Uuid::new_v4(),
            channel_id: "appraisal-device".into(),
            conversation_id: Some(self.session),
            source_event_id: Some(id),
            source_received_at: row.try_get("", "source_received_at").unwrap(),
            source_privacy_epoch: stamps.privacy_epoch,
        }
    }
    async fn generation(&self, text: &str) -> GenerationContext {
        self.service()
            .materialize(self.source(text).await, text)
            .await
            .unwrap()
    }
    async fn records(&self) -> i64 {
        self.db
            .query_one(sql("SELECT count(*) AS n FROM appraisal_records", vec![]))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }
    async fn cleanup(self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            let db = self.db.close().await;
            let schema = self
                .admin
                .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .await;
            let admin = self.admin.close().await;
            db.unwrap();
            schema.unwrap();
            admin.unwrap();
        })
        .await
        .expect("Appraisal schema cleanup exceeded 10 seconds");
    }
}
async fn with_db<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    with_db_at(false, test).await;
}
async fn with_db_at<F, Fut>(before_erasure_migration: bool, test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let f = Fixture::new(before_erasure_migration).await;
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(45),
        test(f.clone()),
    ))
    .catch_unwind()
    .await;
    f.cleanup().await;
    match result {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("appraisal test exceeded 45 seconds"),
        Err(error) => std::panic::resume_unwind(error),
    }
}

#[tokio::test]
async fn committed_judgment_is_idempotent_and_enters_complete_projection_roots() {
    with_db(|f| async move {
        let service = f.service();
        let runtime = f.runtime().await;
        let generation = f.generation("thank you").await;
        let prepared = service.prepare_user_appraisal(&generation).await.unwrap();
        assert!(
            service
                .appraisal_receipt(&prepared)
                .await
                .unwrap()
                .is_none()
        );
        let before = service.extend_affect(&generation).await.unwrap();
        let first = runtime
            .observe_context_user(&service, &generation)
            .await
            .unwrap();
        let second = runtime
            .observe_context_user(&service, &generation)
            .await
            .unwrap();
        assert_eq!(first.appraisal_id, second.appraisal_id);
        assert_eq!(first.result_digest, second.result_digest);
        assert_eq!(f.records().await, 1);
        assert_eq!(
            service
                .appraisal_receipt(&prepared)
                .await
                .unwrap()
                .unwrap()
                .result_digest,
            first.result_digest
        );
        let projected = service.extend_affect(&generation).await.unwrap();
        assert!(
            projected.affect().unwrap().snapshot().social.trust
                > before.affect().unwrap().snapshot().social.trust
        );
        assert!(
            projected
                .lease()
                .dependencies()
                .contains(&ContextEvidence::Appraisal {
                    id: first.appraisal_id,
                    revision: 1,
                    result_digest: first.result_digest
                })
        );
        assert!(
            generation
                .lease()
                .dependencies()
                .iter()
                .all(|root| projected.lease().dependencies().contains(root))
        );
        service
            .validate(&projected.lease().binding(), ContextUse::OutputAdmission)
            .await
            .unwrap();
        runtime.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn short_event_expiry_revokes_projection_but_preserves_live_relationship_event() {
    with_db(|f| async move {
        let service = f.service();
        let store = service.policy_store();
        let mut current = store.current().await.unwrap();
        current.policy.appraisal.affect.session_appraisal_ttl_sec = 1;
        current
            .policy
            .appraisal
            .affect
            .relationship_appraisal_ttl_sec = 60;
        let baseline = current.policy.appraisal.affect.baseline_trust;
        store
            .replace(current.version, current.policy)
            .await
            .unwrap();
        let runtime = f.runtime().await;
        let generation = f.generation("thank you").await;
        let input = service.prepare_user_appraisal(&generation).await.unwrap();
        runtime
            .observe_context_user(&service, &generation)
            .await
            .unwrap();
        let first = service.extend_affect(&generation).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(
            service
                .validate(&first.lease().binding(), ContextUse::OutputAdmission)
                .await
                .is_err()
        );
        assert!(service.appraisal_receipt(&input).await.unwrap().is_some());
        assert!(service.relationship_profile(f.owner).await.unwrap().trust > baseline);
        let fresh = f.generation("continuing the conversation").await;
        let projected = service.extend_affect(&fresh).await.unwrap();
        assert!(projected.affect().unwrap().snapshot().social.trust > baseline);
        assert!(
            !projected
                .affect()
                .unwrap()
                .snapshot()
                .dominant_causes
                .iter()
                .any(|kind| kind == "user_gratitude")
        );
        service
            .validate(&projected.lease().binding(), ContextUse::OutputAdmission)
            .await
            .unwrap();
        runtime.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn delayed_draft_cannot_refresh_relationship_epoch() {
    with_db(|f|async move {
        let service=f.service();let runtime=f.runtime().await;let generation=f.generation("thank you").await;
        let input=service.prepare_user_appraisal(&generation).await.unwrap();
        let drafts=rule_based_user_appraisals(input.text(),input.effective_at(),&input.policy().affect);
        f.db.execute(sql("UPDATE relationship_authority SET relationship_epoch=relationship_epoch+1 WHERE owner_id=$1",vec![f.owner.into()])).await.unwrap();
        assert!(runtime.submit_appraisal(input,drafts).await.is_err());assert_eq!(f.records().await,0);
        assert!(service.prepare_user_appraisal(&generation).await.is_err());
        let fresh=f.generation("thank you again").await;
        runtime.observe_context_user(&service,&fresh).await.unwrap();assert_eq!(f.records().await,1);
        runtime.shutdown().await.unwrap();
    }).await;
}

#[tokio::test]
async fn source_revision_change_rejects_pending_interpretation() {
    with_db(|f|async move {
        let memory=Uuid::new_v4();
        f.db.execute(sql("INSERT INTO memory_items(id,tenant_id,user_id,kind,scope,memory_layer,node_type,content) VALUES($1,$2,$3,'semantic','user','core','fact','tea')",vec![memory.into(),Uuid::nil().into(),f.owner.into()])).await.unwrap();
        let service=f.service();let runtime=f.runtime().await;let generation=f.generation("thank you tea").await;
        assert!(generation.lease().dependencies().iter().any(|root|matches!(root,ContextEvidence::Memory{id,..} if *id==memory)));
        let input=service.prepare_user_appraisal(&generation).await.unwrap();
        let drafts=rule_based_user_appraisals(input.text(),input.effective_at(),&input.policy().affect);
        f.db.execute(sql("UPDATE memory_items SET revision=revision+1,content='new tea' WHERE id=$1",vec![memory.into()])).await.unwrap();
        assert!(runtime.submit_appraisal(input,drafts).await.is_err());assert_eq!(f.records().await,0);
        runtime.shutdown().await.unwrap();
    }).await;
}

#[tokio::test]
async fn privacy_clear_erases_projections_and_rejects_their_output_authority() {
    with_db(|f| async move {
        let service = f.service();
        let runtime = f.runtime().await;
        let generation = f.generation("thank you").await;
        runtime
            .observe_context_user(&service, &generation)
            .await
            .unwrap();
        let projected = service.extend_affect(&generation).await.unwrap();
        assert_eq!(f.records().await, 1);
        f.db.execute(sql(
            "UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1 WHERE user_id=$1",
            vec![f.owner.into()],
        ))
        .await
        .unwrap();
        assert_eq!(f.records().await, 0);
        assert!(
            service
                .validate(&projected.lease().binding(), ContextUse::OutputAdmission)
                .await
                .is_err()
        );
        let count: i64 =
            f.db.query_one(sql("SELECT count(*) AS n FROM affect_projections", vec![]))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "n")
                .unwrap();
        assert_eq!(count, 0);
        runtime.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn same_identity_different_draft_does_not_write() {
    with_db(|f| async move {
        let service = f.service();
        let runtime = f.runtime().await;
        let generation = f.generation("thank you").await;
        let input = service.prepare_user_appraisal(&generation).await.unwrap();
        let mut drafts =
            rule_based_user_appraisals(input.text(), input.effective_at(), &input.policy().affect);
        runtime
            .submit_appraisal(input.clone(), drafts.clone())
            .await
            .unwrap();
        drafts[0].deltas.trust += 0.01;
        assert!(runtime.submit_appraisal(input, drafts).await.is_err());
        assert_eq!(f.records().await, 1);
        runtime.shutdown().await.unwrap();
    })
    .await;
}

async fn memory(f: &Fixture) -> alia_server::modules::memory::MemoryService {
    let memory = alia_server::modules::memory::MemoryService::new();
    memory
        .attach_runtime_for_test(f.db.clone(), None, MemorySection::default())
        .await
        .unwrap();
    memory
}
fn node(content: &str) -> alia_server::modules::memory::MemoryNode {
    alia_server::modules::memory::MemoryNode::new("preference", "preference", content, 0.9)
        .with_runtime_fields("preference", "user", 0.9, 0.9, "internal")
}

#[tokio::test]
async fn relationship_clear_is_idempotent_and_erases_only_its_owner_derived_memories() {
    use alia_server::modules::memory::{MemoryWriteOrigin, RetrievalOpts};
    with_db(|f| async move {
        let service = f.service();
        let runtime = f.runtime().await;
        let memory = memory(&f).await;
        let independent = memory
            .store(f.owner, node("independent coffee preference"))
            .await
            .unwrap();
        let generation = f.generation("thank you for tea").await;
        runtime
            .observe_context_user(&service, &generation)
            .await
            .unwrap();
        let projected = service.extend_affect(&generation).await.unwrap();
        let write = memory
            .bind_context(
                memory.write_snapshot(f.owner).await.unwrap(),
                projected.lease().id(),
                MemoryWriteOrigin::Automatic,
            )
            .await
            .unwrap();
        let dependent = memory
            .store_guarded(&write, node("derived tea preference"))
            .await
            .unwrap();
        assert!(service.relationship_profile(f.owner).await.unwrap().trust > 0.5);
        let operation = Uuid::new_v4();
        let first = service
            .clear_relationship(f.owner, operation, 0)
            .await
            .unwrap();
        let again = service
            .clear_relationship(f.owner, operation, 0)
            .await
            .unwrap();
        assert_eq!(first, again);
        assert_eq!(first.relationship_epoch, 1);
        assert_eq!(first.removed_memories, 1);
        assert!(
            service
                .clear_relationship(f.owner, Uuid::new_v4(), 0)
                .await
                .is_err()
        );
        assert!(
            service
                .validate(&projected.lease().binding(), ContextUse::OutputAdmission)
                .await
                .is_err()
        );
        assert!(memory.get(f.owner, &dependent).await.unwrap().is_none());
        let old =
            f.db.query_one(sql(
                "SELECT status,content FROM memory_items WHERE id=$1 AND user_id=$2",
                vec![Uuid::parse_str(&dependent).unwrap().into(), f.owner.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old.try_get::<String>("", "status").unwrap(), "deleted");
        assert!(old.try_get::<String>("", "content").unwrap().is_empty());
        assert_eq!(
            memory
                .get(f.owner, &independent)
                .await
                .unwrap()
                .unwrap()
                .status,
            "active"
        );
        assert!(
            memory
                .retrieve(f.owner, "tea", RetrievalOpts::default())
                .await
                .unwrap()
                .iter()
                .all(|item| item.id.as_deref() != Some(dependent.as_str()))
        );
        let fresh = f.generation("new coffee source").await;
        assert!(fresh.lease().dependencies().iter().all(
            |item| !matches!(item,ContextEvidence::Memory{id,..} if id.to_string()==dependent)
        ));
        runtime.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn appraisal_cannot_rewrite_an_ancestor_memory_it_consumed() {
    use alia_server::modules::memory::{MemoryPatch, MemoryWriteOrigin};
    with_db(|f| async move {
        let memory = memory(&f).await;
        let service = f.service();
        let runtime = f.runtime().await;
        let parent = memory.store(f.owner, node("tea preference")).await.unwrap();
        let generation = f.generation("thank you tea").await;
        runtime
            .observe_context_user(&service, &generation)
            .await
            .unwrap();
        let projected = service.extend_affect(&generation).await.unwrap();
        let write = memory
            .bind_context(
                memory.write_snapshot(f.owner).await.unwrap(),
                projected.lease().id(),
                MemoryWriteOrigin::Automatic,
            )
            .await
            .unwrap();
        let result = memory
            .update_guarded(
                &write,
                &parent,
                MemoryPatch {
                    expected_revision: Some(1),
                    content: Some("rewritten tea preference".into()),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            result.is_err(),
            "a judgment cannot make its own consumed ancestor a new fact"
        );
        assert_eq!(
            memory
                .get(f.owner, &parent)
                .await
                .unwrap()
                .unwrap()
                .revision,
            1
        );
        runtime.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn committed_child_is_excluded_after_parent_revision_private_or_expiry_change() {
    use alia_server::modules::memory::{MemoryListOpts, MemoryPatch, RetrievalOpts};
    for change in ["revision", "private", "expired"] {
        with_db(move |f| async move {
            let memory = memory(&f).await;
            let parent = memory.store(f.owner, node("tea parent")).await.unwrap();
            let mut write = memory.write_snapshot(f.owner).await.unwrap();
            write.source_memories.push((parent.clone(), 1));
            let child = memory
                .store_guarded(&write, node("tea child"))
                .await
                .unwrap();
            assert!(
                memory
                    .retrieve(f.owner, "tea", RetrievalOpts::default())
                    .await
                    .unwrap()
                    .iter()
                    .any(|m| m.id.as_deref() == Some(child.as_str()))
            );
            assert!(
                memory
                    .graph_for_user(f.owner, MemoryListOpts::default())
                    .await
                    .unwrap()
                    .nodes
                    .iter()
                    .any(|m| m.content == "tea child")
            );
            let patch = match change {
                "revision" => MemoryPatch {
                    expected_revision: Some(1),
                    content: Some("coffee parent".into()),
                    ..Default::default()
                },
                "private" => MemoryPatch {
                    expected_revision: Some(1),
                    visibility: Some("private_runtime".into()),
                    ..Default::default()
                },
                _ => MemoryPatch {
                    expected_revision: Some(1),
                    status: Some("expired".into()),
                    ..Default::default()
                },
            };
            memory.update(f.owner, &parent, patch).await.unwrap();
            assert!(
                memory
                    .retrieve(f.owner, "tea", RetrievalOpts::default())
                    .await
                    .unwrap()
                    .iter()
                    .all(|m| m.id.as_deref() != Some(child.as_str())),
                "standalone {change}"
            );
            assert!(
                memory
                    .graph_for_user(f.owner, MemoryListOpts::default())
                    .await
                    .unwrap()
                    .nodes
                    .iter()
                    .all(|m| m.content != "tea child"),
                "standalone graph {change}"
            );
            let generation = f.generation("tea").await;
            assert!(
                generation.lease().dependencies().iter().all(
                    |item| !matches!(item,ContextEvidence::Memory{id,..} if id.to_string()==child)
                ),
                "Context {change}"
            );
        })
        .await;
    }
}

#[tokio::test]
async fn unrelated_forget_does_not_reauthorize_or_hide_independent_committed_memory() {
    use alia_server::modules::memory::RetrievalOpts;
    with_db(|f|async move {
        let memory=memory(&f).await;let source=f.source("original tea preference").await;
        let mut write=memory.write_snapshot(f.owner).await.unwrap();write.source_event_ids=vec![source.source_event_id.unwrap()];
        let kept=memory.store_guarded(&write,node("tea preference retained")).await.unwrap();
        let forgotten=memory.store(f.owner,node("unrelated coffee")).await.unwrap();
        memory.delete(f.owner,&forgotten).await.unwrap();
        assert!(memory.retrieve(f.owner,"tea",RetrievalOpts::default()).await.unwrap().iter().any(|m|m.id.as_deref()==Some(kept.as_str())));
        let generation=f.generation("tea").await;
        assert!(generation.lease().dependencies().iter().any(|root|matches!(root,ContextEvidence::MemorySourceMessage{id,..} if Some(*id)==source.source_event_id)));
        assert!(f.service().materialize(source,"original tea preference").await.is_err(),"old input authority must stay revoked");
    }).await;
}

#[tokio::test]
async fn initialization_preserves_current_policy_and_rejects_unsupported_or_damaged_rows() {
    async fn snapshot(db: &DatabaseConnection) -> serde_json::Value {
        db.query_one(sql(
            "SELECT jsonb_build_object('version',version,'policy',policy,'policy_digest',policy_digest,'updated_at',updated_at) AS snapshot FROM context_policy WHERE singleton",
            vec![],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "snapshot")
        .unwrap()
    }

    with_db(|f| async move {
        let service = f.service();
        let stored = service.policy_store().current().await.unwrap();
        let before = snapshot(&f.db).await;
        let mut seed = stored.policy.clone();
        seed.max_memory_results = 1;
        seed.appraisal.affect.baseline_trust = 0.37;
        assert_ne!(seed, stored.policy);
        let restarted = service.initialize_policy(seed.clone()).await.unwrap();
        assert_eq!(restarted.version, stored.version);
        assert_eq!(restarted.policy, stored.policy);
        assert_eq!(snapshot(&f.db).await, before);

        let current = serde_json::to_value(&stored.policy).unwrap();
        let mut old = current.clone();
        old.as_object_mut().unwrap().remove("appraisal");
        let old_digest = evidence_digest(&old).unwrap();
        for (policy, digest, expected_error) in [
            (old, old_digest, "missing field `appraisal`"),
            (current, "0".repeat(64), "context policy digest mismatch"),
        ] {
            f.db.execute(sql(
                "UPDATE context_policy SET policy=$1,policy_digest=$2 WHERE singleton",
                vec![policy.into(), digest.into()],
            ))
            .await
            .unwrap();
            let rejected = snapshot(&f.db).await;
            let error = service.initialize_policy(seed.clone()).await.unwrap_err();
            assert!(error.to_string().contains(expected_error), "{error:#}");
            assert!(service.policy_store().current().await.is_err());
            assert_eq!(
                snapshot(&f.db).await,
                rejected,
                "rejection must not repair or replace stored policy"
            );
        }
    })
    .await;
}

#[tokio::test]
async fn negative_zero_policy_survives_sql_restart_and_projection_validation() {
    with_db(|f| async move {
        let service = f.service();
        let current = service.policy_store().current().await.unwrap();
        let mut next = current.policy.clone();
        next.appraisal.affect.baseline_valence = -0.0;
        next.appraisal.affect.baseline_calmness = 0.37;
        let written = service
            .policy_store()
            .replace(current.version, next)
            .await
            .unwrap();
        let reread = service.policy_store().current().await.unwrap();
        assert_eq!(written.version, reread.version);
        assert_eq!(written.policy, reread.policy);
        let mut different_seed = current.policy;
        different_seed.appraisal.affect.baseline_valence = 0.8;
        let restarted = f.service().initialize_policy(different_seed).await.unwrap();
        assert_eq!(restarted.version, reread.version);
        assert_eq!(restarted.policy, reread.policy);
        let generation = f.generation("plain neutral input").await;
        let projected = service.extend_affect(&generation).await.unwrap();
        assert_eq!(projected.affect().unwrap().snapshot().affect.valence, 0.0);
        service
            .validate(&projected.lease().binding(), ContextUse::OutputAdmission)
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
async fn global_nan_and_private_draft_evidence_cannot_acquire_authority() {
    use alia_server::expression::AppraisalScope;
    with_db(|f| async move {
        let service = f.service();
        let runtime = f.runtime().await;
        let generation = f.generation("thank you").await;
        let input = service.prepare_user_appraisal(&generation).await.unwrap();
        let original =
            rule_based_user_appraisals(input.text(), input.effective_at(), &input.policy().affect);
        let mut global = original.clone();
        global[0].scope = AppraisalScope::Global;
        assert!(
            runtime
                .submit_appraisal(input.clone(), global)
                .await
                .is_err()
        );
        let mut nonfinite = original.clone();
        nonfinite[0].confidence = f32::NAN;
        assert!(
            runtime
                .submit_appraisal(input.clone(), nonfinite)
                .await
                .is_err()
        );
        let mut private = original;
        private[0].evidence.summary = "private raw source copy".into();
        private[0].ttl_sec = u64::MAX;
        private[0].deltas.calmness = -0.0;
        private[0].confidence = -0.0;
        runtime
            .submit_appraisal(input.clone(), private)
            .await
            .unwrap();
        assert!(
            service.appraisal_receipt(&input).await.unwrap().is_some(),
            "negative zero must survive the JSONB round trip without digest drift"
        );
        let row =
            f.db.query_one(sql(
                "SELECT events::text AS text FROM appraisal_records",
                vec![],
            ))
            .await
            .unwrap()
            .unwrap();
        let text: String = row.try_get("", "text").unwrap();
        assert!(!text.contains("private raw source copy"));
        assert!(!text.contains(&u64::MAX.to_string()));
        runtime.shutdown().await.unwrap();
    })
    .await;
}

mod privacy_erasure {
    use super::*;
    use alia_server::{
        modules::{memory::MemoryService, platform::lifecycle::ModuleLifecycle},
        providers::adapter::*,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::BTreeMap;

    struct Embedding;
    impl Embedding {
        fn profile() -> EmbeddingProfile {
            EmbeddingProfile {
                provider: "appraisal-erasure-fixture".into(),
                endpoint: "http://appraisal-erasure.test/v1".into(),
                model: "canonical-memory-3".into(),
                revision: "request-config.v1".into(),
                dimensions: 3,
                distance: "cosine".into(),
                vector_type: "vector".into(),
            }
        }
    }
    #[async_trait]
    impl ModuleLifecycle for Embedding {
        fn name(&self) -> &'static str {
            "appraisal_erasure_embedding"
        }
    }
    #[async_trait]
    impl EmbeddingAdapter for Embedding {
        async fn embed(&self, _: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
            anyhow::bail!("erasure fixture requires the actual request profile")
        }
        async fn preview_profile_settings(
            &self,
            _: &BTreeMap<String, serde_json::Value>,
        ) -> anyhow::Result<Option<EmbeddingProfile>> {
            Ok(Some(Self::profile()))
        }
        async fn embed_profiled(
            &self,
            texts: &[String],
        ) -> anyhow::Result<Option<ProfiledEmbeddings>> {
            Ok(Some(ProfiledEmbeddings {
                profile: Self::profile(),
                vectors: texts
                    .iter()
                    .map(|text| EmbeddingVector {
                        input_digest: embedding_text_digest(text),
                        values: vec![1.0, 2.0, 3.0],
                    })
                    .collect(),
            }))
        }
        fn dimension(&self) -> usize {
            3
        }
    }

    #[derive(Clone)]
    struct ErasureFixture {
        base: Fixture,
        memory: Arc<MemoryService>,
        runtime: Arc<ExpressionRuntime>,
    }
    struct Memories {
        input: String,
        dependent: String,
        child: String,
        unrelated: String,
        kept: String,
        other_owner: Uuid,
        other: String,
    }
    fn distinct(content: &str, scope: &str) -> MemoryNode {
        let mut value = node(content);
        value.id = Some(Uuid::new_v4().to_string());
        value.scope = scope.into();
        value
    }
    impl ErasureFixture {
        async fn memories(&self) -> Memories {
            let f = &self.base;
            let input = self
                .memory
                .store(
                    f.owner,
                    distinct("Cedar tea is the user's preference", "global"),
                )
                .await
                .unwrap();
            let generation = f.generation("thank you for remembering Cedar tea").await;
            assert!(
                generation.lease().dependencies().iter().any(
                    |root| matches!(root,ContextEvidence::Memory{id,..} if id.to_string()==input)
                ),
                "the actual original Memory must be consumed by Appraisal"
            );
            self.runtime
                .observe_context_user(&f.service(), &generation)
                .await
                .unwrap();
            let projected = f.service().extend_affect(&generation).await.unwrap();
            let write = self
                .memory
                .bind_context(
                    self.memory.write_snapshot(f.owner).await.unwrap(),
                    projected.lease().id(),
                    MemoryWriteOrigin::Automatic,
                )
                .await
                .unwrap();
            let dependent = self
                .memory
                .store_guarded(&write, distinct("Relational Cedar tea guidance", "global"))
                .await
                .unwrap();
            let mut write = self.memory.write_snapshot(f.owner).await.unwrap();
            write.source_memories.push((dependent.clone(), 1));
            let child = self
                .memory
                .store_guarded(&write, distinct("Further Cedar tea conclusion", "device"))
                .await
                .unwrap();
            // Created after the Appraisal: its deletion advances privacy but
            // has no ordinary memory_sources edge to the Appraisal outputs.
            let unrelated = self
                .memory
                .store(f.owner, distinct("Unrelated orchid reminder", "session"))
                .await
                .unwrap();
            let source = f.source("Independent Willow workshop fact").await;
            let mut write = self.memory.write_snapshot(f.owner).await.unwrap();
            write.source_event_ids.push(source.source_event_id.unwrap());
            let kept = self
                .memory
                .store_guarded(
                    &write,
                    distinct("Independent Willow workshop fact", "global"),
                )
                .await
                .unwrap();
            let other_owner = Uuid::new_v4();
            f.db.execute(sql("INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Other erasure owner','not-a-login','user','active')",
                vec![other_owner.into(),format!("{other_owner}@erasure.test").into()])).await.unwrap();
            let other = self
                .memory
                .store(
                    other_owner,
                    distinct("Other owner's Cedar tea fact", "global"),
                )
                .await
                .unwrap();
            for id in [&input, &dependent, &child, &unrelated, &kept, &other] {
                assert_eq!(self.artifact_count("memory_embeddings", id).await, 1);
                assert!(self.graph_count(id).await > 0);
            }
            assert!(
                self.artifact_count("memory_message_sources", &dependent)
                    .await
                    > 0
            );
            assert_eq!(
                self.artifact_count("memory_appraisal_sources", &dependent)
                    .await,
                1
            );
            Memories {
                input,
                dependent,
                child,
                unrelated,
                kept,
                other_owner,
                other,
            }
        }
        async fn artifact_count(&self, table: &str, id: &str) -> i64 {
            assert!(matches!(
                table,
                "memory_embeddings" | "memory_message_sources" | "memory_appraisal_sources"
            ));
            self.base
                .db
                .query_one(sql(
                    &format!("SELECT count(*) AS n FROM {table} WHERE memory_id=$1::uuid"),
                    vec![id.into()],
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "n")
                .unwrap()
        }
        async fn graph_count(&self, id: &str) -> i64 {
            self.base.db.query_one(sql(
                "SELECT (SELECT count(*) FROM memory_edges WHERE evidence_memory_id=$1::uuid)+(SELECT count(*) FROM memory_nodes WHERE node_key='memory:'||$1 OR properties->>'memory_id'=$1) AS n",
                vec![id.into()],
            )).await.unwrap().unwrap().try_get("","n").unwrap()
        }
        async fn physical_state(&self, id: &str) -> serde_json::Value {
            let row=self.base.db.query_one(sql(
                "SELECT status,content,summary,metadata,evidence,cardinality(source_event_ids) AS source_count,cardinality(topic_tags) AS topic_count,cardinality(affect_tags) AS affect_count FROM memory_items WHERE id=$1::uuid AND user_id=$2 AND tenant_id=$3",
                vec![id.into(),self.base.owner.into(),Uuid::nil().into()],
            )).await.unwrap().unwrap();
            json!({
                "visible_via_get":self.memory.get(self.base.owner,id).await.unwrap().is_some(),
                "status":row.try_get::<String>("","status").unwrap(),
                "content":row.try_get::<String>("","content").unwrap(),
                "summary":row.try_get::<Option<String>>("","summary").unwrap(),
                "metadata":row.try_get::<serde_json::Value>("","metadata").unwrap(),
                "evidence":row.try_get::<serde_json::Value>("","evidence").unwrap(),
                "source_count":row.try_get::<i32>("","source_count").unwrap(),
                "topic_count":row.try_get::<i32>("","topic_count").unwrap(),
                "affect_count":row.try_get::<i32>("","affect_count").unwrap(),
                "vectors":self.artifact_count("memory_embeddings",id).await,
                "message_pins":self.artifact_count("memory_message_sources",id).await,
                "appraisal_pins":self.artifact_count("memory_appraisal_sources",id).await,
                "graph":self.graph_count(id).await,
            })
        }
        async fn assert_erased(&self, id: &str) {
            assert_eq!(
                self.physical_state(id).await,
                json!({
                    "visible_via_get":false,"status":"deleted","content":"","summary":null,
                    "metadata":{},"evidence":{},"source_count":0,"topic_count":0,"affect_count":0,
                    "vectors":0,"message_pins":0,"appraisal_pins":0,"graph":0,
                }),
                "revoked Appraisal-derived canonical Memory must be physically erased"
            );
        }
        async fn assert_independent(&self, m: &Memories) {
            assert!(
                self.memory
                    .get(self.base.owner, &m.kept)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert!(
                self.memory
                    .get(m.other_owner, &m.other)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(
                self.artifact_count("memory_message_sources", &m.kept).await,
                1
            );
            for id in [&m.kept, &m.other] {
                assert_eq!(self.artifact_count("memory_embeddings", id).await, 1);
                assert!(self.graph_count(id).await > 0);
            }
        }
        async fn forget(&self, id: &str) -> anyhow::Result<u64> {
            let mut context = self.memory.write_snapshot(self.base.owner).await?;
            context.source_memories.push((id.into(), 1));
            self.memory
                .forget_many_guarded(&context, &context.source_memories)
                .await
        }
    }
    async fn with_erasure<F, Fut>(test: F)
    where
        F: FnOnce(ErasureFixture) -> Fut,
        Fut: Future<Output = ()>,
    {
        with_erasure_at(false, test).await;
    }
    async fn with_erasure_at<F, Fut>(before_erasure_migration: bool, test: F)
    where
        F: FnOnce(ErasureFixture) -> Fut,
        Fut: Future<Output = ()>,
    {
        with_db_at(before_erasure_migration, |base| async move {
            let config = ServerConfig::load(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/server.example.toml"),
            )
            .unwrap();
            let f = ErasureFixture {
                base,
                memory: Arc::new(MemoryService::new()),
                runtime: Arc::new(ExpressionRuntime::from_config(&config)),
            };
            let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(30), async {
                f.memory
                    .attach_runtime_for_test(
                        f.base.db.clone(),
                        Some(Arc::new(Embedding)),
                        MemorySection::default(),
                    )
                    .await
                    .unwrap();
                let store = ContextPolicyStore::new(f.base.db.clone());
                let current = store.current().await.unwrap();
                let mut policy = current.policy;
                policy.active_embedding_profile = Some(Embedding::profile().id().unwrap());
                store.replace(current.version, policy).await.unwrap();
                f.runtime.attach_db(f.base.db.clone()).await.unwrap();
                test(f.clone()).await;
            }))
            .catch_unwind()
            .await;
            tokio::time::timeout(Duration::from_secs(10), async {
                let memory = f.memory.stop().await;
                let runtime = f.runtime.shutdown().await;
                memory.unwrap();
                runtime.unwrap();
            })
            .await
            .expect("erasure test resource join deadline");
            match result {
                Ok(Ok(())) => (),
                Ok(Err(_)) => panic!("erasure test exceeded 30 seconds"),
                Err(error) => std::panic::resume_unwind(error),
            }
        })
        .await;
    }

    #[tokio::test]
    async fn forgetting_unrelated_memory_erases_all_revoked_appraisal_memory_bodies() {
        with_erasure(|f| async move {
            let m = f.memories().await;
            let count = f.forget(&m.unrelated).await.unwrap();
            assert_eq!(f.base.records().await, 0);
            f.assert_erased(&m.dependent).await;
            f.assert_erased(&m.child).await;
            f.assert_erased(&m.unrelated).await;
            assert_eq!(
                count, 3,
                "each actually erased canonical row is counted once"
            );
            assert!(
                f.memory
                    .get(f.base.owner, &m.input)
                    .await
                    .unwrap()
                    .is_some()
            );
            f.assert_independent(&m).await;
        })
        .await;
    }

    #[tokio::test]
    async fn privacy_epoch_trigger_erases_canonical_appraisal_memory_without_a_forget_call() {
        with_erasure(|f|async move {
            let m=f.memories().await;
            f.base.db.execute(sql("UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1 WHERE user_id=$1",vec![f.base.owner.into()])).await.unwrap();
            assert_eq!(f.base.records().await,0);
            f.assert_erased(&m.dependent).await;
            f.assert_erased(&m.child).await;
            assert!(f.memory.get(f.base.owner,&m.unrelated).await.unwrap().is_some());
            assert!(f.memory.get(f.base.owner,&m.input).await.unwrap().is_some());
            f.assert_independent(&m).await;
        }).await;
    }

    #[tokio::test]
    async fn forgetting_original_appraisal_input_counts_the_overlapping_closure_once() {
        with_erasure(|f| async move {
            let m = f.memories().await;
            let count = f.forget(&m.input).await.unwrap();
            for id in [&m.input, &m.dependent, &m.child] {
                f.assert_erased(id).await;
            }
            assert_eq!(count, 3);
            assert_eq!(f.base.records().await, 0);
            f.assert_independent(&m).await;
        })
        .await;
    }

    #[tokio::test]
    async fn late_sql_failure_rolls_back_memory_erasure_and_both_authority_paths() {
        with_erasure(|f|async move {
            let m=f.memories().await;
            let before=f.physical_state(&m.dependent).await;
            let epoch=f.memory.write_snapshot(f.base.owner).await.unwrap().privacy_epoch;
            let records=f.base.records().await;
            f.base.db.execute_unprepared(&format!(
                "CREATE FUNCTION fail_erasure_after_vectors() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.id='{}'::uuid AND NEW.status='deleted' THEN IF EXISTS(SELECT 1 FROM memory_embeddings WHERE memory_id=NEW.id) THEN RAISE EXCEPTION 'erasure fixture reached the wrong stage'; END IF; RAISE EXCEPTION 'injected failure after vector deletion'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_erasure_after_vectors BEFORE UPDATE OF status ON memory_items FOR EACH ROW EXECUTE FUNCTION fail_erasure_after_vectors()",
                m.child,
            )).await.unwrap();
            let error=f.forget(&m.unrelated).await.unwrap_err();
            assert!(format!("{error:#}").contains("injected failure after vector deletion"));
            assert_eq!(f.physical_state(&m.dependent).await,before);
            assert_eq!(f.memory.write_snapshot(f.base.owner).await.unwrap().privacy_epoch,epoch);
            assert_eq!(f.base.records().await,records);
            assert!(f.memory.get(f.base.owner,&m.unrelated).await.unwrap().is_some());
            let error=f.base.db.execute(sql(
                "UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1 WHERE user_id=$1",vec![f.base.owner.into()],
            )).await.unwrap_err();
            assert!(format!("{error:#}").contains("injected failure after vector deletion"));
            assert_eq!(f.physical_state(&m.dependent).await,before);
            assert_eq!(f.memory.write_snapshot(f.base.owner).await.unwrap().privacy_epoch,epoch);
            assert_eq!(f.base.records().await,records);
            f.base.db.execute_unprepared("DROP TRIGGER fail_erasure_after_vectors ON memory_items").await.unwrap();
            assert_eq!(f.forget(&m.unrelated).await.unwrap(),3);
            f.assert_erased(&m.dependent).await;
            f.assert_erased(&m.child).await;
            f.assert_independent(&m).await;
        }).await;
    }

    #[tokio::test]
    async fn erasure_preserves_records_and_projections_of_another_tenant() {
        with_erasure(|f|async move {
            let m=f.memories().await;
            let foreign=Uuid::new_v4();let tenant=Uuid::new_v4();let graph_node=Uuid::new_v4();
            f.base.db.execute(sql(
                "INSERT INTO memory_items(id,tenant_id,user_id,kind,scope,memory_layer,node_type,content,source_event_ids) SELECT $1,$2,user_id,kind,scope,memory_layer,node_type,content,source_event_ids FROM memory_items WHERE id=$3::uuid",
                vec![foreign.into(),tenant.into(),m.kept.clone().into()],
            )).await.unwrap();
            f.base.db.execute(sql(
                "INSERT INTO memory_embeddings(memory_id,profile_id,source_revision,input_digest,policy_version,privacy_epoch,dimensions,embedding) SELECT $1,profile_id,source_revision,input_digest,policy_version,privacy_epoch,dimensions,embedding FROM memory_embeddings WHERE memory_id=$2::uuid",
                vec![foreign.into(),m.kept.clone().into()],
            )).await.unwrap();
            f.base.db.execute(sql(
                "INSERT INTO memory_message_sources(memory_id,source_event_id,owner_id,source_privacy_epoch,content_digest) SELECT $1,source_event_id,owner_id,source_privacy_epoch,content_digest FROM memory_message_sources WHERE memory_id=$2::uuid",
                vec![foreign.into(),m.kept.clone().into()],
            )).await.unwrap();
            f.base.db.execute(sql(
                "INSERT INTO memory_nodes(id,tenant_id,user_id,node_type,node_key,label,properties) VALUES($1,$2,$3,'memory',$4,'Foreign tenant ordinary fact',$5)",
                vec![graph_node.into(),tenant.into(),f.base.owner.into(),format!("memory:{foreign}").into(),json!({"memory_id":foreign}).into()],
            )).await.unwrap();
            // The foreign orphan node also checks the eraser's orphan cleanup
            // predicate, which must include tenant as well as owner.
            assert_eq!(f.forget(&m.unrelated).await.unwrap(),3);
            f.assert_erased(&m.dependent).await;
            f.assert_erased(&m.child).await;
            let row=f.base.db.query_one(sql("SELECT status,content FROM memory_items WHERE id=$1 AND tenant_id=$2",vec![foreign.into(),tenant.into()])).await.unwrap().unwrap();
            assert_eq!(row.try_get::<String>("","status").unwrap(),"active");
            assert!(!row.try_get::<String>("","content").unwrap().is_empty());
            assert_eq!(f.artifact_count("memory_embeddings",&foreign.to_string()).await,1);
            assert_eq!(f.artifact_count("memory_message_sources",&foreign.to_string()).await,1);
            assert_eq!(f.graph_count(&foreign.to_string()).await,1);
            f.assert_independent(&m).await;
        }).await;
    }

    #[tokio::test]
    async fn migration_erases_old_epoch_and_malformed_origins_but_keeps_current_appraisals() {
        for old_epoch in [json!("malformed original epoch"), json!(null)] {
            with_erasure_at(true,move |f|async move {
                let old=f.memories().await;
                f.base.db.execute(sql("UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1 WHERE user_id=$1",vec![f.base.owner.into()])).await.unwrap();
                assert!(f.memory.get(f.base.owner,&old.dependent).await.unwrap().is_some(),
                    "the real pre-12 trigger must reproduce a leftover canonical copy");
                let current=f.memories().await;
                assert_eq!(f.artifact_count("memory_appraisal_sources",&current.dependent).await,1);
                f.base.db.execute(sql(
                    "UPDATE memory_appraisal_sources SET source=jsonb_set(source,'{source_privacy_epoch}',$2) WHERE memory_id=$1::uuid",
                    vec![old.dependent.clone().into(),old_epoch.into()],
                )).await.unwrap();
                assert!(migrations::run(&f.base.db).await.unwrap().applied>=1);
                f.assert_erased(&old.dependent).await;
                f.assert_erased(&old.child).await;
                assert!(f.memory.get(f.base.owner,&current.dependent).await.unwrap().is_some());
                assert!(f.memory.get(f.base.owner,&current.child).await.unwrap().is_some());
                assert_eq!(f.artifact_count("memory_embeddings",&current.dependent).await,1);
                assert_eq!(f.artifact_count("memory_appraisal_sources",&current.dependent).await,1);
                f.assert_independent(&old).await;
                f.assert_independent(&current).await;
                assert_eq!(migrations::run(&f.base.db).await.unwrap().applied,0);
            }).await;
        }
    }

    #[tokio::test]
    async fn policy_revoked_appraisal_graph_does_not_poison_a_fresh_valid_context() {
        with_erasure(|f| async move {
            let m = f.memories().await;
            let store = ContextPolicyStore::new(f.base.db.clone());
            let original = store.current().await.unwrap();
            let mut policy = original.policy;
            assert_eq!(
                policy.graph_depth, 2,
                "exercise the real default graph traversal depth"
            );
            policy.appraisal.affect.baseline_trust = 0.42;
            store.replace(original.version, policy).await.unwrap();
            assert!(
                f.memory
                    .get(f.base.owner, &m.dependent)
                    .await
                    .unwrap()
                    .is_some(),
                "policy revocation must be the gate here; privacy has not erased the row"
            );
            let source = f.base.source("Cedar tea").await;
            let fresh = f.base.service().materialize(source, "Cedar tea").await;
            assert!(
                fresh.is_ok(),
                "revoked Appraisal graph must not make a valid new source unavailable: {:?}",
                fresh.err()
            );
            let fresh = fresh.unwrap();
            assert!(fresh.lease().dependencies().iter().any(
                |root| matches!(root,ContextEvidence::Memory{id,..} if id.to_string()==m.input)
            ));
            assert!(fresh.lease().dependencies().iter().all(|root| match root {
                ContextEvidence::Memory { id, .. } =>
                    id.to_string() != m.dependent && id.to_string() != m.child,
                ContextEvidence::Appraisal { .. } | ContextEvidence::AffectProjection { .. } =>
                    false,
                _ => true,
            }));
            let material = serde_json::to_string(&fresh.all_materialized_memory()).unwrap();
            assert!(!material.contains("Relational Cedar") && !material.contains("Further Cedar"));
        })
        .await;
    }
}
