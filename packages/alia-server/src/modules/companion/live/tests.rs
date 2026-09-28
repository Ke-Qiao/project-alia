//! Real PostgreSQL tests of the original pending transaction and charged identity.
use super::*;
use crate::modules::{
    companion::types::*, delivery::types::ResponseRecord,
    interaction::initiative::VoiceOpportunity, memory::context::ContextBinding,
};
use alia_core::delivery::VoiceScope;
use chrono::{DateTime, Duration, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};
use uuid::Uuid;

pub(crate) use crate::modules::companion::repository::tests::{Fixture, with_fixture};

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}

async fn wait_for_blocked_retry(f: &Fixture, blocker: i32) {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let row = f.db.query_one(sql(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))) AS blocked",
                vec![blocker.into()],
            )).await.unwrap().unwrap();
            if row.try_get::<bool>("", "blocked").unwrap() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("retry never reached the original transaction barrier");
}
impl Fixture {
    pub(crate) fn live(&self, instance: Uuid) -> LiveRepository {
        LiveRepository::new(self.db.clone(), instance)
    }
    pub(crate) async fn enable_live(&self, policy: CompanionPolicy) -> PolicySnapshot {
        assert_eq!(policy.mode, CompanionMode::Enabled);
        let prior = self.repo().policy(self.owner).await.unwrap();
        self.live(Uuid::new_v4())
            .replace_policy(self.owner, prior.revision, policy)
            .await
            .unwrap()
    }
    pub(crate) async fn seed_pending(&self, repository: &LiveRepository) -> PendingInvitation {
        let (_, source) = self.seed().await;
        let pending = repository.discover_pending(32).await.unwrap();
        let id: Uuid = self
            .db
            .query_one(sql(
                "SELECT candidate_id FROM companion_live_pending WHERE source_event_id=$1",
                vec![source.into()],
            ))
            .await
            .unwrap()
            .expect("canonical live source must be discovered")
            .try_get("", "candidate_id")
            .unwrap();
        pending.into_iter().find(|p| p.candidate_id == id).unwrap()
    }
    pub(crate) async fn opportunity(&self, pending: &PendingInvitation) -> VoiceOpportunity {
        let now: DateTime<Utc> = self
            .db
            .query_one(sql("SELECT clock_timestamp() AS now", vec![]))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "now")
            .unwrap();
        VoiceOpportunity {
            id: Uuid::new_v4(),
            owner: pending.owner_id,
            channel_id: pending.channel_id.clone(),
            scope: VoiceScope {
                device_boot_id: Uuid::new_v4(),
                session_id: self.session,
                generation_epoch: 1,
            },
            speech_revision: 0,
            through_sequence: 10,
            observed_at: now,
            last_capture_at: now,
            expires_at: now + Duration::seconds(5),
        }
    }
    pub(crate) async fn fresh(
        &self,
        repository: &LiveRepository,
    ) -> (LiveAdmissionPermit, ContextBinding, ResponseRecord) {
        let pending = self.seed_pending(repository).await;
        let intent = repository
            .prepare_intent(pending.candidate_id, self.opportunity(&pending).await)
            .await
            .unwrap();
        match repository.admit(&intent).await.unwrap() {
            AdmissionResult::Fresh {
                permit,
                context,
                response,
            } => (permit, context, response),
            other => panic!("expected fresh invitation: {other:?}"),
        }
    }
}

#[tokio::test]
async fn atomic_admission_rollback_and_original_pending_lock_reconcile_without_second_permit() {
    with_fixture(|f| async move {
        let repository = f.live(Uuid::new_v4());
        f.enable_live(CompanionPolicy {
            mode: CompanionMode::Enabled,
            ..Default::default()
        })
        .await;
        let pending = f.seed_pending(&repository).await;
        let intent = repository
            .prepare_intent(pending.candidate_id, f.opportunity(&pending).await)
            .await
            .unwrap();
        let contexts = f.count("context_leases").await;
        f.db.execute_unprepared("CREATE FUNCTION reject_live_response() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected Response write failure'; END; $$; CREATE TRIGGER reject_live_response BEFORE INSERT ON delivery_responses FOR EACH ROW EXECUTE FUNCTION reject_live_response();").await.unwrap();
        assert!(matches!(repository.admit(&intent).await,Err(LiveError::Unavailable(_))));
        assert_eq!(f.count("companion_admissions").await,0);
        assert_eq!(f.count("context_leases").await,contexts);
        f.db.execute_unprepared("DROP TRIGGER reject_live_response ON delivery_responses; DROP FUNCTION reject_live_response();").await.unwrap();
        // Stage the *production* atomic write but control the database outcome.
        // This is a commit/rollback barrier test, not physical commit-ACK loss.
        let staged = repository.stage_for_test(&intent).await.unwrap();
        let blocker: i32 = staged.query_one(sql("SELECT pg_backend_pid() AS pid",vec![])).await.unwrap().unwrap().try_get("","pid").unwrap();
        assert_eq!(f.count("companion_admissions").await, 0);
        assert_eq!(f.count("context_leases").await, contexts);
        assert_eq!(f.count("delivery_responses").await, 0);
        let retry = repository.clone();
        let retry_intent = intent.clone();
        let waiting = tokio::spawn(async move { retry.admit(&retry_intent).await });
        wait_for_blocked_retry(&f,blocker).await;
        assert!(!waiting.is_finished());
        staged.rollback().await.unwrap();
        let permit = match waiting.await.unwrap().unwrap() {
            AdmissionResult::Fresh { permit, .. } => permit,
            other => panic!("rollback must allow only original intent: {other:?}"),
        };
        assert_eq!(permit.admission_id(), intent.admission_id());
        assert_eq!(f.count("companion_admissions").await, 1);
        assert_eq!(f.count("context_leases").await, contexts + 1);
        assert_eq!(f.count("delivery_responses").await, 1);
        assert!(matches!(
            repository.admit(&intent).await.unwrap(),
            AdmissionResult::Existing(_)
        ));
    })
    .await;
}

#[tokio::test]
async fn committed_original_transaction_returns_existing_and_cleanup_survives_revocation() {
    with_fixture(|f| async move {
        let repository = f.live(Uuid::new_v4());
        f.enable_live(CompanionPolicy {
            mode: CompanionMode::Enabled,
            ..Default::default()
        })
        .await;
        let pending = f.seed_pending(&repository).await;
        let intent = repository
            .prepare_intent(pending.candidate_id, f.opportunity(&pending).await)
            .await
            .unwrap();
        let staged = repository.stage_for_test(&intent).await.unwrap();
        let blocker: i32 = staged
            .query_one(sql("SELECT pg_backend_pid() AS pid", vec![]))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "pid")
            .unwrap();
        let retry = repository.clone();
        let retry_intent = intent.clone();
        let waiting = tokio::spawn(async move { retry.admit(&retry_intent).await });
        wait_for_blocked_retry(&f, blocker).await;
        assert!(!waiting.is_finished());
        staged.commit().await.unwrap();
        let receipt = match waiting.await.unwrap().unwrap() {
            AdmissionResult::Existing(receipt) => receipt,
            other => {
                panic!("already committed transaction cannot grant a replacement permit: {other:?}")
            }
        };
        assert_eq!(receipt.admission_id, intent.admission_id());
        // A revoked owner cannot get new authority, yet the original managed
        // cleanup must retire an admission whose commit was not acknowledged.
        f.db.execute(sql(
            "UPDATE users SET status='disabled' WHERE id=$1",
            vec![f.owner.into()],
        ))
        .await
        .unwrap();
        repository
            .retire_candidate(pending.candidate_id, RetirementReason::AttemptFailed)
            .await
            .unwrap();
        repository
            .retire_candidate(pending.candidate_id, RetirementReason::AttemptFailed)
            .await
            .unwrap();
        let row =
            f.db.query_one(sql(
                "SELECT state,private_grant,grant_digest,intent_digest FROM companion_admissions",
                vec![],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<String>("", "state").unwrap(), "retired");
        assert!(
            row.try_get::<Option<serde_json::Value>>("", "private_grant")
                .unwrap()
                .is_none()
        );
        assert!(
            row.try_get::<Option<String>>("", "grant_digest")
                .unwrap()
                .is_none()
        );
        assert!(
            row.try_get::<Option<String>>("", "intent_digest")
                .unwrap()
                .is_none()
        );
        assert_eq!(f.count("companion_admissions").await, 1);
        assert_eq!(f.count("companion_source_claims").await, 1);
        let row =
            f.db.query_one(sql(
                "SELECT cancel_requested FROM delivery_responses",
                vec![],
            ))
            .await
            .unwrap()
            .unwrap();
        assert!(row.try_get::<bool>("", "cancel_requested").unwrap());
    })
    .await;
}

#[tokio::test]
async fn last_daily_budget_is_serialized_and_erased_charge_is_not_refunded() {
    with_fixture(|f| async move {
        let repository = f.live(Uuid::new_v4());
        f.enable_live(CompanionPolicy {mode:CompanionMode::Enabled,daily_budget:1,..Default::default()}).await;
        let first = f.seed_pending(&repository).await;
        let second = f.seed_pending(&repository).await;
        let left = repository.prepare_intent(first.candidate_id,f.opportunity(&first).await).await.unwrap();
        let right = repository.prepare_intent(second.candidate_id,f.opportunity(&second).await).await.unwrap();
        let (a,b) = tokio::join!(repository.admit(&left),repository.admit(&right));
        let results = [a.unwrap(),b.unwrap()];
        assert_eq!(results.iter().filter(|r|matches!(r,AdmissionResult::Fresh{..})).count(),1);
        assert_eq!(results.iter().filter(|r|matches!(r,AdmissionResult::Deferred(reasons) if reasons.contains(&DecisionReason::DailyBudgetExhausted))).count(),1);
        let winning = results.into_iter().find_map(|r|match r { AdmissionResult::Fresh{permit,..}=>Some(permit.candidate_id()),_=>None }).unwrap();
        repository.retire_candidate(winning,RetirementReason::AttemptFailed).await.unwrap();
        let remaining = if winning == first.candidate_id {&right} else {&left};
        assert!(matches!(repository.admit(remaining).await.unwrap(),AdmissionResult::Deferred(reasons) if reasons.contains(&DecisionReason::DailyBudgetExhausted)));
        // A new policy revision and a genuinely new source do not reset the
        // charged UTC day, cooldown or the opaque retired admission.
        f.enable_live(CompanionPolicy {mode:CompanionMode::Enabled,daily_budget:1,..Default::default()}).await;
        let next = f.seed_pending(&repository).await;
        let next_intent = repository.prepare_intent(next.candidate_id,f.opportunity(&next).await).await.unwrap();
        assert!(matches!(repository.admit(&next_intent).await.unwrap(),AdmissionResult::Deferred(reasons) if reasons.contains(&DecisionReason::DailyBudgetExhausted) && reasons.contains(&DecisionReason::Cooldown)));
        assert_eq!(f.count("companion_admissions").await,1);
        assert_eq!(f.count("delivery_responses").await,1);
        assert!(f.db.execute_unprepared("DELETE FROM companion_admissions").await.is_err());
    }).await;
}

#[tokio::test]
async fn shadow_sources_and_limited_worklist_never_upgrade_to_live() {
    with_fixture(|f| async move {
        let repository = f.live(Uuid::new_v4());
        let mut appraisals = Vec::new();
        for _ in 0..5 {
            appraisals.push(f.seed().await.0);
        }
        let operation = Uuid::new_v4();
        let report = f
            .repo()
            .evaluate(f.owner, operation, appraisals)
            .await
            .unwrap();
        assert_eq!(report.decisions.len(), 4);
        assert_eq!(report.limited, 1);
        assert_eq!(f.count("companion_source_claims").await, 5);
        f.enable_live(CompanionPolicy {
            mode: CompanionMode::Enabled,
            ..Default::default()
        })
        .await;
        assert!(repository.discover_pending(32).await.unwrap().is_empty());
        assert_eq!(f.count("companion_live_pending").await, 0);
        let pending = f.seed_pending(&repository).await;
        let other = f.live(Uuid::new_v4());
        assert_eq!(other.retire_batch(32).await.unwrap(), 1);
        assert!(other.discover_pending(32).await.unwrap().is_empty());
        assert!(
            repository
                .prepare_intent(pending.candidate_id, f.opportunity(&pending).await)
                .await
                .is_err()
        );
        assert_eq!(f.count("companion_source_claims").await, 6);
    })
    .await;
}

#[tokio::test]
async fn pending_capacity_original_expiry_and_source_erasure_remain_bounded() {
    with_fixture(|f| async move {
        let repository = f.live(Uuid::new_v4());
        f.enable_live(CompanionPolicy {mode:CompanionMode::Enabled,..Default::default()}).await;
        for _ in 0..5 {f.seed().await;}
        let pending = repository.discover_pending(32).await.unwrap();
        assert_eq!(pending.len(),4);
        let after_fence = repository.discover_pending_excluding(1,&[pending[0].candidate_id]).await.unwrap();
        assert_eq!(after_fence.len(),1);
        assert_ne!(after_fence[0].candidate_id,pending[0].candidate_id);
        assert_eq!(repository.discover_pending(32).await.unwrap().len(),4);
        assert_eq!(f.count("companion_source_claims").await,4);
        let id = pending[0].candidate_id;
        let before = f.db.query_one(sql("SELECT private_candidate,created_at,expires_at FROM companion_live_pending WHERE candidate_id=$1",vec![id.into()])).await.unwrap().unwrap();
        let candidate: Candidate = serde_json::from_value(before.try_get("","private_candidate").unwrap()).unwrap();
        assert_eq!(candidate.created_at,candidate.observation.source_received_at);
        assert_eq!(candidate.expires_at,before.try_get::<DateTime<Utc>>("","expires_at").unwrap());
        f.db.execute(sql("UPDATE chat_messages SET content='source changed' WHERE id=$1",vec![candidate.observation.source_event_id.into()])).await.unwrap();
        let after = f.db.query_one(sql("SELECT state,private_candidate,expires_at FROM companion_live_pending WHERE candidate_id=$1",vec![id.into()])).await.unwrap().unwrap();
        assert_eq!(after.try_get::<String>("","state").unwrap(),"retired");
        assert!(after.try_get::<Option<serde_json::Value>>("","private_candidate").unwrap().is_none());
        assert_eq!(after.try_get::<DateTime<Utc>>("","expires_at").unwrap(),candidate.expires_at);
        // Later turns in this conversation inherit the first message as a
        // canonical root. All four pending rows retire, and the fifth source's
        // stale parent cannot be refreshed merely because capacity is free.
        assert!(repository.discover_pending(32).await.unwrap().is_empty());
        assert_eq!(f.count("companion_source_claims").await,4);
        let live = f.db.query_one(sql("SELECT count(*)::bigint AS n FROM companion_live_pending WHERE private_candidate IS NOT NULL",vec![])).await.unwrap().unwrap();
        assert_eq!(live.try_get::<i64>("","n").unwrap(),0);
        assert!(repository.discover_pending(33).await.is_err());
    }).await;
}

#[tokio::test]
async fn commit_checks_original_short_admission_deadline_without_refreshing_output_deadline() {
    with_fixture(|f| async move {
        let repository = f.live(Uuid::new_v4());
        f.enable_live(CompanionPolicy {
            mode: CompanionMode::Enabled,
            ..Default::default()
        })
        .await;
        let pending = f.seed_pending(&repository).await;
        let opportunity = f.opportunity(&pending).await;
        let intent = repository
            .prepare_intent(pending.candidate_id, opportunity)
            .await
            .unwrap();
        assert!(intent.admit_before() < pending.expires_at);
        let contexts = f.count("context_leases").await;
        let staged = repository.stage_for_test(&intent).await.unwrap();
        // Hold the real pending transaction until the originally frozen instant;
        // its deferred SQL constraint must roll the entire write back at commit.
        let remaining = (intent.admit_before() - Utc::now())
            .to_std()
            .unwrap_or_default();
        tokio::time::sleep(remaining).await;
        assert!(staged.commit().await.is_err());
        assert_eq!(f.count("companion_admissions").await, 0);
        assert_eq!(f.count("delivery_responses").await, 0);
        assert_eq!(f.count("context_leases").await, contexts);
        assert!(matches!(
            repository.admit(&intent).await.unwrap(),
            AdmissionResult::Retired
        ));
        assert!(repository.discover_pending(32).await.unwrap().is_empty());
    })
    .await;
}

#[tokio::test]
async fn capacity_lock_serializes_distinct_owners_in_the_same_schema() {
    with_fixture(|f| async move {
        let repository = f.live(Uuid::new_v4());
        let policy = CompanionPolicy {
            mode: CompanionMode::Enabled,
            ..Default::default()
        };
        f.enable_live(policy.clone()).await;
        let pending = f.seed_pending(&repository).await;
        let other_owner = Uuid::new_v4();
        let other_session = Uuid::new_v4();
        f.add_owner(other_owner, other_session).await;
        let old = f.repo().policy(other_owner).await.unwrap();
        repository
            .replace_policy(other_owner, old.revision, policy)
            .await
            .unwrap();
        f.seed_for(other_owner, other_session).await;
        let intent = repository
            .prepare_intent(pending.candidate_id, f.opportunity(&pending).await)
            .await
            .unwrap();
        let staged = repository.stage_for_test(&intent).await.unwrap();
        let blocker: i32 = staged
            .query_one(sql("SELECT pg_backend_pid() AS pid", vec![]))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "pid")
            .unwrap();
        let competing = repository.clone();
        let waiting = tokio::spawn(async move { competing.discover_pending(32).await });
        wait_for_blocked_retry(&f, blocker).await;
        assert!(!waiting.is_finished());
        staged.rollback().await.unwrap();
        let discovered = waiting.await.unwrap().unwrap();
        assert_eq!(discovered.len(), 2);
        assert!(discovered.iter().any(|p| p.owner_id == other_owner));
        assert_eq!(f.count("companion_source_claims").await, 2);
        assert_eq!(f.count("companion_admissions").await, 0);
    })
    .await;
}
