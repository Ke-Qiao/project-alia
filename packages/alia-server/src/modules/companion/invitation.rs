//! Companion-owned immutable grants and current live policy admission.
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, Statement};
use serde_json::Value;
use uuid::Uuid;

use super::{
    decision::canonical_digest,
    live::types::{INVITATION_OUTPUT_KEY, LiveAdmissionGrant},
    types::{CompanionMode, CompanionPolicy, PolicySnapshot as CompanionPolicySnapshot},
};
use crate::modules::memory::context::{ContextError, ContextLease, ContextResult};

fn revoked() -> anyhow::Error {
    ContextError::Revoked("Companion invitation authority is unavailable or changed".into()).into()
}

/// Retirement erases live grants, but cannot rewrite the identity of a committed
/// invitation. This check grants neither a new admission nor output permission.
pub(crate) async fn read_history_identity(
    tx: &DatabaseTransaction,
    lease: &ContextLease,
    admission_id: Uuid,
) -> ContextResult<Uuid> {
    let source = lease.source();
    let row = tx
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT candidate_id,initiative_request_id,context_id,conversation_id,output_key,valid_until FROM companion_admissions WHERE admission_id=$1 AND owner_id=$2",
            vec![admission_id.into(), source.owner_id.into()],
        ))
        .await?
        .ok_or_else(revoked)?;
    if row.try_get::<Uuid>("", "initiative_request_id")? != source.request_id
        || row.try_get::<Uuid>("", "context_id")? != lease.id()
        || Some(row.try_get::<Uuid>("", "conversation_id")?) != source.conversation_id
        || row.try_get::<String>("", "output_key")? != INVITATION_OUTPUT_KEY
        || Some(
            row.try_get::<DateTime<Utc>>("", "valid_until")?
                .timestamp_micros(),
        ) != lease.valid_until().map(|value| value.timestamp_micros())
    {
        return Err(revoked());
    }
    Ok(row.try_get("", "candidate_id")?)
}

async fn grant(
    tx: &DatabaseTransaction,
    owner: Uuid,
    admission_id: Uuid,
    lock: bool,
) -> ContextResult<(LiveAdmissionGrant, String)> {
    let query = if lock {
        "SELECT * FROM companion_admissions WHERE admission_id=$1 AND owner_id=$2 AND state='active' FOR SHARE"
    } else {
        "SELECT * FROM companion_admissions WHERE admission_id=$1 AND owner_id=$2 AND state='active'"
    };
    let row = tx
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            query,
            vec![admission_id.into(), owner.into()],
        ))
        .await?
        .ok_or_else(revoked)?;
    let value = row
        .try_get::<Option<Value>>("", "private_grant")?
        .ok_or_else(revoked)?;
    let stored_digest = row
        .try_get::<Option<String>>("", "grant_digest")?
        .ok_or_else(revoked)?;
    if serde_json::to_vec(&value)?.len() > 64 * 1024 || canonical_digest(&value)? != stored_digest {
        return Err(revoked());
    }
    let grant: LiveAdmissionGrant = serde_json::from_value(value).map_err(|_| revoked())?;
    grant.validate().map_err(|_| revoked())?;
    let charged_at: DateTime<Utc> = row.try_get("", "charged_at")?;
    if grant.digest()? != stored_digest
        || grant.admission_id != admission_id
        || grant.candidate.observation.owner_id != owner
        || row.try_get::<Uuid>("", "candidate_id")? != grant.candidate.id
        || row.try_get::<Uuid>("", "initiative_request_id")? != grant.initiative_request_id
        || row.try_get::<Option<Uuid>>("", "context_id")? != Some(grant.context_id)
        || row.try_get::<Uuid>("", "server_instance_id")? != grant.server_instance_id
        || row.try_get::<Uuid>("", "opportunity_id")? != grant.opportunity.id
        || row.try_get::<Uuid>("", "conversation_id")?
            != grant.candidate.observation.conversation_id
        || row
            .try_get::<Option<String>>("", "intent_digest")?
            .as_deref()
            != Some(grant.intent_digest.as_str())
        || row.try_get::<String>("", "output_key")? != INVITATION_OUTPUT_KEY
        || row.try_get::<DateTime<Utc>>("", "admit_before")? != grant.admit_before
        || row.try_get::<DateTime<Utc>>("", "valid_until")? != grant.valid_until
        || u64::try_from(row.try_get::<i64>("", "budget_revision")?).ok()
            != grant.input.budget.revision.checked_add(1)
        || charged_at != grant.input.as_of
        || charged_at >= grant.admit_before
        || row.try_get::<chrono::NaiveDate>("", "budget_date")? != charged_at.date_naive()
        || grant.initiative_request_id == grant.candidate.observation.request_id
        || grant.context_id == grant.candidate.observation.context_id
    {
        return Err(revoked());
    }
    Ok((grant, stored_digest))
}

/// Read immutable binding facts without locking the grant. The original Context
/// must still be validated in the transaction; this read grants no output permission.
pub(crate) async fn read_invitation_grant(
    tx: &DatabaseTransaction,
    owner: Uuid,
    admission_id: Uuid,
) -> ContextResult<LiveAdmissionGrant> {
    Ok(grant(tx, owner, admission_id, false).await?.0)
}

/// The caller must first lock and validate the original canonical source closure.
/// The returned grant remains bound to this transaction; it is not an output permit.
pub(crate) async fn lock_invitation_grant(
    tx: &DatabaseTransaction,
    owner: Uuid,
    admission_id: Uuid,
    initial: &LiveAdmissionGrant,
) -> ContextResult<(LiveAdmissionGrant, String)> {
    let row = tx
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT revision,policy,policy_digest FROM companion_policies WHERE owner_id=$1 FOR SHARE",
            vec![owner.into()],
        ))
        .await?
        .ok_or_else(revoked)?;
    let policy: CompanionPolicy =
        serde_json::from_value(row.try_get("", "policy")?).map_err(|_| revoked())?;
    let current = CompanionPolicySnapshot {
        revision: row.try_get("", "revision")?,
        policy,
    };
    current.validate().map_err(|_| revoked())?;
    if current != initial.candidate.policy
        || current.policy.digest()? != row.try_get::<String>("", "policy_digest")?
        || current.policy.mode != CompanionMode::Enabled
        || current.policy.quiet
        || current.policy.paused
    {
        return Err(revoked());
    }
    let (locked, locked_digest) = grant(tx, owner, admission_id, true).await?;
    if locked_digest != initial.digest()? || locked != *initial {
        return Err(revoked());
    }
    Ok((locked, locked_digest))
}
