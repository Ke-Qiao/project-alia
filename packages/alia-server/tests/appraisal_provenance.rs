//! Real PostgreSQL ownership, revocation and projection behavior.
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
    async fn new() -> Self {
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
        if let Err(error) = migrations::run(&f.db).await {
            f.cleanup().await;
            panic!("migration: {error:#}");
        }
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
        f
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
async fn same_identity_different_draft_and_unbound_legacy_calls_do_not_write() {
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
        let old = runtime
            .observe_user_input(alia_server::expression::AffectUserInput {
                channel_id: "appraisal-device",
                user_id: &f.owner.to_string(),
                user_text: "thank you",
            })
            .await;
        assert!(old.persistence_error.is_some());
        assert!(old.appraisals.is_empty());
        assert_eq!(f.records().await, 1);
        runtime.shutdown().await.unwrap();
    })
    .await;
}

async fn memory(f: &Fixture) -> alia_server::providers::memory::AliaMemory {
    let memory = alia_server::providers::memory::AliaMemory::new();
    memory
        .attach_runtime_for_test(f.db.clone(), None, MemorySection::default())
        .await
        .unwrap();
    memory
}
fn node(content: &str) -> alia_server::providers::adapter::MemoryNode {
    alia_server::providers::adapter::MemoryNode::new("preference", "preference", content, 0.9)
        .with_runtime_fields("preference", "user", 0.9, 0.9, "internal")
}

#[tokio::test]
async fn relationship_clear_is_idempotent_and_erases_only_its_owner_derived_memories() {
    use alia_server::providers::adapter::{MemoryBackend, MemoryWriteOrigin, RetrievalOpts};
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
    use alia_server::providers::adapter::{MemoryBackend, MemoryPatch, MemoryWriteOrigin};
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
    use alia_server::providers::adapter::{
        MemoryBackend, MemoryListOpts, MemoryPatch, RetrievalOpts,
    };
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
    use alia_server::providers::adapter::{MemoryBackend, RetrievalOpts};
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
async fn legacy_policy_shape_is_verified_and_upgraded_once_without_overwriting_db_settings() {
    with_db(|f| async move {
        let service = f.service();
        let stored = service.policy_store().current().await.unwrap();
        let mut old = serde_json::to_value(&stored.policy).unwrap();
        old.as_object_mut().unwrap().remove("appraisal");
        let digest = evidence_digest(&old).unwrap();
        f.db.execute(sql(
            "UPDATE context_policy SET policy=$1,policy_digest=$2 WHERE singleton",
            vec![old.clone().into(), digest.into()],
        ))
        .await
        .unwrap();
        let mut seed = stored.policy.clone();
        seed.max_memory_results = 1;
        seed.appraisal.affect.baseline_trust = 0.37;
        let upgraded = service.initialize_policy(seed.clone()).await.unwrap();
        assert_eq!(upgraded.version, stored.version + 1);
        assert_eq!(
            upgraded.policy.max_memory_results,
            stored.policy.max_memory_results
        );
        assert_eq!(upgraded.policy.appraisal.affect.baseline_trust, 0.37);
        seed.appraisal.affect.baseline_trust = 0.8;
        let restarted = service.initialize_policy(seed).await.unwrap();
        assert_eq!(restarted.version, upgraded.version);
        assert_eq!(restarted.policy, upgraded.policy);
        f.db.execute(sql(
            "UPDATE context_policy SET policy=$1,policy_digest=repeat('0',64) WHERE singleton",
            vec![old.into()],
        ))
        .await
        .unwrap();
        assert!(
            service.initialize_policy(stored.policy).await.is_err(),
            "damaged old digest must never be refreshed"
        );
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
