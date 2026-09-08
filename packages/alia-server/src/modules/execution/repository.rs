//! Transactional effect facts and their dispatch intent. No external executor
//! runs inside these transactions, and dispatched work is never re-enqueued.
use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, QueryResult,
    Statement, TransactionTrait, Value as SqlValue,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::types::*;

const SELECT_EFFECT: &str = "SELECT to_jsonb(e) || jsonb_build_object('key', jsonb_build_object('user_id', e.user_id, 'request_id', e.request_id, 'invocation_key', e.invocation_key)) AS record FROM execution_effects e ";
const QUEUE_LOCK: i64 = 0x414c_4941_4558_4543;
pub const RECOVERY_GRACE_SECONDS: i64 = 10;

#[derive(Clone)]
pub struct ExecutionRepository {
    db: DatabaseConnection,
}

impl ExecutionRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn propose(&self, input: NewEffect) -> ExecutionResult<EffectRecord> {
        validate_input(&input)?;
        let tx = self.db.begin().await?;
        // Serializes this owner's proposal admission; it does not lock another
        // owner's operations while checking the per-owner quota.
        tx.execute(stmt(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 814774))",
            vec![input.key.user_id.to_string().into()],
        ))
        .await?;
        let catalog = authorize(
            &tx,
            input.key.user_id,
            &input.tool_name,
            input.required_role.as_deref(),
        )
        .await?;
        let catalog_digest = digest(&catalog)?;
        // Retransmission can observe the same invocation later or carry a new
        // model label. Neither changes intent, nor replaces its first source time.
        let mut intent = serde_json::to_value(&input)?;
        if let Value::Object(fields) = &mut intent {
            fields.remove("model_call_id");
            fields.remove("source_received_at");
        }
        let intent_digest = digest(&json!({"request": intent, "catalog_digest": catalog_digest}))?;
        if let Some(existing) = load(
            &tx,
            "WHERE e.user_id=$1 AND e.request_id=$2 AND e.invocation_key=$3 FOR UPDATE OF e",
            vec![
                input.key.user_id.into(),
                input.key.request_id.into(),
                input.key.invocation_key.clone().into(),
            ],
        )
        .await?
        {
            if existing.intent_digest != intent_digest {
                return Err(ExecutionError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(existing);
        }
        if count(&tx, "SELECT COUNT(*)::bigint AS count FROM execution_effects WHERE user_id=$1 AND status='proposed'", vec![input.key.user_id.into()]).await? >= MAX_PROPOSED_PER_USER {
            return Err(ExecutionError::CapacityExceeded);
        }
        let effect_id = Uuid::new_v4();
        tx.execute(stmt("INSERT INTO execution_effects(effect_id,user_id,request_id,invocation_key,model_call_id,source,conversation_id,channel_id,tool_name,arguments,tool_digest,policy_digest,catalog_digest,intent_digest,requires_confirmation,required_role,source_received_at,source_event_id,status) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,'proposed')", vec![
            effect_id.into(), input.key.user_id.into(), input.key.request_id.into(), input.key.invocation_key.into(), input.model_call_id.into(), input.source.into(), input.conversation_id.into(), input.channel_id.into(), input.tool_name.into(), input.arguments.into(), input.tool_digest.into(), input.policy_digest.into(), catalog_digest.into(), intent_digest.into(), (input.requires_confirmation || catalog["require_confirmation"] == true).into(), input.required_role.into(), input.source_received_at.into(), input.source_event_id.into(),
        ])).await?;
        audit(
            &tx,
            effect_id,
            "proposed",
            Some(input.key.user_id),
            json!({}),
        )
        .await?;
        let record = required(&tx, effect_id).await?;
        tx.commit().await?;
        Ok(record)
    }

    pub async fn approve(
        &self,
        owner: Uuid,
        id: Uuid,
        approval: Approval,
    ) -> ExecutionResult<EffectRecord> {
        if owner != approval.actor_id {
            return Err(ExecutionError::Unauthorized(
                "approval actor must own the effect".into(),
            ));
        }
        let tx = self.db.begin().await?;
        let record = owned_locked(&tx, owner, id).await?;
        authorize(
            &tx,
            owner,
            &record.tool_name,
            record.required_role.as_deref(),
        )
        .await?;
        if record.intent_digest != approval.expected_intent_digest {
            return Err(ExecutionError::IdempotencyConflict);
        }
        if record.status != EffectStatus::Proposed {
            if record.approved_by == Some(owner) && record.status != EffectStatus::Cancelled {
                tx.commit().await?;
                return Ok(record);
            }
            return Err(ExecutionError::InvalidTransition(
                "only proposed effects can be approved".into(),
            ));
        }
        validate_catalog(&tx, &record).await?;
        if approval.kind == ApprovalKind::Automatic && record.requires_confirmation {
            return Err(ExecutionError::Unauthorized(
                "explicit confirmation required".into(),
            ));
        }
        tx.execute(stmt(
            "SELECT pg_advisory_xact_lock($1)",
            vec![QUEUE_LOCK.into()],
        ))
        .await?;
        if count(
            &tx,
            "SELECT COUNT(*)::bigint AS count FROM execution_outbox",
            vec![],
        )
        .await?
            >= MAX_PENDING
        {
            return Err(ExecutionError::CapacityExceeded);
        }
        let kind = match approval.kind {
            ApprovalKind::Automatic => "automatic",
            ApprovalKind::Explicit => "explicit",
        };
        tx.execute(stmt("UPDATE execution_effects SET status='approved',approved_by=$2,approval_kind=$3,approved_at=clock_timestamp(),approval_expires_at=clock_timestamp()+($4::bigint*interval '1 second'),updated_at=clock_timestamp() WHERE effect_id=$1", vec![id.into(), owner.into(), kind.into(), (APPROVAL_VALIDITY.as_secs() as i64).into()])).await?;
        tx.execute(stmt(
            "INSERT INTO execution_outbox(effect_id) VALUES($1)",
            vec![id.into()],
        ))
        .await?;
        audit(
            &tx,
            id,
            "approved",
            Some(owner),
            json!({"kind":approval.kind,"intent_digest":record.intent_digest}),
        )
        .await?;
        let result = required(&tx, id).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn pending(&self, limit: u32) -> ExecutionResult<Vec<EffectRecord>> {
        load_many(&self.db, "JOIN execution_outbox o ON o.effect_id=e.effect_id WHERE e.status='approved' ORDER BY o.created_at,e.effect_id LIMIT $1", vec![(limit.clamp(1, MAX_PENDING as u32) as i64).into()]).await
    }

    pub async fn claim(&self, request: ClaimRequest) -> ExecutionResult<Option<ExecutionClaim>> {
        if request.worker_id.is_nil()
            || request.execution_timeout.is_zero()
            || request.execution_timeout > MAX_EXECUTION_TIMEOUT
        {
            return Err(ExecutionError::InvalidInput(
                "invalid worker identity or execution deadline".into(),
            ));
        }
        let tx = self.db.begin().await?;
        let Some(record) = load(
            &tx,
            "WHERE e.effect_id=$1 FOR UPDATE OF e",
            vec![request.effect_id.into()],
        )
        .await?
        else {
            return Ok(None);
        };
        if record.status != EffectStatus::Approved {
            return Ok(None);
        }
        let has_intent = tx
            .query_one(stmt(
                "SELECT effect_id FROM execution_outbox WHERE effect_id=$1",
                vec![record.effect_id.into()],
            ))
            .await?
            .is_some();
        if !has_intent {
            return Err(ExecutionError::InvalidTransition(
                "approved effect has no outbox intent".into(),
            ));
        }
        let now = db_time(&tx).await?;
        let preflight = async {
            validate_catalog(&tx, &record).await?;
            if record.tool_digest != request.expected_tool_digest
                || record.policy_digest != request.expected_policy_digest
            {
                return Err(ExecutionError::VersionChanged);
            }
            if record
                .approval_expires_at
                .is_none_or(|expiry| expiry <= now)
            {
                return Err(ExecutionError::Unauthorized("approval expired".into()));
            }
            Ok(())
        }
        .await;
        if let Err(error) = preflight {
            match error {
                ExecutionError::IdentityUnlinked
                | ExecutionError::Unauthorized(_)
                | ExecutionError::VersionChanged => {
                    reject_in_transaction(&tx, &record, &error.to_string()).await?;
                    tx.commit().await?;
                    return Ok(None);
                }
                other => return Err(other),
            }
        }
        let claim_id = Uuid::new_v4();
        let millis = i64::try_from(request.execution_timeout.as_millis())
            .map_err(|_| ExecutionError::InvalidInput("execution duration overflow".into()))?
            .max(1);
        tx.execute(stmt("UPDATE execution_effects SET status='dispatched',claim_id=$2,worker_id=$3,deadline_at=clock_timestamp()+($4::bigint*interval '1 millisecond'),updated_at=clock_timestamp() WHERE effect_id=$1", vec![record.effect_id.into(), claim_id.into(), request.worker_id.into(), millis.into()])).await?;
        tx.execute(stmt(
            "DELETE FROM execution_outbox WHERE effect_id=$1",
            vec![record.effect_id.into()],
        ))
        .await?;
        audit(
            &tx,
            record.effect_id,
            "dispatched",
            None,
            json!({"claim_id":claim_id,"worker_id":request.worker_id}),
        )
        .await?;
        let effect = required(&tx, record.effect_id).await?;
        let deadline_at = effect
            .deadline_at
            .ok_or_else(|| ExecutionError::InvalidTransition("dispatch has no deadline".into()))?;
        tx.commit().await?;
        Ok(Some(ExecutionClaim {
            effect,
            claim_id,
            deadline_at,
        }))
    }

    pub async fn complete(
        &self,
        claim_id: Uuid,
        outcome: Completion,
    ) -> ExecutionResult<EffectRecord> {
        let encoded = serde_json::to_value(&outcome)?;
        if serde_json::to_vec(&encoded)?.len() > MAX_RECEIPT_BYTES {
            return Err(ExecutionError::InvalidInput(
                "execution receipt exceeds 64 KiB".into(),
            ));
        }
        let tx = self.db.begin().await?;
        let record = load(
            &tx,
            "WHERE e.claim_id=$1 FOR UPDATE OF e",
            vec![claim_id.into()],
        )
        .await?
        .ok_or(ExecutionError::NotFound)?;
        if record.status != EffectStatus::Dispatched {
            if record.completion.as_ref() == Some(&outcome) {
                tx.commit().await?;
                return Ok(record);
            }
            return Err(ExecutionError::CompletionConflict);
        }
        let status = match outcome {
            Completion::Succeeded { .. } => "succeeded",
            Completion::Failed { .. } => "failed",
            Completion::Unknown { .. } => "unknown",
        };
        tx.execute(stmt("UPDATE execution_effects SET status=$2,completion=$3,updated_at=clock_timestamp() WHERE effect_id=$1", vec![record.effect_id.into(), status.into(), encoded.clone().into()])).await?;
        audit(
            &tx,
            record.effect_id,
            status,
            None,
            json!({"claim_id":claim_id,"completion":encoded}),
        )
        .await?;
        let result = required(&tx, record.effect_id).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn cancel(&self, owner: Uuid, id: Uuid) -> ExecutionResult<EffectRecord> {
        let tx = self.db.begin().await?;
        let record = owned_locked(&tx, owner, id).await?;
        active_user(&tx, owner).await?;
        if record.status == EffectStatus::Cancelled {
            tx.commit().await?;
            return Ok(record);
        }
        if !matches!(
            record.status,
            EffectStatus::Proposed | EffectStatus::Approved
        ) {
            return Err(ExecutionError::InvalidTransition(
                "a dispatched effect cannot be cancelled by withdrawing its response".into(),
            ));
        }
        tx.execute(stmt(
            "DELETE FROM execution_outbox WHERE effect_id=$1",
            vec![id.into()],
        ))
        .await?;
        tx.execute(stmt("UPDATE execution_effects SET status='cancelled',updated_at=clock_timestamp() WHERE effect_id=$1", vec![id.into()])).await?;
        audit(&tx, id, "cancelled", Some(owner), json!({})).await?;
        let result = required(&tx, id).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn reject_undispatched(
        &self,
        id: Uuid,
        expected_intent_digest: &str,
        reason: &str,
    ) -> ExecutionResult<EffectRecord> {
        let tx = self.db.begin().await?;
        let record = load(&tx, "WHERE e.effect_id=$1 FOR UPDATE OF e", vec![id.into()])
            .await?
            .ok_or(ExecutionError::NotFound)?;
        if record.intent_digest != expected_intent_digest {
            return Err(ExecutionError::IdempotencyConflict);
        }
        if !matches!(
            record.status,
            EffectStatus::Proposed | EffectStatus::Approved
        ) {
            return Err(ExecutionError::InvalidTransition(
                "effect already dispatched or terminal".into(),
            ));
        }
        reject_in_transaction(&tx, &record, reason).await?;
        let result = required(&tx, id).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn recover_expired(&self, limit: u32) -> ExecutionResult<u64> {
        let tx = self.db.begin().await?;
        let expired = load_many(&tx, "WHERE e.status='dispatched' AND e.deadline_at < clock_timestamp()-($1::bigint*interval '1 second') ORDER BY e.deadline_at LIMIT $2 FOR UPDATE OF e SKIP LOCKED", vec![RECOVERY_GRACE_SECONDS.into(), (limit.clamp(1, MAX_PENDING as u32) as i64).into()]).await?;
        for effect in &expired {
            let completion = serde_json::to_value(Completion::Unknown {
                reason:
                    "dispatch deadline expired without a durable receipt; do not retry execution"
                        .into(),
            })?;
            tx.execute(stmt("UPDATE execution_effects SET status='unknown',completion=$2,updated_at=clock_timestamp() WHERE effect_id=$1", vec![effect.effect_id.into(), completion.clone().into()])).await?;
            audit(
                &tx,
                effect.effect_id,
                "recovered_unknown",
                None,
                json!({"claim_id":effect.claim_id,"completion":completion}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(expired.len() as u64)
    }

    pub async fn get(&self, owner: Uuid, id: Uuid) -> ExecutionResult<Option<EffectRecord>> {
        load(
            &self.db,
            "WHERE e.user_id=$1 AND e.effect_id=$2",
            vec![owner.into(), id.into()],
        )
        .await
    }

    pub async fn list(
        &self,
        owner: Uuid,
        before: Option<Uuid>,
        limit: u32,
    ) -> ExecutionResult<Vec<EffectRecord>> {
        let limit = (limit.clamp(1, 100) as i64).into();
        if let Some(before) = before {
            load_many(&self.db, "WHERE e.user_id=$1 AND (e.created_at,e.effect_id)<(SELECT p.created_at,p.effect_id FROM execution_effects p WHERE p.effect_id=$2 AND p.user_id=$1) ORDER BY e.created_at DESC,e.effect_id DESC LIMIT $3", vec![owner.into(), before.into(), limit]).await
        } else {
            load_many(
                &self.db,
                "WHERE e.user_id=$1 ORDER BY e.created_at DESC,e.effect_id DESC LIMIT $2",
                vec![owner.into(), limit],
            )
            .await
        }
    }
}

fn stmt(sql: impl Into<String>, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}
fn parse(row: QueryResult) -> ExecutionResult<EffectRecord> {
    Ok(serde_json::from_value(row.try_get("", "record")?)?)
}
async fn load<C: ConnectionTrait>(
    db: &C,
    predicate: &str,
    values: Vec<SqlValue>,
) -> ExecutionResult<Option<EffectRecord>> {
    db.query_one(stmt(format!("{SELECT_EFFECT}{predicate}"), values))
        .await?
        .map(parse)
        .transpose()
}
async fn load_many<C: ConnectionTrait>(
    db: &C,
    predicate: &str,
    values: Vec<SqlValue>,
) -> ExecutionResult<Vec<EffectRecord>> {
    db.query_all(stmt(format!("{SELECT_EFFECT}{predicate}"), values))
        .await?
        .into_iter()
        .map(parse)
        .collect()
}
async fn required(tx: &DatabaseTransaction, id: Uuid) -> ExecutionResult<EffectRecord> {
    load(tx, "WHERE e.effect_id=$1", vec![id.into()])
        .await?
        .ok_or(ExecutionError::NotFound)
}
async fn owned_locked(
    tx: &DatabaseTransaction,
    owner: Uuid,
    id: Uuid,
) -> ExecutionResult<EffectRecord> {
    load(
        tx,
        "WHERE e.effect_id=$1 AND e.user_id=$2 FOR UPDATE OF e",
        vec![id.into(), owner.into()],
    )
    .await?
    .ok_or(ExecutionError::NotFound)
}
async fn count(tx: &DatabaseTransaction, sql: &str, values: Vec<SqlValue>) -> ExecutionResult<u64> {
    let value: i64 = tx
        .query_one(stmt(sql, values))
        .await?
        .ok_or_else(|| ExecutionError::InvalidTransition("count returned no row".into()))?
        .try_get("", "count")?;
    Ok(value as u64)
}
async fn db_time(tx: &DatabaseTransaction) -> ExecutionResult<DateTime<Utc>> {
    Ok(tx
        .query_one(stmt("SELECT clock_timestamp() AS now", vec![]))
        .await?
        .ok_or_else(|| ExecutionError::InvalidTransition("database clock unavailable".into()))?
        .try_get("", "now")?)
}

pub fn digest(value: &impl Serialize) -> ExecutionResult<String> {
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
}

async fn active_user(tx: &DatabaseTransaction, id: Uuid) -> ExecutionResult<String> {
    let row = tx
        .query_one(stmt(
            "SELECT role,status FROM users WHERE id=$1 FOR SHARE",
            vec![id.into()],
        ))
        .await?
        .ok_or(ExecutionError::IdentityUnlinked)?;
    let status: String = row.try_get("", "status")?;
    if status != "active" {
        return Err(ExecutionError::Unauthorized("account is not active".into()));
    }
    Ok(row.try_get("", "role")?)
}

async fn authorize(
    tx: &DatabaseTransaction,
    owner: Uuid,
    tool: &str,
    required_role: Option<&str>,
) -> ExecutionResult<Value> {
    let role = active_user(tx, owner).await?;
    let row = tx.query_one(stmt("SELECT jsonb_build_object('name',name,'source',source,'parameters',parameters,'enabled',enabled,'require_confirmation',require_confirmation,'required_role',required_role) AS policy FROM tools WHERE name=$1 FOR SHARE", vec![tool.into()])).await?.ok_or_else(|| ExecutionError::Unauthorized("tool is not registered".into()))?;
    let catalog: Value = row.try_get("", "policy")?;
    if catalog["enabled"] != true {
        return Err(ExecutionError::Unauthorized("tool is disabled".into()));
    }
    for required in [required_role, catalog["required_role"].as_str()]
        .into_iter()
        .flatten()
    {
        if (required != "user" && required != "admin") || (required == "admin" && role != "admin") {
            return Err(ExecutionError::Unauthorized(
                "current role cannot execute this tool".into(),
            ));
        }
    }
    Ok(catalog)
}

async fn validate_catalog(tx: &DatabaseTransaction, record: &EffectRecord) -> ExecutionResult<()> {
    let catalog = authorize(
        tx,
        record.key.user_id,
        &record.tool_name,
        record.required_role.as_deref(),
    )
    .await?;
    if digest(&catalog)? != record.catalog_digest {
        return Err(ExecutionError::VersionChanged);
    }
    Ok(())
}

async fn audit(
    tx: &DatabaseTransaction,
    id: Uuid,
    event: &str,
    actor: Option<Uuid>,
    evidence: Value,
) -> ExecutionResult<()> {
    tx.execute(stmt("INSERT INTO execution_events(event_id,effect_id,event_type,actor_id,evidence) VALUES($1,$2,$3,$4,$5)", vec![Uuid::new_v4().into(), id.into(), event.into(), actor.into(), evidence.into()])).await?;
    Ok(())
}

async fn reject_in_transaction(
    tx: &DatabaseTransaction,
    record: &EffectRecord,
    reason: &str,
) -> ExecutionResult<()> {
    let reason: String = reason.chars().take(2048).collect();
    let completion = serde_json::to_value(Completion::Failed {
        evidence: json!({"execution_started":false,"reason":reason}),
    })?;
    tx.execute(stmt(
        "DELETE FROM execution_outbox WHERE effect_id=$1",
        vec![record.effect_id.into()],
    ))
    .await?;
    tx.execute(stmt("UPDATE execution_effects SET status='failed',completion=$2,updated_at=clock_timestamp() WHERE effect_id=$1", vec![record.effect_id.into(), completion.clone().into()])).await?;
    audit(
        tx,
        record.effect_id,
        "rejected_before_dispatch",
        None,
        completion,
    )
    .await
}

fn validate_input(input: &NewEffect) -> ExecutionResult<()> {
    let valid_digest =
        |value: &str| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    if input.key.user_id.is_nil() {
        return Err(ExecutionError::IdentityUnlinked);
    }
    if input.key.request_id.is_nil()
        || input.key.invocation_key.is_empty()
        || input.key.invocation_key.len() > 128
        || input.model_call_id.len() > 256
        || input.source.is_empty()
        || input.source.len() > 64
        || input.tool_name.is_empty()
        || input.tool_name.len() > 128
        || input.channel_id.as_ref().is_some_and(|id| id.len() > 256)
        || !valid_digest(&input.tool_digest)
        || !valid_digest(&input.policy_digest)
        || input
            .required_role
            .as_ref()
            .is_some_and(|role| role != "user" && role != "admin")
    {
        return Err(ExecutionError::InvalidInput(
            "invalid effect identity or policy metadata".into(),
        ));
    }
    if !input.arguments.is_object()
        || serde_json::to_vec(&input.arguments)?.len() > MAX_ARGUMENT_BYTES
    {
        return Err(ExecutionError::InvalidInput(
            "tool arguments must be an object of at most 16 KiB".into(),
        ));
    }
    Ok(())
}
