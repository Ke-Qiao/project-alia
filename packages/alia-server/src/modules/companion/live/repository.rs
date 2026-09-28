use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, IsolationLevel,
    QueryResult, Statement, TransactionTrait, Value,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::types::*;
use crate::modules::{
    companion::{
        decision::{canonical_digest, decide, generate_candidate},
        repository::locked_policy,
        types::*,
    },
    delivery::{
        repository::DeliveryRepository,
        types::{NewResponse, ResponseKey},
    },
    interaction::initiative::VoiceOpportunity,
    memory::context::{
        ContextError,
        companion::{
            issue_invitation_in_transaction, load_companion_source, lock_companion_authority,
        },
    },
};

const CAPACITY_LOCK: i64 = 0x414c_434f_4d50_4c56;
const PENDING_OWNER: i64 = 4;
const PENDING_TOTAL: i64 = 128;
const GRANT_OWNER: i64 = 128;
const GRANT_TOTAL: i64 = 1024;

#[derive(Clone)]
pub(crate) struct LiveRepository {
    db: DatabaseConnection,
    instance_id: Uuid,
}

// Transaction-local data is not a permit. Only admit's acknowledged commit
// converts it into the single consumable permission.
enum PreparedAdmission {
    Fresh {
        context: crate::modules::memory::context::ContextBinding,
        response: crate::modules::delivery::types::ResponseRecord,
    },
    Observed(AdmissionResult),
}
impl LiveRepository {
    pub fn new(db: DatabaseConnection, instance_id: Uuid) -> Self {
        Self { db, instance_id }
    }
    async fn transaction(&self) -> LiveResult<DatabaseTransaction> {
        if self.instance_id.is_nil() {
            return Err(LiveError::InvalidInput);
        }
        let tx = self
            .db
            .begin_with_config(Some(IsolationLevel::ReadCommitted), None)
            .await?;
        tx.execute(sql("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        Ok(tx)
    }
    /// Internal integration hook only; the public shadow repository stays closed.
    pub async fn replace_policy(
        &self,
        owner: Uuid,
        expected: i64,
        policy: CompanionPolicy,
    ) -> LiveResult<PolicySnapshot> {
        policy.validate().map_err(|_| LiveError::InvalidInput)?;
        let tx = self.transaction().await?;
        lock_companion_authority(&tx, owner).await?;
        let prior = locked_policy(&tx, owner).await?;
        if prior.revision != expected {
            return Err(LiveError::Conflict);
        }
        let revision = expected.checked_add(1).ok_or(LiveError::Conflict)?;
        tx.execute(sql("UPDATE companion_policies SET revision=$2,policy=$3,policy_digest=$4,updated_at=clock_timestamp() WHERE owner_id=$1",vec![owner.into(),revision.into(),json!(policy).into(),canonical_digest(&policy)?.into()])).await?;
        tx.commit().await?;
        Ok(PolicySnapshot { revision, policy })
    }

    /// SQL, not the event bus, is the restart/work source. A scan is <=32 rows.
    pub async fn discover_pending(&self, limit: u16) -> LiveResult<Vec<PendingInvitation>> {
        self.discover_pending_excluding(limit, &[]).await
    }

    /// A retained uncertain-attempt fence is excluded before LIMIT, so failed
    /// cleanup of an early batch cannot hide later original candidates.
    pub async fn discover_pending_excluding(
        &self,
        limit: u16,
        excluded: &[Uuid],
    ) -> LiveResult<Vec<PendingInvitation>> {
        bounded(limit)?;
        if excluded.len() > 1024 {
            return Err(LiveError::InvalidInput);
        }
        let tx = self.transaction().await?;
        let rows=tx.query_all(sql("SELECT a.owner_id,a.appraisal_id FROM appraisal_records a JOIN companion_policies p ON p.owner_id=a.owner_id JOIN users u ON u.id=a.owner_id AND u.status='active' JOIN memory_privacy_state privacy ON privacy.user_id=a.owner_id AND privacy.privacy_epoch=a.source_privacy_epoch JOIN relationship_authority relationship ON relationship.owner_id=a.owner_id AND relationship.relationship_epoch=a.source_relationship_epoch JOIN context_policy g ON g.singleton AND g.version=a.policy_version WHERE (g.policy->'appraisal'->'affect'->>'enabled')::boolean AND (g.policy->'appraisal'->'affect'->>'relationship_profile_user_visible')::boolean AND g.policy->>'paused_until' IS NULL AND a.status='active' AND a.source_fact->>'kind'='user_message' AND p.policy->>'mode'='enabled' AND NOT (p.policy->>'quiet')::boolean AND NOT (p.policy->>'paused')::boolean AND p.enabled_since IS NOT NULL AND a.effective_at>=p.enabled_since AND a.effective_at<=clock_timestamp() AND a.valid_until>clock_timestamp() AND a.effective_at+((p.policy->>'candidate_ttl_seconds')::integer*interval '1 second')>clock_timestamp() AND NOT EXISTS(SELECT 1 FROM companion_source_claims c WHERE c.owner_id=a.owner_id AND c.source_event_id=a.source_event_id) AND NOT EXISTS(SELECT 1 FROM companion_rejected_sources r WHERE r.owner_id=a.owner_id AND r.appraisal_id=a.appraisal_id) AND (SELECT count(*) FROM companion_live_pending l WHERE l.owner_id=a.owner_id AND l.private_candidate IS NOT NULL)<4 ORDER BY a.effective_at,a.appraisal_id LIMIT $1",vec![i64::from(limit).into()])).await?;
        tx.commit().await?;
        for row in rows {
            self.create_pending(
                row.try_get("", "owner_id")?,
                row.try_get("", "appraisal_id")?,
            )
            .await?;
        }
        let tx = self.transaction().await?;
        let rows=tx.query_all(sql("SELECT candidate_id,owner_id,private_candidate->'observation'->>'channel_id' AS channel_id,expires_at FROM companion_live_pending WHERE state='pending' AND server_instance_id=$1 AND expires_at>clock_timestamp() AND NOT candidate_id=ANY(ARRAY(SELECT jsonb_array_elements_text($3)::uuid)) ORDER BY created_at,candidate_id LIMIT $2",vec![self.instance_id.into(),i64::from(limit).into(),json!(excluded).into()])).await?;
        tx.commit().await?;
        rows.into_iter()
            .map(|r| {
                Ok(PendingInvitation {
                    candidate_id: r.try_get("", "candidate_id")?,
                    owner_id: r.try_get("", "owner_id")?,
                    channel_id: r.try_get("", "channel_id")?,
                    expires_at: r.try_get("", "expires_at")?,
                })
            })
            .collect()
    }
    async fn create_pending(&self, owner: Uuid, appraisal: Uuid) -> LiveResult<()> {
        let tx = self.transaction().await?;
        let authority = match lock_companion_authority(&tx, owner).await {
            Ok(authority) if authority.private_readable => authority,
            Ok(_) => return Ok(()),
            Err(error)
                if matches!(
                    error.downcast_ref::<ContextError>(),
                    Some(ContextError::Forbidden)
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        let source = match load_companion_source(&tx, owner, appraisal, &authority).await {
            Ok(s) => s,
            Err(e)
                if matches!(
                    e.downcast_ref::<ContextError>(),
                    Some(ContextError::Revoked(_) | ContextError::PolicyChanged)
                ) =>
            {
                tx.execute(sql("INSERT INTO companion_rejected_sources(owner_id,appraisal_id) SELECT owner_id,appraisal_id FROM appraisal_records WHERE owner_id=$1 AND appraisal_id=$2 ON CONFLICT DO NOTHING",vec![owner.into(),appraisal.into()])).await?;
                tx.commit().await?;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let policy = locked_policy(&tx, owner).await?;
        let cutoff: Option<DateTime<Utc>> = tx
            .query_one(sql(
                "SELECT enabled_since FROM companion_policies WHERE owner_id=$1",
                vec![owner.into()],
            ))
            .await?
            .unwrap()
            .try_get("", "enabled_since")?;
        if policy.policy.mode != CompanionMode::Enabled
            || policy.policy.quiet
            || policy.policy.paused
            || cutoff.is_none_or(|at| source.observation.source_received_at < at)
        {
            return Ok(());
        }
        let candidate = generate_candidate(
            &source.observation,
            &policy,
            source.observation.effective_at,
        )?;
        if candidate.expires_at <= authority.as_of {
            return Ok(());
        }
        tx.execute(sql(
            "SELECT pg_advisory_xact_lock(hashtextextended(current_schema(),$1))",
            vec![CAPACITY_LOCK.into()],
        ))
        .await?;
        let counts=tx.query_one(sql("SELECT count(*)::bigint AS total,count(*) FILTER(WHERE owner_id=$1)::bigint AS owned FROM companion_live_pending WHERE private_candidate IS NOT NULL",vec![owner.into()])).await?.unwrap();
        if counts.try_get::<i64>("", "owned")? >= PENDING_OWNER
            || counts.try_get::<i64>("", "total")? >= PENDING_TOTAL
        {
            return Ok(());
        }
        let claimed=tx.execute(sql("INSERT INTO companion_source_claims(owner_id,source_event_id,kind,generator_version,origin_mode,policy_revision) VALUES($1,$2,'follow_up_invitation',$3,'enabled',$4) ON CONFLICT DO NOTHING",vec![owner.into(),candidate.observation.source_event_id.into(),GENERATOR_VERSION.into(),policy.revision.into()])).await?.rows_affected();
        if claimed == 0 {
            return Ok(());
        }
        tx.execute(sql("INSERT INTO companion_live_pending(candidate_id,owner_id,source_event_id,kind,generator_version,server_instance_id,created_at,expires_at,state,private_candidate,candidate_digest,appraisal_id) VALUES($1,$2,$3,'follow_up_invitation',$4,$5,$6,$7,'pending',$8,$9,$10)",vec![candidate.id.into(),owner.into(),candidate.observation.source_event_id.into(),GENERATOR_VERSION.into(),self.instance_id.into(),candidate.created_at.into(),candidate.expires_at.into(),json!(candidate).into(),canonical_digest(&candidate)?.into(),appraisal.into()])).await?;
        for (kind, id) in source.erasure_roots {
            tx.execute(sql(
                "INSERT INTO companion_live_sources(candidate_id,kind,source_id) VALUES($1,$2,$3)",
                vec![candidate.id.into(), kind.into(), id.into()],
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn prepare_intent(
        &self,
        id: Uuid,
        opportunity: VoiceOpportunity,
    ) -> LiveResult<LiveAdmissionIntent> {
        let tx = self.transaction().await?;
        let row=tx.query_one(sql("SELECT * FROM companion_live_pending WHERE candidate_id=$1 AND server_instance_id=$2 AND state='pending'",vec![id.into(),self.instance_id.into()])).await?.ok_or(LiveError::Forbidden)?;
        let candidate = read_candidate(&row)?;
        if opportunity.owner != candidate.observation.owner_id
            || opportunity.channel_id != candidate.observation.channel_id
            || !opportunity.scope.is_valid()
            || opportunity.id.is_nil()
        {
            return Err(LiveError::Forbidden);
        }
        let frozen = FrozenVoiceOpportunity::freeze(&opportunity);
        frozen
            .snapshot(self.instance_id)
            .validate()
            .map_err(|_| LiveError::InvalidInput)?;
        let next_day = opportunity
            .observed_at
            .date_naive()
            .succ_opt()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .ok_or(LiveError::InvalidInput)?
            .and_utc();
        let admit_before = micros(
            candidate
                .expires_at
                .min(opportunity.expires_at)
                .min(opportunity.last_capture_at + chrono::Duration::seconds(2))
                .min(next_day),
        )?;
        let now = now(&tx).await?;
        if opportunity.observed_at > now || now >= admit_before {
            return Err(LiveError::Unavailable(
                "original voice opportunity expired".into(),
            ));
        }
        let expected_scope = alia_core::delivery::VoiceScope {
            generation_epoch: opportunity
                .scope
                .generation_epoch
                .checked_add(1)
                .ok_or(LiveError::InvalidInput)?,
            ..opportunity.scope
        };
        let admission_id = identity("admission", id);
        let context_id = identity("context", id);
        let initiative_request_id = identity("request", id);
        let digest = canonical_digest(
            &json!({"candidate":candidate,"instance":self.instance_id,"opportunity":frozen,"scope":expected_scope,"admission_id":admission_id,"context_id":context_id,"request_id":initiative_request_id,"admit_before":admit_before,"valid_until":candidate.expires_at}),
        )?;
        tx.commit().await?;
        Ok(LiveAdmissionIntent {
            valid_until: candidate.expires_at,
            candidate,
            admission_id,
            context_id,
            initiative_request_id,
            server_instance_id: self.instance_id,
            opportunity,
            expected_scope,
            admit_before,
            digest,
        })
    }

    pub async fn admit(&self, intent: &LiveAdmissionIntent) -> LiveResult<AdmissionResult> {
        let tx = self.transaction().await?;
        let prepared = self.prepare_admission(&tx, intent).await?;
        tx.commit().await.map_err(|_| LiveError::CommitUnknown {
            candidate_id: intent.candidate.id,
            admission_id: intent.admission_id,
        })?;
        Ok(match prepared {
            PreparedAdmission::Fresh { context, response } => AdmissionResult::Fresh {
                permit: LiveAdmissionPermit::committed(intent, context.clone()),
                context,
                response,
            },
            PreparedAdmission::Observed(result) => result,
        })
    }

    async fn prepare_admission(
        &self,
        tx: &DatabaseTransaction,
        intent: &LiveAdmissionIntent,
    ) -> LiveResult<PreparedAdmission> {
        if intent.server_instance_id != self.instance_id {
            return Err(LiveError::Forbidden);
        }
        let owner = intent.candidate.observation.owner_id;
        let authority = lock_companion_authority(tx, owner).await?;
        // Roots precede candidate/admission locks, including on retry.
        let source = load_companion_source(
            tx,
            owner,
            intent.candidate.observation.appraisals[0].appraisal_id,
            &authority,
        )
        .await;
        if let Err(e) = &source {
            if !matches!(
                e.downcast_ref::<ContextError>(),
                Some(
                    ContextError::Revoked(_)
                        | ContextError::PolicyChanged
                        | ContextError::Forbidden
                )
            ) {
                return Err(LiveError::Unavailable(e.to_string()));
            }
        }
        let policy = locked_policy(tx, owner).await?;
        let row=tx.query_one(sql("SELECT * FROM companion_live_pending WHERE candidate_id=$1 AND owner_id=$2 FOR UPDATE",vec![intent.candidate.id.into(),owner.into()])).await?.ok_or(LiveError::Forbidden)?;
        // New READ COMMITTED statement after acquiring the same serialization
        // row sees the prior transaction's commit; absence proves rollback.
        if let Some(old) = tx
            .query_one(sql(
                "SELECT * FROM companion_admissions WHERE candidate_id=$1",
                vec![intent.candidate.id.into()],
            ))
            .await?
        {
            if old
                .try_get::<Option<String>>("", "intent_digest")?
                .is_some_and(|v| v != intent.digest)
            {
                return Err(LiveError::Conflict);
            }
            let receipt = receipt(&old)?;
            return Ok(PreparedAdmission::Observed(AdmissionResult::Existing(
                receipt,
            )));
        }
        let as_of = now(tx).await?;
        if row.try_get::<String>("", "state")? != "pending"
            || row.try_get::<Uuid>("", "server_instance_id")? != self.instance_id
        {
            return Ok(PreparedAdmission::Observed(AdmissionResult::Retired));
        }
        let reason = if source
            .as_ref()
            .is_ok_and(|s| s.observation != intent.candidate.observation)
            || source.is_err()
        {
            Some(RetirementReason::SourceRevoked)
        } else if policy != intent.candidate.policy || policy.policy.mode != CompanionMode::Enabled
        {
            Some(RetirementReason::PolicyChanged)
        } else if as_of >= intent.admit_before {
            Some(RetirementReason::Expired)
        } else {
            None
        };
        if let Some(reason) = reason {
            retire(tx, intent.candidate.id, reason).await?;
            return Ok(PreparedAdmission::Observed(AdmissionResult::Retired));
        }
        if read_candidate(&row)? != intent.candidate {
            return Err(LiveError::Conflict);
        }
        let budget = budget(tx, &intent.candidate, as_of).await?;
        let input = DecisionInput {
            candidate: intent.candidate.clone(),
            current_policy: policy,
            as_of,
            opportunity: FrozenVoiceOpportunity::freeze(&intent.opportunity)
                .snapshot(self.instance_id),
            budget,
        };
        let decision = decide(&input)?;
        if decision.outcome != DecisionOutcome::Allowed {
            return Ok(PreparedAdmission::Observed(AdmissionResult::Deferred(
                decision.reasons,
            )));
        }
        let content = InvitationContent {
            schema_version: 1,
            invitation_kind: CandidateKind::FollowUpInvitation,
            generator_version: GENERATOR_VERSION.into(),
            text: CandidateKind::FollowUpInvitation.text().into(),
            voice_scope: intent.expected_scope,
            candidate_id: intent.candidate.id,
            admission_id: intent.admission_id,
            source_event_id: intent.candidate.observation.source_event_id,
        };
        let grant = LiveAdmissionGrant {
            schema_version: 1,
            admission_id: intent.admission_id,
            candidate: intent.candidate.clone(),
            context_id: intent.context_id,
            initiative_request_id: intent.initiative_request_id,
            server_instance_id: self.instance_id,
            opportunity: FrozenVoiceOpportunity::freeze(&intent.opportunity),
            expected_scope: intent.expected_scope,
            content: content.clone(),
            admit_before: intent.admit_before,
            valid_until: intent.valid_until,
            intent_digest: intent.digest.clone(),
            input,
            decision,
        };
        grant.validate()?;
        tx.execute(sql(
            "SELECT pg_advisory_xact_lock(hashtextextended(current_schema(),$1))",
            vec![CAPACITY_LOCK.into()],
        ))
        .await?;
        let count=tx.query_one(sql("SELECT count(*)::bigint AS total,count(*) FILTER(WHERE owner_id=$1)::bigint AS owned FROM companion_admissions WHERE private_grant IS NOT NULL",vec![owner.into()])).await?.unwrap();
        if count.try_get::<i64>("", "owned")? >= GRANT_OWNER
            || count.try_get::<i64>("", "total")? >= GRANT_TOTAL
        {
            return Err(LiveError::Unavailable("live grant capacity reached".into()));
        }
        let revision = i64::try_from(grant.input.budget.revision)
            .ok()
            .and_then(|v| v.checked_add(1))
            .ok_or(LiveError::Conflict)?;
        tx.execute(sql("INSERT INTO companion_admissions(admission_id,candidate_id,owner_id,initiative_request_id,output_key,context_id,conversation_id,server_instance_id,opportunity_id,charged_at,budget_date,budget_revision,admit_before,valid_until,state,private_grant,grant_digest,intent_digest) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,'active',$15,$16,$17)",vec![intent.admission_id.into(),intent.candidate.id.into(),owner.into(),intent.initiative_request_id.into(),INVITATION_OUTPUT_KEY.into(),intent.context_id.into(),intent.candidate.observation.conversation_id.into(),self.instance_id.into(),intent.opportunity.id.into(),as_of.into(),as_of.date_naive().into(),revision.into(),intent.admit_before.into(),intent.valid_until.into(),json!(grant).into(),grant.digest()?.into(),intent.digest.clone().into()])).await?;
        let lease = issue_invitation_in_transaction(tx, owner, intent.admission_id).await?;
        let s = lease.source();
        let response = DeliveryRepository::new(self.db.clone())
            .prepare_in_transaction(
                tx,
                NewResponse {
                    key: ResponseKey {
                        user_id: owner,
                        request_id: intent.initiative_request_id,
                        output_key: INVITATION_OUTPUT_KEY.into(),
                    },
                    context_id: lease.id(),
                    conversation_id: s.conversation_id,
                    source_event_id: s.source_event_id,
                    source_received_at: s.source_received_at,
                    source_privacy_epoch: Some(s.source_privacy_epoch),
                    channel_id: s.channel_id.clone(),
                    kind: INVITATION_KIND.into(),
                    content: json!(content),
                },
            )
            .await
            .map_err(|e| LiveError::Unavailable(e.to_string()))?;
        tx.execute(sql(
            "UPDATE companion_live_pending SET state='admitted' WHERE candidate_id=$1",
            vec![intent.candidate.id.into()],
        ))
        .await?;
        // A deferred constraint also checks this at commit, after all source,
        // Context and Response work. Expiry never leaves a charged partial grant.
        if now(tx).await? >= intent.admit_before {
            return Err(LiveError::Unavailable(
                "original voice opportunity expired before commit".into(),
            ));
        }
        Ok(PreparedAdmission::Fresh {
            context: lease.binding(),
            response,
        })
    }

    #[cfg(test)]
    pub(crate) async fn stage_for_test(
        &self,
        intent: &LiveAdmissionIntent,
    ) -> LiveResult<DatabaseTransaction> {
        let tx = self.transaction().await?;
        if !matches!(
            self.prepare_admission(&tx, intent).await?,
            PreparedAdmission::Fresh { .. }
        ) {
            return Err(LiveError::Conflict);
        }
        Ok(tx)
    }

    pub async fn receipt(&self, owner: Uuid, id: Uuid) -> LiveResult<AdmissionReceipt> {
        let tx = self.transaction().await?;
        lock_companion_authority(&tx, owner).await?;
        let row = tx
            .query_one(sql(
                "SELECT * FROM companion_admissions WHERE admission_id=$1 AND owner_id=$2",
                vec![id.into(), owner.into()],
            ))
            .await?
            .ok_or(LiveError::Forbidden)?;
        let result = receipt(&row)?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn retire_candidate(&self, id: Uuid, reason: RetirementReason) -> LiveResult<()> {
        let tx = self.transaction().await?;
        if tx.query_one(sql("SELECT candidate_id FROM companion_live_pending WHERE candidate_id=$1 AND server_instance_id=$2 FOR UPDATE",vec![id.into(),self.instance_id.into()])).await?.is_some() {retire(&tx,id,reason).await?;}
        tx.commit().await?;
        Ok(())
    }
    pub async fn retire_admission(
        &self,
        owner: Uuid,
        id: Uuid,
        reason: RetirementReason,
    ) -> LiveResult<()> {
        let tx = self.transaction().await?;
        if let Some(row)=tx.query_one(sql("SELECT p.candidate_id FROM companion_live_pending p JOIN companion_admissions a ON a.candidate_id=p.candidate_id WHERE a.admission_id=$1 AND a.owner_id=$2 AND a.server_instance_id=$3 FOR UPDATE OF p",vec![id.into(),owner.into(),self.instance_id.into()])).await? {retire(&tx,row.try_get("","candidate_id")?,reason).await?;}
        tx.commit().await?;
        Ok(())
    }
    pub async fn retire_batch(&self, limit: u16) -> LiveResult<u64> {
        bounded(limit)?;
        let tx = self.transaction().await?;
        let rows=tx.query_all(sql("SELECT candidate_id,server_instance_id FROM companion_live_pending WHERE state<>'retired' AND (server_instance_id<>$1 OR expires_at<=clock_timestamp()) ORDER BY candidate_id LIMIT $2 FOR UPDATE SKIP LOCKED",vec![self.instance_id.into(),i64::from(limit).into()])).await?;
        for row in &rows {
            retire(
                &tx,
                row.try_get("", "candidate_id")?,
                if row.try_get::<Uuid>("", "server_instance_id")? != self.instance_id {
                    RetirementReason::InstanceEnded
                } else {
                    RetirementReason::Expired
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(rows.len() as u64)
    }
}
fn bounded(limit: u16) -> LiveResult<()> {
    if !(1..=32).contains(&limit) {
        return Err(LiveError::InvalidInput);
    }
    Ok(())
}
fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}
async fn now(tx: &DatabaseTransaction) -> LiveResult<DateTime<Utc>> {
    Ok(tx
        .query_one(sql("SELECT clock_timestamp() AS now", vec![]))
        .await?
        .unwrap()
        .try_get("", "now")?)
}
fn micros(at: DateTime<Utc>) -> LiveResult<DateTime<Utc>> {
    DateTime::from_timestamp_micros(at.timestamp_micros()).ok_or(LiveError::InvalidInput)
}
fn identity(domain: &str, id: Uuid) -> Uuid {
    let hash = Sha256::digest(format!("alia.companion.live.v1:{domain}:{id}"));
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash[..16]);
    bytes[6] = (bytes[6] & 15) | 0x80;
    bytes[8] = (bytes[8] & 63) | 0x80;
    Uuid::from_bytes(bytes)
}
fn read_candidate(row: &QueryResult) -> LiveResult<Candidate> {
    let value: serde_json::Value = row.try_get("", "private_candidate")?;
    let candidate: Candidate = serde_json::from_value(value)?;
    candidate.validate()?;
    if canonical_digest(&candidate)? != row.try_get::<String>("", "candidate_digest")? {
        return Err(LiveError::Conflict);
    }
    Ok(candidate)
}
async fn retire(tx: &DatabaseTransaction, id: Uuid, reason: RetirementReason) -> LiveResult<()> {
    tx.execute(sql(
        "SELECT retire_companion_live(ARRAY[$1::uuid],$2)",
        vec![id.into(), reason.as_str().into()],
    ))
    .await?;
    Ok(())
}
fn receipt(row: &QueryResult) -> LiveResult<AdmissionReceipt> {
    Ok(AdmissionReceipt {
        admission_id: row.try_get("", "admission_id")?,
        candidate_id: row.try_get("", "candidate_id")?,
        initiative_request_id: row.try_get("", "initiative_request_id")?,
        state: if row.try_get::<String>("", "state")? == "active" {
            AdmissionState::Active
        } else {
            AdmissionState::Retired
        },
        charged_at: row.try_get("", "charged_at")?,
        budget_date: row.try_get("", "budget_date")?,
    })
}
async fn budget(
    tx: &DatabaseTransaction,
    candidate: &Candidate,
    as_of: DateTime<Utc>,
) -> LiveResult<BudgetSnapshot> {
    let owner = candidate.observation.owner_id;
    let row=tx.query_one(sql("SELECT count(*) FILTER(WHERE budget_date=$2)::bigint AS today,COALESCE(max(budget_revision),0)::bigint AS revision,max(charged_at) AS last FROM companion_admissions WHERE owner_id=$1",vec![owner.into(),as_of.date_naive().into()])).await?.unwrap();
    let last: Option<DateTime<Utc>> = row.try_get("", "last")?;
    let recent=tx.query_one(sql("SELECT conversation_id,charged_at FROM companion_admissions WHERE owner_id=$1 ORDER BY charged_at DESC,budget_revision DESC LIMIT 1",vec![owner.into()])).await?;
    let awaiting = if let Some(recent) = recent {
        candidate.observation.conversation_id != recent.try_get::<Uuid>("", "conversation_id")?
            || candidate.observation.source_received_at
                <= recent.try_get::<DateTime<Utc>>("", "charged_at")?
    } else {
        false
    };
    Ok(BudgetSnapshot {
        revision: u64::try_from(row.try_get::<i64>("", "revision")?)
            .map_err(|_| LiveError::Conflict)?,
        as_of,
        utc_date: as_of.date_naive(),
        admitted_today: u16::try_from(row.try_get::<i64>("", "today")?)
            .map_err(|_| LiveError::Conflict)?,
        last_admitted_at: last,
        deduplicated: false,
        awaiting_response: awaiting,
    })
}
