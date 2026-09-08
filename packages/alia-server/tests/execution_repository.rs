//! Real PostgreSQL execution tests. Each test owns a disposable schema.
//! Missing ALIA_TEST_DATABASE_URL or unavailable infrastructure is a failure.

use std::{future::Future, panic::AssertUnwindSafe, time::Duration};

use alia_server::modules::execution::{
    repository::ExecutionRepository,
    types::{
        Approval, ApprovalKind, ClaimRequest, Completion, EffectKey, EffectRecord, EffectStatus,
        ExecutionError, MAX_PENDING, MAX_PROPOSED_PER_USER, NewEffect,
    },
};
use alia_server::modules::platform::migrations;
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use uuid::Uuid;

const TOOL_NAME: &str = "test.execution_effect";
const TOOL_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const POLICY_DIGEST: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
// Keep the suite within a small disposable database's connection budget while
// individual tests still run concurrent repository operations on separate connections.
static DATABASE_CASES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

fn effect(user_id: Uuid) -> NewEffect {
    NewEffect {
        key: EffectKey {
            user_id,
            request_id: Uuid::new_v4(),
            invocation_key: "call_0".to_string(),
        },
        model_call_id: "model_call_0".to_string(),
        source_received_at: chrono::Utc::now(),
        source_event_id: None,
        source_privacy_epoch: None,
        source: "integration_test".to_string(),
        conversation_id: None,
        channel_id: None,
        tool_name: TOOL_NAME.to_string(),
        arguments: serde_json::json!({"value": "first"}),
        tool_digest: TOOL_DIGEST.to_string(),
        policy_digest: POLICY_DIGEST.to_string(),
        requires_confirmation: true,
        required_role: None,
    }
}

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
        let mut admin_options = ConnectOptions::new(url.clone());
        admin_options
            .max_connections(1)
            .connect_timeout(Duration::from_secs(5))
            .sqlx_logging(false);
        let admin = Database::connect(admin_options)
            .await
            .expect("execution test PostgreSQL must be reachable");
        assert_eq!(admin.get_database_backend(), DatabaseBackend::Postgres);
        let schema = format!("execution_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .expect("create isolated execution test schema");
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(4)
            .min_connections(1)
            .connect_timeout(Duration::from_secs(5))
            .acquire_timeout(Duration::from_secs(10))
            .set_schema_search_path(format!("{schema},public"))
            .sqlx_logging(false);
        let db = match Database::connect(options).await {
            Ok(db) => db,
            Err(error) => {
                admin
                    .execute_unprepared(&format!("DROP SCHEMA {schema} CASCADE"))
                    .await
                    .expect("clean up schema after connection failure");
                panic!("connect isolated execution test schema: {error}");
            }
        };
        let database = Self { admin, db, schema };
        if let Err(error) = migrations::run(&database.db).await {
            database.cleanup().await;
            panic!("install real bundled schema for execution test: {error:#}");
        }
        database
    }

    async fn cleanup(self) {
        self.db.close().await.expect("close execution test pool");
        self.admin
            .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .await
            .expect("drop own execution test schema");
        self.admin.close().await.expect("close test admin pool");
    }

    async fn count(&self, table: &str) -> i64 {
        assert!(
            table
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        );
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

    async fn user(&self, role: &str) -> Uuid {
        let user_id = Uuid::new_v4();
        self.db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "INSERT INTO users (id,email,name,password_hash,role,status) VALUES ($1,$2,'Execution test','not-a-login-hash',$3,'active')",
                vec![
                    user_id.into(),
                    format!("{user_id}@execution.test").into(),
                    role.into(),
                ],
            ))
            .await
            .unwrap();
        user_id
    }

    async fn tool(&self, requires_confirmation: bool, required_role: Option<&str>) {
        self.db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "INSERT INTO tools (name,description,parameters,source,require_confirmation,required_role,enabled) VALUES ($1,'Execution test tool',$2,'integration_test',$3,$4,TRUE)",
                vec![
                    TOOL_NAME.into(),
                    serde_json::json!({"type":"object","properties":{"value":{"type":"string"}}}).into(),
                    requires_confirmation.into(),
                    required_role.map(str::to_string).into(),
                ],
            ))
            .await
            .unwrap();
    }

    async fn sql(&self, sql: &str) {
        self.db.execute_unprepared(sql).await.unwrap();
    }

    async fn set_user_status(&self, user_id: Uuid, status: &str) {
        self.db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE users SET status=$2 WHERE id=$1",
                vec![user_id.into(), status.into()],
            ))
            .await
            .unwrap();
    }

    async fn set_deadline_age(&self, effect_id: Uuid, seconds: i32) {
        self.db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE execution_effects SET deadline_at=NOW()-make_interval(secs => $2::integer) WHERE effect_id=$1",
                vec![effect_id.into(), seconds.into()],
            ))
            .await
            .unwrap();
    }
}

fn approval(record: &EffectRecord) -> Approval {
    Approval {
        actor_id: record.key.user_id,
        kind: ApprovalKind::Explicit,
        expected_intent_digest: record.intent_digest.clone(),
    }
}

fn claim_request(record: &EffectRecord) -> ClaimRequest {
    ClaimRequest {
        effect_id: record.effect_id,
        worker_id: Uuid::new_v4(),
        expected_tool_digest: record.tool_digest.clone(),
        expected_policy_digest: record.policy_digest.clone(),
        execution_timeout: Duration::from_secs(60),
    }
}

async fn approve(repository: &ExecutionRepository, record: &EffectRecord) -> EffectRecord {
    repository
        .approve(record.key.user_id, record.effect_id, approval(record))
        .await
        .unwrap()
}

async fn with_database<F, Fut>(test: F)
where
    F: FnOnce(TestDatabase) -> Fut,
    Fut: Future<Output = ()>,
{
    let _database_slot = DATABASE_CASES.acquire().await.unwrap();
    let database = TestDatabase::new().await;
    // Always remove this test's schema, including failed assertions or deadlocks.
    let outcome = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(45),
        test(database.clone()),
    ))
    .catch_unwind()
    .await;
    database.cleanup().await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("execution repository test exceeded 45 seconds"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
async fn concurrent_idempotency_preserves_one_effect_and_rejects_changed_intent() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        let original = effect(user);
        let results =
            futures::future::join_all((0..8).map(|_| repository.propose(original.clone()))).await;
        let records = results.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        assert!(
            records
                .iter()
                .all(|record| record.effect_id == records[0].effect_id)
        );
        assert_eq!(database.count("execution_effects").await, 1);
        assert_eq!(database.count("execution_outbox").await, 0);
        assert_eq!(records[0].status, EffectStatus::Proposed);

        // Provider call IDs only correlate model messages. Retrying the same
        // trusted request/invocation cannot create a second external effect.
        let mut provider_retry = original.clone();
        provider_retry.model_call_id = "different_provider_call_id".into();
        provider_retry.source_received_at += chrono::Duration::minutes(5);
        let retried = repository.propose(provider_retry).await.unwrap();
        assert_eq!(retried.effect_id, records[0].effect_id);
        assert_eq!(retried.model_call_id, original.model_call_id);
        assert_eq!(
            retried.source_received_at.timestamp_micros(),
            original.source_received_at.timestamp_micros()
        );

        let mut changed_epoch = original.clone();
        changed_epoch.source_privacy_epoch = Some(1);
        assert!(matches!(
            repository.propose(changed_epoch).await,
            Err(ExecutionError::IdempotencyConflict)
        ));
        assert_eq!(retried.source_privacy_epoch, original.source_privacy_epoch);

        let mut changed_source = original.clone();
        changed_source.source_event_id = Some(Uuid::new_v4());
        assert!(matches!(
            repository.propose(changed_source).await,
            Err(ExecutionError::IdempotencyConflict)
        ));

        let mut changed = original.clone();
        changed.arguments = serde_json::json!({"value": "different"});
        assert!(matches!(
            repository.propose(changed).await,
            Err(ExecutionError::IdempotencyConflict)
        ));
        let mut changed = original.clone();
        changed.tool_digest = "c".repeat(64);
        assert!(matches!(
            repository.propose(changed).await,
            Err(ExecutionError::IdempotencyConflict)
        ));
        let retained = repository
            .get(user, records[0].effect_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.arguments, original.arguments);

        // Reusing a model call ID in a later request is a distinct effect.
        let next = repository.propose(effect(user)).await.unwrap();
        assert_ne!(next.effect_id, retained.effect_id);
        let other_user = database.user("user").await;
        let mut other = original;
        other.key.user_id = other_user;
        let separate = repository.propose(other).await.unwrap();
        assert_ne!(separate.effect_id, retained.effect_id);
        assert_eq!(database.count("execution_effects").await, 3);
    })
    .await;
}

#[tokio::test]
async fn approval_and_outbox_roll_back_together_on_real_database_error() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(user)).await.unwrap();
        let events_before = database.count("execution_events").await;
        database.sql("CREATE FUNCTION fail_outbox_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected outbox insert failure'; END $$; CREATE TRIGGER fail_outbox_insert BEFORE INSERT ON execution_outbox FOR EACH ROW EXECUTE FUNCTION fail_outbox_insert()").await;
        assert!(matches!(repository.approve(user, record.effect_id, approval(&record)).await, Err(ExecutionError::Database(_))));
        let retained = repository.get(user, record.effect_id).await.unwrap().unwrap();
        assert_eq!(retained.status, EffectStatus::Proposed);
        assert!(retained.approved_at.is_none());
        assert_eq!(database.count("execution_outbox").await, 0);
        assert_eq!(database.count("execution_events").await, events_before);
        database.sql("DROP TRIGGER fail_outbox_insert ON execution_outbox").await;
        let approved = approve(&repository, &record).await;
        assert_eq!(approved.status, EffectStatus::Approved);
        assert_eq!(database.count("execution_outbox").await, 1);
        let repeated = approve(&repository, &record).await;
        assert_eq!(repeated.effect_id, approved.effect_id);
        assert_eq!(database.count("execution_outbox").await, 1);

        // A fresh repository instance discovers committed work without notifications.
        let restarted = ExecutionRepository::new(database.db.clone());
        let pending = restarted.pending(10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].effect_id, record.effect_id);
    }).await;
}

#[tokio::test]
async fn approval_binds_actor_intent_and_confirmation_requirement() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        let other = database.user("admin").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(user)).await.unwrap();
        let mut wrong_actor = approval(&record);
        wrong_actor.actor_id = other;
        assert!(
            repository
                .approve(user, record.effect_id, wrong_actor)
                .await
                .is_err()
        );
        let mut stale_intent = approval(&record);
        stale_intent.expected_intent_digest = "d".repeat(64);
        assert!(
            repository
                .approve(user, record.effect_id, stale_intent)
                .await
                .is_err()
        );
        let mut automatic = approval(&record);
        automatic.kind = ApprovalKind::Automatic;
        assert!(
            repository
                .approve(user, record.effect_id, automatic)
                .await
                .is_err()
        );
        assert_eq!(database.count("execution_outbox").await, 0);
        assert_eq!(
            approve(&repository, &record).await.status,
            EffectStatus::Approved
        );
    })
    .await;
}

#[tokio::test]
async fn automatic_effect_still_requires_a_durable_approval_transaction() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(false, None).await;
        let mut input = effect(user);
        input.requires_confirmation = false;
        let record = repository.propose(input).await.unwrap();
        assert_eq!(record.status, EffectStatus::Proposed);
        assert!(repository.pending(10).await.unwrap().is_empty());
        assert!(
            repository
                .claim(claim_request(&record))
                .await
                .unwrap()
                .is_none()
        );
        let mut automatic = approval(&record);
        automatic.kind = ApprovalKind::Automatic;
        let approved = repository
            .approve(user, record.effect_id, automatic)
            .await
            .unwrap();
        assert_eq!(approved.status, EffectStatus::Approved);
        assert_eq!(approved.approval_kind, Some(ApprovalKind::Automatic));
        assert_eq!(database.count("execution_outbox").await, 1);
    })
    .await;
}

#[tokio::test]
async fn cancellation_and_claim_have_one_winner() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        for _ in 0..12 {
            let record = repository.propose(effect(user)).await.unwrap();
            let approved = approve(&repository, &record).await;
            let (cancelled, claimed) = tokio::join!(
                repository.cancel(user, record.effect_id),
                repository.claim(claim_request(&approved)),
            );
            let claimed = claimed.unwrap();
            let retained = repository
                .get(user, record.effect_id)
                .await
                .unwrap()
                .unwrap();
            match claimed {
                Some(claim) => {
                    assert_eq!(retained.status, EffectStatus::Dispatched);
                    assert!(
                        cancelled.is_err(),
                        "cancel cannot report success after dispatch"
                    );
                    repository
                        .complete(
                            claim.claim_id,
                            Completion::Unknown {
                                reason: "test did not invoke external tool".to_string(),
                            },
                        )
                        .await
                        .unwrap();
                }
                None => {
                    assert_eq!(cancelled.unwrap().status, EffectStatus::Cancelled);
                    assert_eq!(retained.status, EffectStatus::Cancelled);
                }
            }
            assert!(
                repository
                    .claim(claim_request(&approved))
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(database.count("execution_outbox").await, 0);
    })
    .await;
}

#[tokio::test]
async fn concurrent_workers_cannot_claim_the_same_effect_twice() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(user)).await.unwrap();
        let record = approve(&repository, &record).await;
        let claims =
            futures::future::join_all((0..8).map(|_| repository.claim(claim_request(&record))))
                .await;
        let claims = claims
            .into_iter()
            .map(Result::unwrap)
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(claims.len(), 1);
        assert_eq!(database.count("execution_outbox").await, 0);
        let retained = repository
            .get(user, record.effect_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.claim_id, Some(claims[0].claim_id));
        assert_eq!(retained.status, EffectStatus::Dispatched);
    })
    .await;
}

#[tokio::test]
async fn failed_outbox_consumption_rolls_back_the_claim() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(user)).await.unwrap();
        let record = approve(&repository, &record).await;
        let events_before = database.count("execution_events").await;
        database.sql("CREATE FUNCTION fail_outbox_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected outbox delete failure'; END $$; CREATE TRIGGER fail_outbox_delete BEFORE DELETE ON execution_outbox FOR EACH ROW EXECUTE FUNCTION fail_outbox_delete()").await;
        assert!(matches!(repository.claim(claim_request(&record)).await, Err(ExecutionError::Database(_))));
        let retained = repository.get(user, record.effect_id).await.unwrap().unwrap();
        assert_eq!(retained.status, EffectStatus::Approved);
        assert!(retained.claim_id.is_none());
        assert!(retained.worker_id.is_none());
        assert_eq!(database.count("execution_outbox").await, 1);
        assert_eq!(database.count("execution_events").await, events_before);
        database.sql("DROP TRIGGER fail_outbox_delete ON execution_outbox").await;
        assert!(repository.claim(claim_request(&record)).await.unwrap().is_some());
        assert_eq!(database.count("execution_outbox").await, 0);
    }).await;
}

#[tokio::test]
async fn user_revocation_is_checked_at_proposal_approval_and_claim() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        database.set_user_status(user, "disabled").await;
        assert!(repository.propose(effect(user)).await.is_err());
        assert_eq!(database.count("execution_effects").await, 0);
        database.set_user_status(user, "active").await;
        let proposal = repository.propose(effect(user)).await.unwrap();
        database.set_user_status(user, "disabled").await;
        assert!(
            repository
                .approve(user, proposal.effect_id, approval(&proposal))
                .await
                .is_err()
        );
        assert_eq!(database.count("execution_outbox").await, 0);
        database.set_user_status(user, "active").await;
        let dispatched_candidate = repository.propose(effect(user)).await.unwrap();
        let approved = approve(&repository, &dispatched_candidate).await;
        database.set_user_status(user, "disabled").await;
        assert!(
            repository
                .claim(claim_request(&approved))
                .await
                .unwrap()
                .is_none()
        );
        let retained = repository
            .get(user, approved.effect_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.status, EffectStatus::Failed);
        assert_eq!(database.count("execution_outbox").await, 0);
    })
    .await;
}

#[tokio::test]
async fn role_downgrade_before_claim_prevents_dispatch() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("admin").await;
        database.tool(true, Some("admin")).await;
        let mut input = effect(user);
        input.required_role = Some("admin".to_string());
        let record = repository.propose(input).await.unwrap();
        let record = approve(&repository, &record).await;
        database.sql("UPDATE users SET role='user'").await;
        assert!(
            repository
                .claim(claim_request(&record))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            repository
                .get(user, record.effect_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            EffectStatus::Failed
        );
        assert!(repository.pending(10).await.unwrap().is_empty());
    })
    .await;
}

#[tokio::test]
async fn catalog_change_between_proposal_and_approval_is_rejected() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(user)).await.unwrap();
        database.sql("UPDATE tools SET parameters='{\"type\":\"object\",\"required\":[\"new_field\"]}'::jsonb").await;
        assert!(matches!(repository.approve(user, record.effect_id, approval(&record)).await, Err(ExecutionError::VersionChanged)));
        assert_eq!(database.count("execution_outbox").await, 0);
    }).await;
}

#[tokio::test]
async fn changed_catalog_or_worker_policy_consumes_stale_outbox_without_dispatch() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(user)).await.unwrap();
        let record = approve(&repository, &record).await;
        database.sql("UPDATE tools SET enabled=FALSE").await;
        assert!(
            repository
                .claim(claim_request(&record))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            repository
                .get(user, record.effect_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            EffectStatus::Failed
        );
        database.sql("UPDATE tools SET enabled=TRUE").await;
        for change_tool in [true, false] {
            let record = repository.propose(effect(user)).await.unwrap();
            let record = approve(&repository, &record).await;
            let mut request = claim_request(&record);
            if change_tool {
                request.expected_tool_digest = "c".repeat(64);
            } else {
                request.expected_policy_digest = "d".repeat(64);
            }
            assert!(repository.claim(request).await.unwrap().is_none());
            assert_eq!(
                repository
                    .get(user, record.effect_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                EffectStatus::Failed
            );
        }
        assert_eq!(database.count("execution_outbox").await, 0);
    })
    .await;
}

#[tokio::test]
async fn expired_approval_cannot_be_dispatched() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(user)).await.unwrap();
        let record = approve(&repository, &record).await;
        database
            .sql("UPDATE execution_effects SET approval_expires_at=NOW()-INTERVAL '1 second'")
            .await;
        assert!(
            repository
                .claim(claim_request(&record))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            repository
                .get(user, record.effect_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            EffectStatus::Failed
        );
        assert!(repository.pending(10).await.unwrap().is_empty());
    })
    .await;
}

#[tokio::test]
async fn expired_dispatch_recovers_to_unknown_once_and_never_reclaims() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(user)).await.unwrap();
        let record = approve(&repository, &record).await;
        let claim = repository
            .claim(claim_request(&record))
            .await
            .unwrap()
            .unwrap();
        database.set_deadline_age(record.effect_id, 5).await;
        let restarted = ExecutionRepository::new(database.db.clone());
        assert_eq!(
            restarted.recover_expired(10).await.unwrap(),
            0,
            "worker retains the documented ten second result grace"
        );
        database.set_deadline_age(record.effect_id, 30).await;
        assert_eq!(restarted.recover_expired(10).await.unwrap(), 1);
        assert_eq!(restarted.recover_expired(10).await.unwrap(), 0);
        let retained = restarted
            .get(user, record.effect_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.status, EffectStatus::Unknown);
        assert_eq!(retained.claim_id, Some(claim.claim_id));
        assert!(matches!(
            retained.completion,
            Some(Completion::Unknown { .. })
        ));
        assert!(
            restarted
                .claim(claim_request(&record))
                .await
                .unwrap()
                .is_none()
        );
        assert!(restarted.pending(10).await.unwrap().is_empty());
        assert!(
            restarted
                .complete(
                    claim.claim_id,
                    Completion::Succeeded {
                        receipt: serde_json::json!({"late":true})
                    }
                )
                .await
                .is_err(),
            "unverified late receipt must not silently overwrite recovery state"
        );
    })
    .await;
}

#[tokio::test]
async fn completions_are_claim_bound_idempotent_and_reject_conflicts() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(user)).await.unwrap();
        let record = approve(&repository, &record).await;
        let claim = repository
            .claim(claim_request(&record))
            .await
            .unwrap()
            .unwrap();
        let completion = Completion::Succeeded {
            receipt: serde_json::json!({"device_receipt":"test-receipt-1"}),
        };
        assert!(
            repository
                .complete(Uuid::new_v4(), completion.clone())
                .await
                .is_err()
        );
        let first = repository
            .complete(claim.claim_id, completion.clone())
            .await
            .unwrap();
        let event_count = database.count("execution_events").await;
        let repeated = repository
            .complete(claim.claim_id, completion.clone())
            .await
            .unwrap();
        assert_eq!(first.status, EffectStatus::Succeeded);
        assert_eq!(first.completion, repeated.completion);
        assert_eq!(database.count("execution_events").await, event_count);
        assert!(matches!(
            repository
                .complete(
                    claim.claim_id,
                    Completion::Failed {
                        evidence: serde_json::json!({"different":true})
                    }
                )
                .await,
            Err(ExecutionError::CompletionConflict)
        ));
        assert_eq!(
            repository
                .get(user, record.effect_id)
                .await
                .unwrap()
                .unwrap()
                .completion,
            Some(completion)
        );
    })
    .await;
}

#[tokio::test]
async fn completion_database_failure_preserves_dispatch_for_safe_retry() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(user)).await.unwrap();
        let record = approve(&repository, &record).await;
        let claim = repository.claim(claim_request(&record)).await.unwrap().unwrap();
        let events_before = database.count("execution_events").await;
        database.sql("CREATE FUNCTION fail_completion_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.status = 'succeeded' THEN RAISE EXCEPTION 'injected completion update failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_completion_update BEFORE UPDATE ON execution_effects FOR EACH ROW EXECUTE FUNCTION fail_completion_update()").await;
        let completion = Completion::Succeeded { receipt: serde_json::json!({"receipt":"committed externally"}) };
        assert!(matches!(repository.complete(claim.claim_id, completion.clone()).await, Err(ExecutionError::Database(_))));
        let retained = repository.get(user, record.effect_id).await.unwrap().unwrap();
        assert_eq!(retained.status, EffectStatus::Dispatched);
        assert_eq!(retained.claim_id, Some(claim.claim_id));
        assert!(retained.completion.is_none());
        assert_eq!(database.count("execution_events").await, events_before);
        assert!(repository.claim(claim_request(&record)).await.unwrap().is_none());
        database.sql("DROP TRIGGER fail_completion_update ON execution_effects").await;
        assert_eq!(repository.complete(claim.claim_id, completion).await.unwrap().status, EffectStatus::Succeeded);
    }).await;
}

#[tokio::test]
async fn completion_and_audit_event_commit_atomically() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(user)).await.unwrap();
        let record = approve(&repository, &record).await;
        let claim = repository.claim(claim_request(&record)).await.unwrap().unwrap();
        let events_before = database.count("execution_events").await;
        database.sql("CREATE FUNCTION fail_event_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected execution event failure'; END $$; CREATE TRIGGER fail_event_insert BEFORE INSERT ON execution_events FOR EACH ROW EXECUTE FUNCTION fail_event_insert()").await;
        let completion = Completion::Failed { evidence: serde_json::json!({"rejected_before_action":true}) };
        assert!(matches!(repository.complete(claim.claim_id, completion.clone()).await, Err(ExecutionError::Database(_))));
        let retained = repository.get(user, record.effect_id).await.unwrap().unwrap();
        assert_eq!(retained.status, EffectStatus::Dispatched);
        assert!(retained.completion.is_none());
        assert_eq!(database.count("execution_events").await, events_before);
        database.sql("DROP TRIGGER fail_event_insert ON execution_events").await;
        assert_eq!(repository.complete(claim.claim_id, completion).await.unwrap().status, EffectStatus::Failed);
        assert_eq!(database.count("execution_events").await, events_before + 1);
    }).await;
}

#[tokio::test]
async fn queries_and_owner_mutations_do_not_cross_user_boundaries() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let alice = database.user("user").await;
        let bob = database.user("admin").await;
        database.tool(true, None).await;
        let record = repository.propose(effect(alice)).await.unwrap();
        assert!(
            repository
                .get(bob, record.effect_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(repository.list(bob, None, 10).await.unwrap().is_empty());
        assert!(
            repository
                .approve(
                    bob,
                    record.effect_id,
                    Approval {
                        actor_id: bob,
                        kind: ApprovalKind::Explicit,
                        expected_intent_digest: record.intent_digest.clone()
                    }
                )
                .await
                .is_err()
        );
        assert!(repository.cancel(bob, record.effect_id).await.is_err());
        let own = repository.list(alice, None, 10).await.unwrap();
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].effect_id, record.effect_id);
        assert_eq!(own[0].status, EffectStatus::Proposed);
        assert_eq!(database.count("execution_outbox").await, 0);
    })
    .await;
}

#[tokio::test]
async fn concurrent_proposals_cannot_exceed_per_user_confirmation_capacity() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        let user = database.user("user").await;
        database.tool(true, None).await;
        for _ in 0..MAX_PROPOSED_PER_USER - 1 {
            repository.propose(effect(user)).await.unwrap();
        }
        let results =
            futures::future::join_all((0..8).map(|_| repository.propose(effect(user)))).await;
        let mut accepted = 0;
        for result in results {
            match result {
                Ok(_) => accepted += 1,
                Err(ExecutionError::CapacityExceeded) => {}
                Err(error) => panic!("unexpected proposal failure: {error}"),
            }
        }
        assert_eq!(accepted, 1);
        assert_eq!(
            database.count("execution_effects").await,
            MAX_PROPOSED_PER_USER as i64
        );
        assert_eq!(database.count("execution_outbox").await, 0);
        let existing = repository.list(user, None, 1).await.unwrap().remove(0);
        repository.cancel(user, existing.effect_id).await.unwrap();
        assert!(repository.propose(effect(user)).await.is_ok());
    })
    .await;
}

#[tokio::test]
async fn concurrent_approvals_cannot_exceed_durable_queue_capacity() {
    with_database(|database| async move {
        let repository = ExecutionRepository::new(database.db.clone());
        database.tool(true, None).await;
        let user = database.user("user").await;
        for _ in 0..MAX_PENDING - 1 {
            let record = repository.propose(effect(user)).await.unwrap();
            approve(&repository, &record).await;
        }
        let mut candidates = Vec::new();
        for _ in 0..8 {
            candidates.push(repository.propose(effect(user)).await.unwrap());
        }
        let results = futures::future::join_all(
            candidates
                .iter()
                .map(|record| repository.approve(user, record.effect_id, approval(record))),
        )
        .await;
        let mut accepted = Vec::new();
        for result in results {
            match result {
                Ok(record) => accepted.push(record),
                Err(ExecutionError::CapacityExceeded) => {}
                Err(error) => panic!("unexpected approval failure: {error}"),
            }
        }
        assert_eq!(accepted.len(), 1);
        assert_eq!(database.count("execution_outbox").await, MAX_PENDING as i64);
        let remaining = repository
            .get(
                user,
                candidates
                    .iter()
                    .find(|candidate| candidate.effect_id != accepted[0].effect_id)
                    .unwrap()
                    .effect_id,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remaining.status, EffectStatus::Proposed);
        repository
            .cancel(user, accepted[0].effect_id)
            .await
            .unwrap();
        approve(&repository, &remaining).await;
        assert_eq!(database.count("execution_outbox").await, MAX_PENDING as i64);
    })
    .await;
}
