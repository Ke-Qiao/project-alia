//! Real PostgreSQL source, erasure, atomic receipt and restart regressions.
use super::*;
use crate::{
    config::ServerConfig,
    expression::{ExpressionRuntime, rule_based_user_appraisals},
    modules::{
        memory::{
            context::{ContextPolicy, ContextService, ContextSource},
            source::capture_source_epochs,
        },
        platform::migrations,
    },
    providers::embedding::NoopEmbedding,
};
use futures::FutureExt;
use sea_orm::{ConnectOptions, Database};
use std::{future::Future, panic::AssertUnwindSafe, path::Path, sync::Arc, time::Duration};

#[derive(Clone)]
struct Fixture {
    db: DatabaseConnection,
    admin: DatabaseConnection,
    schema: String,
    owner: Uuid,
    session: Uuid,
    runtime: Arc<ExpressionRuntime>,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("Companion tests require disposable real PostgreSQL");
        let mut opts = ConnectOptions::new(url.clone());
        opts.max_connections(1).sqlx_logging(false);
        let admin = Database::connect(opts).await.unwrap();
        let schema = format!("companion_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut opts = ConnectOptions::new(url);
        opts.max_connections(6)
            .sqlx_logging(false)
            .set_schema_search_path(format!("{schema},public"));
        let db = match Database::connect(opts).await {
            Ok(db) => db,
            Err(error) => {
                admin
                    .execute_unprepared(&format!("DROP SCHEMA {schema} CASCADE"))
                    .await
                    .unwrap();
                admin.close().await.unwrap();
                panic!("Companion schema connection failed: {error}");
            }
        };
        let config = ServerConfig::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/server.example.toml"),
        )
        .unwrap();
        let f = Self {
            db,
            admin,
            schema,
            owner: Uuid::new_v4(),
            session: Uuid::new_v4(),
            runtime: Arc::new(ExpressionRuntime::from_config(&config)),
        };
        let setup = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(45), async {
            migrations::run(&f.db).await.unwrap();
            f.add_owner(f.owner, f.session).await;
            f.context()
                .initialize_policy(ContextPolicy::from_server_config(&config).unwrap())
                .await
                .unwrap();
            f.runtime.attach_db(f.db.clone()).await.unwrap();
        }))
        .catch_unwind()
        .await;
        match setup {
            Ok(Ok(())) => f,
            result => {
                f.cleanup().await;
                match result {
                    Err(error) => std::panic::resume_unwind(error),
                    _ => panic!("Companion setup timed out"),
                }
            }
        }
    }
    fn repo(&self) -> CompanionRepository {
        CompanionRepository::new(self.db.clone())
    }
    fn context(&self) -> ContextService {
        ContextService::new(self.db.clone(), Arc::new(NoopEmbedding))
    }
    async fn add_owner(&self, owner: Uuid, session: Uuid) {
        self.db.execute(sql("INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Companion fixture','not-a-login','user','active')", vec![owner.into(), format!("{owner}@companion.test").into()])).await.unwrap();
        self.db.execute(sql("INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'companion-device',$2)", vec![session.into(), owner.to_string().into()])).await.unwrap();
    }
    async fn seed(&self) -> (Uuid, Uuid) {
        self.seed_for(self.owner, self.session).await
    }
    async fn seed_for(&self, owner: Uuid, session: Uuid) -> (Uuid, Uuid) {
        let text = "thank you PRIVATE_COMPANION_FIXTURE";
        let stamps = capture_source_epochs(&self.db, owner).await.unwrap();
        let source = Uuid::new_v4();
        let row = self.db.query_one(sql("INSERT INTO chat_messages(id,session_id,user_id,role,content,created_at,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,clock_timestamp(),clock_timestamp(),$5,$6) RETURNING source_received_at", vec![source.into(), session.into(), owner.to_string().into(), text.into(), stamps.privacy_epoch.into(), stamps.relationship_epoch.into()])).await.unwrap().unwrap();
        let generation = self
            .context()
            .materialize(
                ContextSource {
                    owner_id: owner,
                    request_id: Uuid::new_v4(),
                    channel_id: "companion-device".into(),
                    conversation_id: Some(session),
                    source_event_id: Some(source),
                    source_received_at: row.try_get("", "source_received_at").unwrap(),
                    source_privacy_epoch: stamps.privacy_epoch,
                },
                text,
            )
            .await
            .unwrap();
        let input = self
            .context()
            .prepare_user_appraisal(&generation)
            .await
            .unwrap();
        let drafts =
            rule_based_user_appraisals(input.text(), input.effective_at(), &input.policy().affect);
        let receipt = self.runtime.submit_appraisal(input, drafts).await.unwrap();
        (receipt.appraisal_id, source)
    }
    async fn count(&self, table: &str) -> i64 {
        self.db
            .query_one(sql(
                &format!("SELECT COUNT(*)::bigint AS n FROM {table}"),
                vec![],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }
    async fn assert_erased(&self, operation: Uuid) {
        let row = self.db.query_one(sql("SELECT state,private_snapshot,snapshot_digest FROM companion_operations WHERE operation_id=$1", vec![operation.into()])).await.unwrap().unwrap();
        assert_eq!(row.try_get::<String>("", "state").unwrap(), "erased");
        assert!(
            row.try_get::<Option<serde_json::Value>>("", "private_snapshot")
                .unwrap()
                .is_none()
        );
        assert!(
            row.try_get::<Option<String>>("", "snapshot_digest")
                .unwrap()
                .is_none()
        );
        let row = self.db.query_one(sql("SELECT COUNT(*)::bigint AS n FROM companion_candidates WHERE operation_id=$1 AND private_candidate IS NOT NULL", vec![operation.into()])).await.unwrap().unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
        let row = self
            .db
            .query_one(sql(
                "SELECT COUNT(*)::bigint AS n FROM companion_sources WHERE operation_id=$1",
                vec![operation.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
        assert!(matches!(
            self.repo().replay(self.owner, operation).await,
            Err(RepoError::Erased)
        ));
    }
    async fn cleanup(self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            self.runtime.shutdown().await.unwrap();
            self.db.close().await.unwrap();
            self.admin
                .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .await
                .unwrap();
            self.admin.close().await.unwrap();
        })
        .await
        .expect("Companion cleanup exceeded its budget");
    }
}
async fn with_fixture<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let f = Fixture::new().await;
    let outcome = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(45),
        test(f.clone()),
    ))
    .catch_unwind()
    .await;
    f.cleanup().await;
    match outcome {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("Companion test timed out"),
        Err(error) => std::panic::resume_unwind(error),
    }
}

#[tokio::test]
async fn fixed_shadow_receipt_survives_competing_workers_restart_and_duplicate_trigger() {
    with_fixture(|f| async move {
        let (appraisal, _) = f.seed().await;
        let repo = f.repo();
        let pending = repo.discovery_pending(32).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].channel_id, "companion-device");
        let operation = Uuid::new_v4();
        let other_repo = f.repo();
        let (left, right) = tokio::join!(
            repo.evaluate(f.owner, operation, vec![appraisal]),
            other_repo.evaluate(f.owner, operation, vec![appraisal])
        );
        let report = left.unwrap();
        assert_eq!(json!(report), json!(right.unwrap()));
        assert_eq!(
            json!(report),
            json!(f.repo().replay(f.owner, operation).await.unwrap())
        );
        assert_eq!(report.examined, 1);
        assert_eq!(
            report.decisions[0].decision.outcome,
            DecisionOutcome::Suppressed
        );
        assert!(
            report.decisions[0]
                .decision
                .reasons
                .contains(&DecisionReason::OpportunityUnavailable)
        );
        assert_eq!(
            report.decisions[0].input.candidate.created_at,
            report.decisions[0].input.candidate.observation.effective_at
        );
        assert!(
            !json!(report)
                .to_string()
                .contains("PRIVATE_COMPANION_FIXTURE")
        );
        let duplicate = f
            .repo()
            .evaluate(f.owner, Uuid::new_v4(), vec![appraisal])
            .await
            .unwrap();
        assert!(
            duplicate.decisions[0]
                .decision
                .reasons
                .contains(&DecisionReason::Duplicate)
        );
        assert_eq!(f.count("companion_candidates").await, 1);
        assert!(f.repo().discovery_pending(32).await.unwrap().is_empty());
        for table in [
            "delivery_responses",
            "deliveries",
            "execution_effects",
            "execution_outbox",
            "affect_projections",
            "memory_items",
        ] {
            assert_eq!(f.count(table).await, 0, "shadow mutated {table}");
        }
        assert_eq!(f.count("appraisal_records").await, 1);
        assert!(matches!(
            repo.evaluate(f.owner, operation, vec![appraisal, appraisal])
                .await,
            Err(RepoError::InvalidInput(_))
        ));
        let (other, _) = f.seed().await;
        assert!(matches!(
            repo.evaluate(f.owner, operation, vec![other]).await,
            Err(RepoError::Conflict)
        ));
    })
    .await;
}

#[tokio::test]
async fn receipt_and_candidate_failure_rolls_back_the_entire_admission() {
    with_fixture(|f| async move {
        let (appraisal, _) = f.seed().await;
        let operation = Uuid::new_v4();
        f.db.execute_unprepared("CREATE FUNCTION fail_companion_candidate() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NOT EXISTS(SELECT 1 FROM companion_operations WHERE operation_id=NEW.operation_id) THEN RAISE EXCEPTION 'fault did not reach operation insert'; END IF; RAISE EXCEPTION 'injected Companion candidate failure'; END $$; CREATE TRIGGER fail_companion_candidate BEFORE INSERT ON companion_candidates FOR EACH ROW EXECUTE FUNCTION fail_companion_candidate()").await.unwrap();
        assert!(matches!(f.repo().evaluate(f.owner, operation, vec![appraisal]).await, Err(RepoError::Unavailable(_))));
        for table in ["companion_operations", "companion_candidates", "companion_sources", "companion_processed_sources"] { assert_eq!(f.count(table).await, 0); }
        f.db.execute_unprepared("DROP TRIGGER fail_companion_candidate ON companion_candidates; DROP FUNCTION fail_companion_candidate()").await.unwrap();
        let report = f.repo().evaluate(f.owner, operation, vec![appraisal]).await.unwrap();
        assert_eq!(report.decisions.len(), 1);
        assert_eq!(f.count("companion_candidates").await, 1);
    }).await;
}

#[tokio::test]
async fn source_revision_and_relationship_clear_erase_private_replay_but_keep_tombstones() {
    with_fixture(|f| async move {
        let (appraisal, source) = f.seed().await;
        let operation = Uuid::new_v4();
        f.repo()
            .evaluate(f.owner, operation, vec![appraisal])
            .await
            .unwrap();
        f.db.execute(sql(
            "UPDATE chat_messages SET content='changed canonical source' WHERE id=$1",
            vec![source.into()],
        ))
        .await
        .unwrap();
        f.assert_erased(operation).await;
        assert!(f.repo().discovery_pending(32).await.unwrap().is_empty());
        assert_eq!(f.count("companion_processed_sources").await, 1);
        assert!(matches!(
            f.repo().evaluate(f.owner, operation, vec![appraisal]).await,
            Err(RepoError::Erased)
        ));
        let (next, _) = f.seed().await;
        let next_operation = Uuid::new_v4();
        f.repo()
            .evaluate(f.owner, next_operation, vec![next])
            .await
            .unwrap();
        f.context()
            .clear_relationship(f.owner, Uuid::new_v4(), 0)
            .await
            .unwrap();
        f.assert_erased(next_operation).await;
        assert_eq!(f.count("companion_processed_sources").await, 2);
        assert!(f.repo().discovery_pending(32).await.unwrap().is_empty());
    })
    .await;
}

#[tokio::test]
async fn privacy_erasure_is_owner_scoped_and_policy_cas_cannot_revive_a_shadow_candidate() {
    with_fixture(|f| async move {
        let (appraisal, _) = f.seed().await;
        let other_owner = Uuid::new_v4();
        let other_session = Uuid::new_v4();
        f.add_owner(other_owner, other_session).await;
        let (other_appraisal, _) = f.seed_for(other_owner, other_session).await;
        let operation = Uuid::new_v4();
        let other_operation = Uuid::new_v4();
        f.repo()
            .evaluate(f.owner, operation, vec![appraisal])
            .await
            .unwrap();
        let other = f
            .repo()
            .evaluate(other_owner, other_operation, vec![other_appraisal])
            .await
            .unwrap();
        assert!(matches!(
            f.repo().replay(other_owner, operation).await,
            Err(RepoError::Forbidden)
        ));
        f.db.execute(sql(
            "UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1 WHERE user_id=$1",
            vec![f.owner.into()],
        ))
        .await
        .unwrap();
        f.assert_erased(operation).await;
        assert_eq!(
            json!(other),
            json!(f.repo().replay(other_owner, other_operation).await.unwrap())
        );
        let before = f.repo().policy(other_owner).await.unwrap();
        let mut policy = before.policy.clone();
        policy.quiet = true;
        let changed = f
            .repo()
            .replace_policy(other_owner, before.revision, policy.clone())
            .await
            .unwrap();
        assert_eq!(changed.revision, before.revision + 1);
        assert!(matches!(
            f.repo()
                .replace_policy(other_owner, before.revision, policy)
                .await,
            Err(RepoError::Conflict)
        ));
        assert_eq!(f.repo().policy(other_owner).await.unwrap(), changed);
        assert!(matches!(
            f.repo().replay(other_owner, other_operation).await,
            Err(RepoError::Erased)
        ));
        let mut enabled = changed.policy.clone();
        enabled.mode = CompanionMode::Enabled;
        assert!(matches!(
            f.repo()
                .replace_policy(other_owner, changed.revision, enabled)
                .await,
            Err(RepoError::Unavailable(_))
        ));
        assert_eq!(f.repo().policy(other_owner).await.unwrap(), changed);
    })
    .await;
}

#[tokio::test]
async fn bounded_page_and_explicit_simulated_opportunity_never_acquire_output_authority() {
    with_fixture(|f| async move {
        let mut ids = Vec::new();
        for _ in 0..5 {
            ids.push(f.seed().await.0);
        }
        let tx = f.db.begin().await.unwrap();
        let authority = lock_companion_authority(&tx, f.owner).await.unwrap();
        tx.commit().await.unwrap();
        let report = f
            .repo()
            .evaluate_with_opportunity(
                f.owner,
                Uuid::new_v4(),
                ids.clone(),
                Some(OpportunitySnapshot::simulation(authority.as_of)),
            )
            .await
            .unwrap();
        assert_eq!(
            (report.examined, report.limited, report.decisions.len()),
            (5, 1, 4)
        );
        assert!(
            report
                .decisions
                .iter()
                .all(|d| d.decision.outcome == DecisionOutcome::ShadowWouldAllow)
        );
        assert!(report.decisions.iter().all(|d| {
            d.decision
                .reasons
                .contains(&DecisionReason::SimulatedOpportunity)
        }));
        assert!(f.repo().discovery_pending(32).await.unwrap().is_empty());
        assert!(matches!(
            f.repo().discovery_pending(33).await,
            Err(RepoError::InvalidInput(_))
        ));
        assert!(matches!(
            f.repo()
                .evaluate(f.owner, Uuid::new_v4(), vec![Uuid::new_v4(); 33])
                .await,
            Err(RepoError::InvalidInput(_))
        ));
        let row =
            f.db.query_one(sql(
                "SELECT count(*)::bigint AS n FROM context_leases",
                vec![],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.try_get::<i64>("", "n").unwrap(),
            5,
            "shadow must issue no context lease"
        );
        assert_eq!(f.count("appraisal_records").await, 5);
        assert_eq!(f.count("delivery_responses").await, 0);
    })
    .await;
}

#[tokio::test]
async fn absent_opportunity_uses_database_time_while_future_observations_are_rejected() {
    with_fixture(|f| async move {
        let (appraisal, _) = f.seed().await;
        let tx = f.db.begin().await.unwrap();
        let authority = lock_companion_authority(&tx, f.owner).await.unwrap();
        tx.commit().await.unwrap();
        let future = authority.as_of + chrono::Duration::hours(1);
        let simulated = OpportunitySnapshot::simulation(future);
        let mut verified = simulated.clone();
        verified.source = OpportunitySource::VerifiedLive {
            server_instance_id: Uuid::new_v4(),
            device_boot_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            generation_epoch: 1,
        };
        verified.capture_revision = Some(1);
        verified.last_capture_at = Some(future);
        for opportunity in [simulated, verified] {
            opportunity.validate().unwrap();
            assert!(matches!(
                f.repo()
                    .evaluate_with_opportunity(
                        f.owner,
                        Uuid::new_v4(),
                        vec![appraisal],
                        Some(opportunity),
                    )
                    .await,
                Err(RepoError::Unavailable(message))
                    if message.contains("inconsistent frozen times")
            ));
        }
        for table in [
            "companion_operations",
            "companion_candidates",
            "companion_processed_sources",
            "companion_rejected_sources",
        ] {
            assert_eq!(f.count(table).await, 0, "future input wrote {table}");
        }

        let operation = Uuid::new_v4();
        let report = f
            .repo()
            .evaluate_with_opportunity(f.owner, operation, vec![appraisal], None)
            .await
            .unwrap();
        let input = &report.decisions[0].input;
        assert_eq!(
            input.opportunity,
            OpportunitySnapshot::unavailable(report.as_of)
        );
        assert_eq!(input.as_of, report.as_of);
        assert_eq!(
            report.decisions[0].decision.outcome,
            DecisionOutcome::Suppressed
        );
        assert_eq!(
            json!(report),
            json!(f.repo().replay(f.owner, operation).await.unwrap())
        );
    })
    .await;
}

#[tokio::test]
async fn stale_sources_are_retired_without_starving_discovery_or_refreshing_original_time() {
    with_fixture(|f| async move {
        let (stale, message) = f.seed().await;
        let (valid, _) = f.seed().await;
        f.db.execute(sql("UPDATE chat_messages SET content='source revision changed' WHERE id=$1", vec![message.into()])).await.unwrap();
        assert_eq!(f.repo().discovery_pending(1).await.unwrap()[0].appraisal_id, stale);
        assert!(matches!(f.repo().evaluate(f.owner, Uuid::new_v4(), vec![stale]).await, Err(RepoError::Erased)));
        assert_eq!(f.count("companion_rejected_sources").await, 1);
        assert_eq!(f.repo().discovery_pending(1).await.unwrap()[0].appraisal_id, valid);
        assert_eq!(f.count("companion_operations").await, 0);
        f.db.execute(sql("UPDATE appraisal_records SET effective_at=effective_at+INTERVAL '1 microsecond' WHERE appraisal_id=$1", vec![valid.into()])).await.unwrap();
        assert!(matches!(f.repo().evaluate(f.owner, Uuid::new_v4(), vec![valid]).await, Err(RepoError::Erased)));
        assert!(f.repo().discovery_pending(1).await.unwrap().is_empty());
        assert_eq!(f.count("companion_rejected_sources").await, 2);
        assert_eq!(f.count("companion_candidates").await, 0);
    }).await;
}

#[tokio::test]
async fn private_retention_and_capacity_are_bounded_while_erasure_keeps_identity() {
    with_fixture(|f| async move {
        let (appraisal, _) = f.seed().await;
        let policy = f.repo().policy(f.owner).await.unwrap();
        // Reserved private slots test the SQL capacity boundary independently
        // of source interpretation. They never participate in replay/admission.
        f.db.execute(sql("INSERT INTO companion_operations(operation_id,owner_id,request_digest,policy_revision,context_policy_version,privacy_epoch,relationship_epoch,as_of,state,private_snapshot,snapshot_digest,private_until) SELECT gen_random_uuid(),$1,repeat('0',64),$2,1,0,0,clock_timestamp(),'completed','{}'::jsonb,repeat('0',64),clock_timestamp()+INTERVAL '1 hour' FROM generate_series(1,128)", vec![f.owner.into(), policy.revision.into()])).await.unwrap();
        assert!(matches!(f.repo().evaluate(f.owner, Uuid::new_v4(), vec![appraisal]).await, Err(RepoError::Unavailable(message)) if message.contains("capacity")));
        assert_eq!(f.count("companion_candidates").await, 0);
        let expired = Uuid::new_v4();
        f.db.execute(sql("INSERT INTO companion_operations(operation_id,owner_id,request_digest,policy_revision,context_policy_version,privacy_epoch,relationship_epoch,as_of,state,private_snapshot,snapshot_digest,private_until) VALUES($1,$2,repeat('0',64),$3,1,0,0,clock_timestamp(),'completed','{}'::jsonb,repeat('0',64),clock_timestamp()-INTERVAL '1 second')", vec![expired.into(), f.owner.into(), policy.revision.into()])).await.unwrap();
        assert_eq!(f.repo().prune_expired(1).await.unwrap(), 1);
        f.assert_erased(expired).await;
        assert_eq!(f.repo().prune_expired(128).await.unwrap(), 0);
        assert_eq!(f.count("companion_operations").await, 129, "erasure retains opaque operation identities");
        assert!(matches!(f.repo().prune_expired(129).await, Err(RepoError::InvalidInput(_))));
    }).await;
}

#[tokio::test]
async fn source_erasure_triggers_ignore_cache_hits_but_cover_graph_node_projections() {
    with_fixture(|f| async move {
        let (appraisal, _) = f.seed().await;
        let operation = Uuid::new_v4();
        let report = f.repo().evaluate(f.owner, operation, vec![appraisal]).await.unwrap();
        let source = &report.decisions[0].input.candidate.observation;
        let response = Uuid::new_v4(); let cache = Uuid::new_v4();
        // These extra opaque pins isolate the SQL trigger contract. Canonical
        // closure construction is tested by normal admission/source revocation.
        f.db.execute(sql("INSERT INTO delivery_responses(response_id,user_id,request_id,output_key,source_received_at,channel_id,kind,content,intent_digest,context_id) VALUES($1,$2,$3,'trigger-fixture',$4,'companion-device','text','{}',repeat('0',64),$5)", vec![response.into(), f.owner.into(), Uuid::new_v4().into(), source.source_received_at.into(), source.context_id.into()])).await.unwrap();
        f.db.execute(sql("INSERT INTO llm_cache(id,user_id,scope,query_hash,query_text,response_text,expires_at,revision,context_id,origin_response_id,status,content_digest) VALUES($1,$2,'fixture',repeat('0',64),'query','response',clock_timestamp()+INTERVAL '1 hour',1,$3,$4,'active',repeat('0',64))", vec![cache.into(), f.owner.into(), source.context_id.into(), response.into()])).await.unwrap();
        f.db.execute(sql("INSERT INTO companion_sources(operation_id,kind,source_id) VALUES($1,'cache',$2)", vec![operation.into(), cache.into()])).await.unwrap();
        f.db.execute(sql("UPDATE llm_cache SET hit_count=hit_count+1 WHERE id=$1", vec![cache.into()])).await.unwrap();
        assert_eq!(json!(report), json!(f.repo().replay(f.owner, operation).await.unwrap()));
        f.db.execute(sql("UPDATE llm_cache SET response_text=response_text WHERE id=$1", vec![cache.into()])).await.unwrap();
        assert_eq!(json!(report), json!(f.repo().replay(f.owner, operation).await.unwrap()));
        f.db.execute(sql("UPDATE llm_cache SET response_text='changed response',revision=revision+1 WHERE id=$1", vec![cache.into()])).await.unwrap();
        f.assert_erased(operation).await;
        let (next, _) = f.seed().await;
        let next_operation = Uuid::new_v4();
        f.repo().evaluate(f.owner, next_operation, vec![next]).await.unwrap();
        let src = Uuid::new_v4(); let dst = Uuid::new_v4(); let edge = Uuid::new_v4();
        for node in [src,dst] {
            f.db.execute(sql("INSERT INTO memory_nodes(id,tenant_id,user_id,node_type,node_key,label) VALUES($1,$2,$3,'topic',$4,'original label')", vec![node.into(), Uuid::nil().into(), f.owner.into(), node.to_string().into()])).await.unwrap();
        }
        f.db.execute(sql("INSERT INTO memory_edges(id,tenant_id,user_id,src_node_id,dst_node_id,relation) VALUES($1,$2,$3,$4,$5,'RELATED_TO')", vec![edge.into(), Uuid::nil().into(), f.owner.into(), src.into(), dst.into()])).await.unwrap();
        f.db.execute(sql("INSERT INTO companion_sources(operation_id,kind,source_id) VALUES($1,'graph',$2)", vec![next_operation.into(), edge.into()])).await.unwrap();
        f.db.execute(sql("UPDATE memory_nodes SET label='changed label' WHERE id=$1", vec![src.into()])).await.unwrap();
        f.assert_erased(next_operation).await;
    }).await;
}
