//! Bounded PostgreSQL shadow admission. A transaction owns both canonical reads
//! and its immutable decision receipt; no model, sensor or output is invoked.
use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, QueryResult,
    Statement, TransactionTrait, Value,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{
    decision::{canonical_digest, decide, generate_candidate, replay as replay_decision},
    types::*,
};
use crate::modules::memory::context::{
    ContextError,
    companion::{CompanionAuthority, load_companion_source, lock_companion_authority},
};

const MAX_OBSERVATIONS: usize = 32;
const MAX_CANDIDATES: usize = 4;
const MAX_SNAPSHOT_BYTES: usize = 128 * 1024;
const MAX_PRIVATE_PER_OWNER: i64 = 128;
const MAX_PRIVATE_TOTAL: i64 = 1024;
const PRIVATE_CAPACITY_LOCK: i64 = 0x414c_434f_4d50_4350;

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("invalid Companion input: {0}")]
    InvalidInput(String),
    #[error("Companion operation or policy identity changed")]
    Conflict,
    #[error("Companion access is not authorized")]
    Forbidden,
    #[error("Companion replay is unavailable because its source was erased or expired")]
    Erased,
    #[error("Companion storage is unavailable: {0}")]
    Unavailable(String),
}
pub type RepoResult<T> = Result<T, RepoError>;

impl From<sea_orm::DbErr> for RepoError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Unavailable(error.to_string())
    }
}
impl From<anyhow::Error> for RepoError {
    fn from(error: anyhow::Error) -> Self {
        match error.downcast_ref::<ContextError>() {
            Some(ContextError::Forbidden) => Self::Forbidden,
            Some(ContextError::PolicyChanged) => Self::Conflict,
            Some(ContextError::Revoked(_)) => Self::Erased,
            _ => Self::Unavailable(error.to_string()),
        }
    }
}
impl From<serde_json::Error> for RepoError {
    fn from(error: serde_json::Error) -> Self {
        Self::Unavailable(error.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowDecision {
    pub input: DecisionInput,
    pub decision: DecisionRecord,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowReport {
    pub operation_id: Uuid,
    pub as_of: DateTime<Utc>,
    pub policy_revision: i64,
    pub examined: usize,
    pub limited: usize,
    pub decisions: Vec<ShadowDecision>,
}
#[derive(Debug, Clone)]
pub struct PendingObservation {
    pub owner_id: Uuid,
    pub appraisal_id: Uuid,
    pub channel_id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredShadow {
    observations: Vec<CanonicalObservation>,
    report: ShadowReport,
}

#[derive(Clone)]
pub struct CompanionRepository {
    db: DatabaseConnection,
}

impl CompanionRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    async fn transaction(&self) -> RepoResult<DatabaseTransaction> {
        let tx = self.db.begin().await?;
        tx.execute(sql("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        Ok(tx)
    }

    pub async fn policy(&self, owner: Uuid) -> RepoResult<PolicySnapshot> {
        let tx = self.transaction().await?;
        lock_companion_authority(&tx, owner).await?;
        let result = locked_policy(&tx, owner).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn replace_policy(
        &self,
        owner: Uuid,
        expected_revision: i64,
        policy: CompanionPolicy,
    ) -> RepoResult<PolicySnapshot> {
        policy
            .validate()
            .map_err(|e| RepoError::InvalidInput(e.to_string()))?;
        if expected_revision <= 0 {
            return Err(RepoError::InvalidInput(
                "positive policy revision required".into(),
            ));
        }
        // Enabling delivery requires the separate live-admission integration.
        if policy.mode == CompanionMode::Enabled {
            return Err(RepoError::Unavailable(
                "live Companion admission is not installed".into(),
            ));
        }
        let tx = self.transaction().await?;
        lock_companion_authority(&tx, owner).await?;
        let old = locked_policy(&tx, owner).await?;
        if old.revision != expected_revision {
            return Err(RepoError::Conflict);
        }
        let revision = old
            .revision
            .checked_add(1)
            .ok_or_else(|| RepoError::Unavailable("policy revision exhausted".into()))?;
        tx.execute(sql("UPDATE companion_policies SET revision=$2,policy=$3,policy_digest=$4,updated_at=clock_timestamp() WHERE owner_id=$1", vec![owner.into(), revision.into(), json!(policy).into(), canonical_digest(&policy)?.into()])).await?;
        tx.commit().await?;
        Ok(PolicySnapshot { revision, policy })
    }

    pub async fn evaluate(
        &self,
        owner: Uuid,
        operation_id: Uuid,
        appraisal_ids: Vec<Uuid>,
    ) -> RepoResult<ShadowReport> {
        self.evaluate_inner(owner, operation_id, appraisal_ids, None)
            .await
    }

    /// Only a trusted runtime may provide an opportunity. The HTTP request has
    /// no field for this snapshot, and it still cannot authorize live output.
    /// Absence is frozen at the transaction's database time, not a host clock.
    pub(crate) async fn evaluate_with_opportunity(
        &self,
        owner: Uuid,
        operation_id: Uuid,
        appraisal_ids: Vec<Uuid>,
        opportunity: Option<OpportunitySnapshot>,
    ) -> RepoResult<ShadowReport> {
        self.evaluate_inner(owner, operation_id, appraisal_ids, opportunity)
            .await
    }

    async fn evaluate_inner(
        &self,
        owner: Uuid,
        operation_id: Uuid,
        mut appraisal_ids: Vec<Uuid>,
        opportunity: Option<OpportunitySnapshot>,
    ) -> RepoResult<ShadowReport> {
        if owner.is_nil()
            || operation_id.is_nil()
            || appraisal_ids.is_empty()
            || appraisal_ids.len() > MAX_OBSERVATIONS
            || appraisal_ids.iter().any(Uuid::is_nil)
        {
            return Err(RepoError::InvalidInput(
                "nonempty identities and 1..32 Appraisal IDs required".into(),
            ));
        }
        appraisal_ids.sort_unstable();
        if appraisal_ids.windows(2).any(|ids| ids[0] == ids[1]) {
            return Err(RepoError::InvalidInput("duplicate Appraisal ID".into()));
        }
        let request_digest =
            canonical_digest(&json!({"owner":owner,"appraisal_ids":appraisal_ids}))?;
        let tx = self.transaction().await?;
        let authority = lock_companion_authority(&tx, owner).await?;
        let policy = locked_policy(&tx, owner).await?;
        tx.execute(sql(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 927614))",
            vec![operation_id.to_string().into()],
        ))
        .await?;
        if let Some(row) = operation(&tx, operation_id).await? {
            if row.try_get::<Uuid>("", "owner_id")? != owner
                || row.try_get::<String>("", "request_digest")? != request_digest
            {
                return Err(RepoError::Conflict);
            }
            return finish_read(tx, row, owner, &authority, &policy).await;
        }
        if !authority.private_readable {
            return Err(RepoError::Forbidden);
        }
        if policy.policy.mode == CompanionMode::Enabled {
            return Err(RepoError::Unavailable(
                "live Companion admission is not installed".into(),
            ));
        }
        // The owner's privacy lock serializes competing workers and CAS with
        // erasure. All facts and decisions commit before that lock is released.
        let mut sources = Vec::with_capacity(appraisal_ids.len());
        let mut roots = BTreeSet::new();
        for id in &appraisal_ids {
            let source = match load_companion_source(&tx, owner, *id, &authority).await {
                Ok(source) => source,
                Err(error)
                    if matches!(
                        error.downcast_ref::<ContextError>(),
                        Some(ContextError::Revoked(_) | ContextError::PolicyChanged)
                    ) =>
                {
                    // Retirement only names a real canonical row owned by this
                    // caller. No stale bytes or replacement authority survive.
                    tx.execute(sql("INSERT INTO companion_rejected_sources(owner_id,appraisal_id) SELECT owner_id,appraisal_id FROM appraisal_records WHERE owner_id=$1 AND appraisal_id=$2 ON CONFLICT DO NOTHING", vec![owner.into(), (*id).into()])).await?;
                    tx.commit().await?;
                    return Err(RepoError::Erased);
                }
                Err(error) => return Err(error.into()),
            };
            roots.extend(source.erasure_roots);
            if roots.len() > crate::modules::memory::context::MAX_EVIDENCE_REFS {
                return Err(RepoError::InvalidInput(
                    "combined source roots exceed the evidence bound".into(),
                ));
            }
            sources.push(source.observation);
        }
        sources.sort_by_key(|source| {
            (
                source.effective_at,
                source.source_event_id,
                source.appraisals[0].appraisal_id,
            )
        });
        // Shared immutable origin also covers observations limited out of this
        // shadow batch; no later live policy can promote the same source.
        for observation in &sources {
            tx.execute(sql("INSERT INTO companion_source_claims(owner_id,source_event_id,kind,generator_version,origin_mode,policy_revision) VALUES($1,$2,'follow_up_invitation',$3,'shadow',$4) ON CONFLICT DO NOTHING", vec![owner.into(), observation.source_event_id.into(), GENERATOR_VERSION.into(), policy.revision.into()])).await?;
            let claim = tx.query_one(sql("SELECT origin_mode FROM companion_source_claims WHERE owner_id=$1 AND source_event_id=$2 AND kind='follow_up_invitation' AND generator_version=$3", vec![owner.into(), observation.source_event_id.into(), GENERATOR_VERSION.into()])).await?.unwrap();
            if claim.try_get::<String>("", "origin_mode")? != "shadow" {
                return Err(RepoError::Conflict);
            }
        }
        let opportunity =
            opportunity.unwrap_or_else(|| OpportunitySnapshot::unavailable(authority.as_of));
        let mut decisions = Vec::new();
        let mut new_candidates = Vec::new();
        let mut batch_keys = BTreeSet::new();
        for observation in sources.iter().take(MAX_CANDIDATES) {
            let candidate = generate_candidate(observation, &policy, observation.effective_at)?;
            let mut budget = BudgetSnapshot::empty(authority.as_of);
            budget.deduplicated = !batch_keys.insert(candidate.dedup_key.clone())
                || tx.query_one(sql("SELECT candidate_id FROM companion_candidates WHERE owner_id=$1 AND dedup_key=$2", vec![owner.into(), candidate.dedup_key.clone().into()])).await?.is_some();
            if !budget.deduplicated {
                new_candidates.push(candidate.clone());
            }
            let input = DecisionInput {
                candidate,
                current_policy: policy.clone(),
                as_of: authority.as_of,
                opportunity: opportunity.clone(),
                budget,
            };
            let decision = decide(&input)?;
            if decision.outcome == DecisionOutcome::Allowed {
                return Err(RepoError::Unavailable(
                    "shadow repository cannot issue a live decision".into(),
                ));
            }
            decisions.push(ShadowDecision { input, decision });
        }
        let report = ShadowReport {
            operation_id,
            as_of: authority.as_of,
            policy_revision: policy.revision,
            examined: sources.len(),
            limited: sources.len().saturating_sub(MAX_CANDIDATES),
            decisions,
        };
        let stored = StoredShadow {
            observations: sources,
            report: report.clone(),
        };
        let private_until = stored
            .observations
            .iter()
            .map(|source| {
                (source.source_received_at + chrono::Duration::hours(24)).min(source.valid_until)
            })
            .min()
            .ok_or_else(|| RepoError::InvalidInput("empty shadow worklist".into()))?;
        if private_until <= authority.as_of {
            return Err(RepoError::Erased);
        }
        if serde_json::to_vec(&stored)?.len() > MAX_SNAPSHOT_BYTES {
            return Err(RepoError::InvalidInput(
                "shadow snapshot exceeds byte budget".into(),
            ));
        }
        let value = json!(stored);
        // The owner authority lock already serializes its quota. A short global
        // lock also bounds different owners committing at the same time.
        tx.execute(sql(
            "SELECT pg_advisory_xact_lock($1)",
            vec![PRIVATE_CAPACITY_LOCK.into()],
        ))
        .await?;
        let capacity = tx.query_one(sql("SELECT COUNT(*)::bigint AS total,COUNT(*) FILTER(WHERE owner_id=$1)::bigint AS owned FROM companion_operations WHERE state='completed'", vec![owner.into()])).await?.unwrap();
        if capacity.try_get::<i64>("", "total")? >= MAX_PRIVATE_TOTAL
            || capacity.try_get::<i64>("", "owned")? >= MAX_PRIVATE_PER_OWNER
        {
            return Err(RepoError::Unavailable(
                "private shadow receipt capacity reached".into(),
            ));
        }
        tx.execute(sql("INSERT INTO companion_operations(operation_id,owner_id,request_digest,policy_revision,context_policy_version,privacy_epoch,relationship_epoch,as_of,state,private_snapshot,snapshot_digest,private_until) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'completed',$9,$10,$11)", vec![operation_id.into(), owner.into(), request_digest.into(), policy.revision.into(), authority.context_policy_version.into(), authority.privacy_epoch.into(), authority.relationship_epoch.into(), authority.as_of.into(), value.into(), canonical_digest(&stored)?.into(), private_until.into()])).await?;
        for candidate in new_candidates {
            tx.execute(sql("INSERT INTO companion_candidates(candidate_id,owner_id,operation_id,dedup_key,private_candidate) VALUES($1,$2,$3,$4,$5)", vec![candidate.id.into(), owner.into(), operation_id.into(), candidate.dedup_key.clone().into(), json!(candidate).into()])).await?;
        }
        for observation in &stored.observations {
            tx.execute(sql("INSERT INTO companion_processed_sources(owner_id,appraisal_id,source_event_id,operation_id) VALUES($1,$2,$3,$4) ON CONFLICT(owner_id,appraisal_id) DO NOTHING", vec![owner.into(), observation.appraisals[0].appraisal_id.into(), observation.source_event_id.into(), operation_id.into()])).await?;
        }
        for (kind, id) in roots {
            tx.execute(sql(
                "INSERT INTO companion_sources(operation_id,kind,source_id) VALUES($1,$2,$3)",
                vec![operation_id.into(), kind.into(), id.into()],
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(report)
    }

    pub async fn replay(&self, owner: Uuid, operation_id: Uuid) -> RepoResult<ShadowReport> {
        if operation_id.is_nil() {
            return Err(RepoError::InvalidInput("operation ID required".into()));
        }
        let tx = self.transaction().await?;
        let authority = lock_companion_authority(&tx, owner).await?;
        let policy = locked_policy(&tx, owner).await?;
        let row = operation(&tx, operation_id)
            .await?
            .ok_or(RepoError::Forbidden)?;
        if row.try_get::<Uuid>("", "owner_id")? != owner {
            return Err(RepoError::Forbidden);
        }
        finish_read(tx, row, owner, &authority, &policy).await
    }

    pub async fn discovery_pending(&self, limit: u16) -> RepoResult<Vec<PendingObservation>> {
        if !(1..=32).contains(&limit) {
            return Err(RepoError::InvalidInput(
                "discovery limit must be 1..32".into(),
            ));
        }
        let tx = self.transaction().await?;
        let rows = tx.query_all(sql("SELECT a.owner_id,a.appraisal_id,c.channel_id FROM appraisal_records a JOIN context_leases c ON c.context_id=a.context_id JOIN users u ON u.id=a.owner_id AND u.status='active' JOIN memory_privacy_state p ON p.user_id=a.owner_id AND p.privacy_epoch=a.source_privacy_epoch JOIN relationship_authority r ON r.owner_id=a.owner_id AND r.relationship_epoch=a.source_relationship_epoch JOIN context_policy g ON g.singleton AND g.version=a.policy_version LEFT JOIN companion_policies cp ON cp.owner_id=a.owner_id WHERE a.status='active' AND a.source_fact->>'kind'='user_message' AND a.valid_until>clock_timestamp() AND a.effective_at<=clock_timestamp() AND a.effective_at+(COALESCE((cp.policy->>'candidate_ttl_seconds')::integer,60)*interval '1 second')>clock_timestamp() AND COALESCE(cp.policy->>'mode','shadow')='shadow' AND (g.policy->'appraisal'->'affect'->>'enabled')::boolean AND (g.policy->'appraisal'->'affect'->>'relationship_profile_user_visible')::boolean AND g.policy->>'paused_until' IS NULL AND NOT EXISTS(SELECT 1 FROM companion_processed_sources s WHERE s.owner_id=a.owner_id AND (s.appraisal_id=a.appraisal_id OR s.source_event_id=a.source_event_id)) AND NOT EXISTS(SELECT 1 FROM companion_rejected_sources s WHERE s.owner_id=a.owner_id AND s.appraisal_id=a.appraisal_id) AND NOT EXISTS(SELECT 1 FROM companion_source_claims s WHERE s.owner_id=a.owner_id AND s.source_event_id=a.source_event_id) ORDER BY a.effective_at,a.appraisal_id LIMIT $1", vec![i64::from(limit).into()])).await?;
        tx.commit().await?;
        rows.into_iter()
            .map(|row| {
                Ok(PendingObservation {
                    owner_id: row.try_get("", "owner_id")?,
                    appraisal_id: row.try_get("", "appraisal_id")?,
                    channel_id: row.try_get("", "channel_id")?,
                })
            })
            .collect()
    }

    /// Bounded storage erasure only: this creates no authority or fresh work.
    /// SKIP LOCKED avoids waiting behind an in-flight source/owner erasure.
    pub async fn prune_expired(&self, limit: u16) -> RepoResult<u64> {
        if !(1..=128).contains(&limit) {
            return Err(RepoError::InvalidInput("prune limit must be 1..128".into()));
        }
        let tx = self.transaction().await?;
        let rows = tx.query_all(sql("SELECT operation_id FROM companion_operations WHERE state='completed' AND private_until<=clock_timestamp() ORDER BY operation_id LIMIT $1 FOR UPDATE SKIP LOCKED", vec![i64::from(limit).into()])).await?;
        for row in &rows {
            let id: Uuid = row.try_get("", "operation_id")?;
            tx.execute(sql(
                "SELECT erase_companion_operations(ARRAY[$1::uuid])",
                vec![id.into()],
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(rows.len() as u64)
    }
}

pub(super) async fn locked_policy(
    tx: &DatabaseTransaction,
    owner: Uuid,
) -> RepoResult<PolicySnapshot> {
    let seed = CompanionPolicy::default();
    tx.execute(sql("INSERT INTO companion_policies(owner_id,policy,policy_digest) VALUES($1,$2,$3) ON CONFLICT DO NOTHING", vec![owner.into(), json!(seed).into(), canonical_digest(&seed)?.into()])).await?;
    let row = tx.query_one(sql("SELECT revision,policy,policy_digest FROM companion_policies WHERE owner_id=$1 FOR UPDATE", vec![owner.into()])).await?.ok_or(RepoError::Forbidden)?;
    let policy: CompanionPolicy = serde_json::from_value(row.try_get("", "policy")?)?;
    policy.validate()?;
    if canonical_digest(&policy)? != row.try_get::<String>("", "policy_digest")? {
        return Err(RepoError::Unavailable("policy digest mismatch".into()));
    }
    Ok(PolicySnapshot {
        revision: row.try_get("", "revision")?,
        policy,
    })
}

async fn operation(tx: &DatabaseTransaction, id: Uuid) -> RepoResult<Option<QueryResult>> {
    // Source mutations erase operations while holding their source row lock.
    // Do not reverse that order by locking this receipt before its sources.
    Ok(tx
        .query_one(sql(
            "SELECT * FROM companion_operations WHERE operation_id=$1",
            vec![id.into()],
        ))
        .await?)
}

async fn finish_read(
    tx: DatabaseTransaction,
    row: QueryResult,
    owner: Uuid,
    authority: &CompanionAuthority,
    policy: &PolicySnapshot,
) -> RepoResult<ShadowReport> {
    if !authority.private_readable {
        return Err(RepoError::Forbidden);
    }
    if row.try_get::<String>("", "state")? == "erased" {
        return Err(RepoError::Erased);
    }
    let private_until: DateTime<Utc> = row.try_get("", "private_until")?;
    if private_until <= authority.as_of {
        return erase_read(tx, row.try_get("", "operation_id")?).await;
    }
    if row.try_get::<i64>("", "policy_revision")? != policy.revision
        || row.try_get::<i64>("", "context_policy_version")? != authority.context_policy_version
        || row.try_get::<i64>("", "privacy_epoch")? != authority.privacy_epoch
        || row.try_get::<i64>("", "relationship_epoch")? != authority.relationship_epoch
    {
        return erase_read(tx, row.try_get("", "operation_id")?).await;
    }
    let value: serde_json::Value = row.try_get("", "private_snapshot")?;
    let stored: StoredShadow = serde_json::from_value(value)?;
    if canonical_digest(&stored)? != row.try_get::<String>("", "snapshot_digest")?
        || stored.observations.len() > MAX_OBSERVATIONS
        || stored.report.decisions.len() > MAX_CANDIDATES
    {
        return Err(RepoError::Unavailable(
            "shadow receipt digest or bounds changed".into(),
        ));
    }
    for observation in &stored.observations {
        let Some(reference) = observation.appraisals.first() else {
            return Err(RepoError::Unavailable("shadow source pin missing".into()));
        };
        match load_companion_source(&tx, owner, reference.appraisal_id, authority).await {
            Ok(current)
                if canonical_digest(&current.observation)? == canonical_digest(observation)? =>
            {
                ()
            }
            Ok(_) => return erase_read(tx, stored.report.operation_id).await,
            Err(error)
                if matches!(
                    error.downcast_ref::<ContextError>(),
                    Some(ContextError::Revoked(_) | ContextError::PolicyChanged)
                ) =>
            {
                return erase_read(tx, stored.report.operation_id).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    for entry in &stored.report.decisions {
        if !replay_decision(&entry.input, &entry.decision)? {
            return Err(RepoError::Unavailable(
                "deterministic replay differs from its receipt".into(),
            ));
        }
    }
    let still_live: bool = tx
        .query_one(sql(
            "SELECT $1::timestamptz>clock_timestamp() AS live",
            vec![private_until.into()],
        ))
        .await?
        .unwrap()
        .try_get("", "live")?;
    if !still_live {
        return erase_read(tx, stored.report.operation_id).await;
    }
    tx.commit().await?;
    Ok(stored.report)
}

async fn erase_read(tx: DatabaseTransaction, operation: Uuid) -> RepoResult<ShadowReport> {
    tx.execute(sql(
        "SELECT erase_companion_operations(ARRAY[$1::uuid])",
        vec![operation.into()],
    ))
    .await?;
    tx.commit().await?;
    Err(RepoError::Erased)
}

fn sql(text: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, text, values)
}

#[cfg(test)]
pub(crate) mod tests;
