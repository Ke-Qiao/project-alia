//! Explicit, bounded reinterpretation of admitted user facts. A page keeps its
//! original policy, source set and expiry caps through retries and restarts.
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction, QueryResult, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{repository::*, *};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppraisalRebuildPage {
    pub operation_id: Uuid,
    pub policy_version: i64,
    pub relationship_epoch: i64,
    pub examined: usize,
    pub committed: usize,
    pub skipped: usize,
    pub pending: usize,
    pub next_after: Option<Uuid>,
    pub receipts: Vec<AppraisalReceipt>,
}

#[derive(Debug, Clone)]
pub(crate) struct RebuildClaim {
    owner: Uuid,
    operation: Uuid,
    source: Uuid,
    token: Uuid,
}

fn revoked(reason: &str) -> anyhow::Error {
    ContextError::Revoked(reason.into()).into()
}

fn request_id(operation: Uuid, source: Uuid) -> Uuid {
    let hash = Sha256::digest(format!("alia.appraisal.rebuild.v1:{operation}:{source}"));
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

async fn authority(
    tx: &DatabaseTransaction,
    owner: Uuid,
) -> ContextResult<(PolicySnapshot, i64, i64)> {
    let policy = policy::verify(tx, None, ContextUse::AppraisalWrite).await?;
    if tx
        .query_one(stmt(
            "SELECT id FROM users WHERE id=$1 AND status='active' FOR SHARE",
            vec![owner.into()],
        ))
        .await?
        .is_none()
    {
        return Err(ContextError::Forbidden.into());
    }
    tx.execute(stmt(
        "INSERT INTO memory_privacy_state(user_id) VALUES($1) ON CONFLICT DO NOTHING",
        vec![owner.into()],
    ))
    .await?;
    let privacy = tx
        .query_one(stmt(
            "SELECT privacy_epoch FROM memory_privacy_state WHERE user_id=$1 FOR UPDATE",
            vec![owner.into()],
        ))
        .await?
        .unwrap()
        .try_get("", "privacy_epoch")?;
    let relationship = appraisal::relationship_epoch(tx, owner).await?;
    Ok((policy, privacy, relationship))
}

fn verify_operation(
    row: &QueryResult,
    owner: Uuid,
    policy: i64,
    privacy: i64,
    relationship: i64,
) -> ContextResult<()> {
    if row.try_get::<Uuid>("", "owner_id")? != owner {
        return Err(ContextError::RelationshipOperationConflict {
            operation_id: row.try_get("", "operation_id")?,
        }
        .into());
    }
    if row.try_get::<i64>("", "policy_version")? != policy {
        return Err(ContextError::PolicyChanged.into());
    }
    if row.try_get::<bool>("", "revoked")?
        || row.try_get::<i64>("", "privacy_epoch")? != privacy
        || row.try_get::<i64>("", "relationship_epoch")? != relationship
    {
        return Err(revoked("rebuild authority was revoked"));
    }
    Ok(())
}

async fn page(
    tx: &DatabaseTransaction,
    operation: &QueryResult,
) -> ContextResult<AppraisalRebuildPage> {
    let id: Uuid = operation.try_get("", "operation_id")?;
    let rows = tx.query_all(stmt("SELECT source_event_id,status,receipt FROM appraisal_rebuild_items WHERE operation_id=$1 ORDER BY source_event_id", vec![id.into()])).await?;
    anyhow::ensure!(rows.len() <= 32, "rebuild worklist exceeds its bound");
    let mut report = AppraisalRebuildPage {
        operation_id: id,
        policy_version: operation.try_get("", "policy_version")?,
        relationship_epoch: operation.try_get("", "relationship_epoch")?,
        examined: rows.len(),
        committed: 0,
        skipped: 0,
        pending: 0,
        next_after: None,
        receipts: Vec::new(),
    };
    for row in rows {
        report.next_after = Some(row.try_get("", "source_event_id")?);
        match row.try_get::<String>("", "status")?.as_str() {
            "committed" => {
                report.committed += 1;
                report.receipts.push(serde_json::from_value(
                    row.try_get::<Value>("", "receipt")?,
                )?);
            }
            "skipped" => report.skipped += 1,
            _ => report.pending += 1,
        }
    }
    Ok(report)
}

impl ContextService {
    pub(crate) async fn begin_rebuild(
        &self,
        owner: Uuid,
        operation: Uuid,
        after: Option<Uuid>,
        limit: u16,
    ) -> ContextResult<AppraisalRebuildPage> {
        anyhow::ensure!(
            !owner.is_nil()
                && !operation.is_nil()
                && after.is_none_or(|id| !id.is_nil())
                && (1..=32).contains(&limit),
            "invalid rebuild page intent"
        );
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let (policy, privacy, relationship) = authority(&tx, owner).await?;
        if let Some(row) = tx
            .query_one(stmt(
                "SELECT * FROM appraisal_rebuild_operations WHERE operation_id=$1 FOR UPDATE",
                vec![operation.into()],
            ))
            .await?
        {
            verify_operation(&row, owner, policy.version, privacy, relationship)?;
            if row.try_get::<Option<Uuid>>("", "after_source")? != after
                || row.try_get::<i16>("", "page_limit")? != limit as i16
            {
                return Err(ContextError::RelationshipOperationConflict {
                    operation_id: operation,
                }
                .into());
            }
            let report = page(&tx, &row).await?;
            tx.commit().await?;
            return Ok(report);
        }
        tx.execute(stmt("INSERT INTO appraisal_rebuild_operations(operation_id,owner_id,policy_version,privacy_epoch,relationship_epoch,after_source,page_limit) VALUES($1,$2,$3,$4,$5,$6,$7)", vec![operation.into(), owner.into(), policy.version.into(), privacy.into(), relationship.into(), after.into(), (limit as i16).into()])).await?;
        // Select a fixed page before any interpretation. Only an existing
        // canonical user-fact judgment can nominate a raw source for rebuilding.
        let rows = tx.query_all(stmt("SELECT DISTINCT ON(source_event_id) source_event_id,context_id,source_fact,valid_until FROM appraisal_records WHERE owner_id=$1 AND status='active' AND source_fact->>'kind'='user_message' AND source_privacy_epoch=$2 AND source_relationship_epoch=$3 AND valid_until>clock_timestamp() AND ($4::uuid IS NULL OR source_event_id>$4) ORDER BY source_event_id,created_at DESC LIMIT $5", vec![owner.into(), privacy.into(), relationship.into(), after.into(), i64::from(limit).into()])).await?;
        for row in rows {
            let source: Uuid = row.try_get("", "source_event_id")?;
            let until: DateTime<Utc> = row.try_get("", "valid_until")?;
            let prepared = self
                .rebuild_lease(&tx, owner, operation, &policy, relationship, &row)
                .await;
            let context_id = match prepared {
                Ok(id) => Some(id),
                Err(error) if appraisal::is_revoked(&error) => None,
                Err(error) => return Err(error),
            };
            tx.execute(stmt("INSERT INTO appraisal_rebuild_items(operation_id,source_event_id,context_id,valid_until,status) VALUES($1,$2,$3,$4,$5)", vec![operation.into(), source.into(), context_id.into(), until.into(), if context_id.is_some() { "pending" } else { "skipped" }.into()])).await?;
        }
        let operation_row = tx
            .query_one(stmt(
                "SELECT * FROM appraisal_rebuild_operations WHERE operation_id=$1",
                vec![operation.into()],
            ))
            .await?
            .unwrap();
        let report = page(&tx, &operation_row).await?;
        tx.commit()
            .await
            .map_err(|_| ContextError::CommitUnknownOperation {
                operation_id: operation,
            })?;
        Ok(report)
    }

    async fn rebuild_lease(
        &self,
        tx: &DatabaseTransaction,
        owner: Uuid,
        operation: Uuid,
        policy: &PolicySnapshot,
        relationship: i64,
        record: &QueryResult,
    ) -> ContextResult<Uuid> {
        let old = load(tx, owner, record.try_get("", "context_id")?).await?;
        lock_authority(tx, &old.source, policy.version).await?;
        let fact: AppraisalFact = serde_json::from_value(record.try_get("", "source_fact")?)?;
        appraisal::validate_fact(tx, &old, &fact).await?;
        let source_id: Uuid = record.try_get("", "source_event_id")?;
        if old.source.source_event_id != Some(source_id) {
            return Err(revoked("rebuild source differs from original judgment"));
        }
        let raw = tx.query_one(stmt("SELECT content,source_relationship_epoch FROM chat_messages WHERE id=$1 AND user_id=$2 AND role='user' FOR SHARE", vec![source_id.into(), owner.to_string().into()])).await?.ok_or_else(|| revoked("rebuild raw source disappeared"))?;
        if raw.try_get::<Option<i64>>("", "source_relationship_epoch")? != Some(relationship) {
            return Err(revoked("rebuild source relationship stamp changed"));
        }
        let text: String = raw.try_get("", "content")?;
        if text.len() > MAX_CONTEXT_BYTES {
            return Err(ContextError::BoundsExceeded.into());
        }
        let until: DateTime<Utc> = record.try_get("", "valid_until")?;
        ensure_live(tx, Some(until)).await?;
        let mut source = old.source;
        source.request_id = request_id(operation, source_id);
        let mut lease = ContextLease {
            id: Uuid::new_v4(),
            purpose: ContextPurpose::AppraisalRebuild,
            materialized_digest: String::new(),
            policy_version: policy.version,
            dependencies: vec![
                ContextEvidence::UserMessage {
                    id: source_id,
                    session_id: source
                        .conversation_id
                        .ok_or_else(|| revoked("rebuild source has no session"))?,
                    source_privacy_epoch: source.source_privacy_epoch,
                    content_digest: digest(&json!(text))?,
                },
                ContextEvidence::Relationship {
                    epoch: relationship,
                },
            ],
            source,
            valid_until: Some(until),
        };
        persist(
            tx,
            &mut lease,
            "appraisal-rebuild",
            "appraisal_rebuild",
            &json!({"raw":text,"operation_id":operation,"valid_until":until}),
        )
        .await?;
        Ok(lease.id)
    }

    /// Claim only one pending source. An abandoned claim can be taken over
    /// after 15 seconds; every eventual writer compares its original token.
    pub(crate) async fn claim_rebuild(
        &self,
        owner: Uuid,
        operation: Uuid,
    ) -> ContextResult<Option<AppraisalInput>> {
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let (policy, privacy, relationship) = authority(&tx, owner).await?;
        let op = tx
            .query_one(stmt(
                "SELECT * FROM appraisal_rebuild_operations WHERE operation_id=$1 FOR UPDATE",
                vec![operation.into()],
            ))
            .await?
            .ok_or_else(|| revoked("rebuild intent disappeared"))?;
        verify_operation(&op, owner, policy.version, privacy, relationship)?;
        for _ in 0..32 {
            let Some(row) = tx.query_one(stmt("SELECT * FROM appraisal_rebuild_items WHERE operation_id=$1 AND (status='pending' OR (status='claimed' AND claim_until<=clock_timestamp())) ORDER BY source_event_id LIMIT 1 FOR UPDATE", vec![operation.into()])).await? else { tx.commit().await?; return Ok(None); };
            let source_id: Uuid = row.try_get("", "source_event_id")?;
            let context_id: Uuid = row.try_get("", "context_id")?;
            let prepared = Self::validate_lease_in_transaction(
                &tx,
                owner,
                context_id,
                ContextUse::AppraisalWrite,
            )
            .await;
            let validated = match prepared {
                Ok(lease) => lease,
                Err(error) if appraisal::is_revoked(&error) => {
                    tx.execute(stmt("UPDATE appraisal_rebuild_items SET status='skipped',claim_token=NULL,claim_until=NULL WHERE operation_id=$1 AND source_event_id=$2", vec![operation.into(), source_id.into()])).await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let text: String = tx.query_one(stmt("SELECT content FROM chat_messages WHERE id=$1 AND user_id=$2 AND role='user' FOR SHARE", vec![source_id.into(), owner.to_string().into()])).await?.ok_or_else(|| revoked("rebuild source disappeared"))?.try_get("", "content")?;
            let token = Uuid::new_v4();
            tx.execute(stmt("UPDATE appraisal_rebuild_items SET status='claimed',claim_token=$3,claim_until=clock_timestamp()+INTERVAL '15 seconds' WHERE operation_id=$1 AND source_event_id=$2", vec![operation.into(), source_id.into(), token.into()])).await?;
            let fact = AppraisalFact::UserMessage {
                id: source_id,
                content_digest: digest(&json!(text))?,
            };
            let input_digest = digest(
                &json!({"source":validated.lease.source,"relationship_epoch":relationship,"policy":validated.policy.appraisal.interpretation_digest()?,"fact":fact,"material":text,"roots":validated.lease.dependencies,"operation_id":operation}),
            )?;
            let input = AppraisalInput {
                effective_at: validated.lease.source.source_received_at,
                lease: validated.lease,
                relationship_epoch: relationship,
                policy: validated.policy.appraisal,
                fact,
                occurrence: format!("user:{source_id}"),
                input_digest,
                text,
                recalls: Vec::new(),
                rebuild: Some(RebuildClaim {
                    owner,
                    operation,
                    source: source_id,
                    token,
                }),
            };
            tx.commit()
                .await
                .map_err(|_| ContextError::CommitUnknownOperation {
                    operation_id: operation,
                })?;
            return Ok(Some(input));
        }
        tx.commit().await?;
        Ok(None)
    }

    pub(crate) async fn skip_rebuild_claim(&self, claim: &RebuildClaim) -> ContextResult<()> {
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let (policy, privacy, relationship) = authority(&tx, claim.owner).await?;
        let op = tx
            .query_one(stmt(
                "SELECT * FROM appraisal_rebuild_operations WHERE operation_id=$1 FOR UPDATE",
                vec![claim.operation.into()],
            ))
            .await?
            .ok_or_else(|| revoked("rebuild operation disappeared"))?;
        verify_operation(&op, claim.owner, policy.version, privacy, relationship)?;
        tx.execute(stmt("UPDATE appraisal_rebuild_items SET status='skipped',claim_token=NULL,claim_until=NULL WHERE operation_id=$1 AND source_event_id=$2 AND status='claimed' AND claim_token=$3", vec![claim.operation.into(), claim.source.into(), claim.token.into()])).await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Called after Context authority is locked, before the canonical Appraisal
/// mutation. The claim and its lease expiry cannot be refreshed by the draft.
pub(super) async fn verify_claim(
    tx: &DatabaseTransaction,
    input: &AppraisalInput,
) -> ContextResult<()> {
    let claim = input
        .rebuild
        .as_ref()
        .ok_or_else(|| revoked("missing rebuild claim"))?;
    let op = tx
        .query_one(stmt(
            "SELECT * FROM appraisal_rebuild_operations WHERE operation_id=$1 FOR UPDATE",
            vec![claim.operation.into()],
        ))
        .await?
        .ok_or_else(|| revoked("rebuild intent disappeared"))?;
    verify_operation(
        &op,
        input.lease.source.owner_id,
        input.lease.policy_version,
        input.lease.source.source_privacy_epoch,
        input.relationship_epoch,
    )?;
    let row = tx.query_one(stmt("SELECT context_id,valid_until FROM appraisal_rebuild_items WHERE operation_id=$1 AND source_event_id=$2 AND status='claimed' AND claim_token=$3 AND claim_until>clock_timestamp() FOR UPDATE", vec![claim.operation.into(), claim.source.into(), claim.token.into()])).await?.ok_or_else(|| revoked("rebuild claim expired or was replaced"))?;
    if claim.owner != input.lease.source.owner_id
        || Some(claim.source) != input.lease.source.source_event_id
        || row.try_get::<Uuid>("", "context_id")? != input.lease.id
        || input.lease.valid_until != Some(row.try_get("", "valid_until")?)
    {
        return Err(revoked("rebuild changed its original context or expiry"));
    }
    Ok(())
}

pub(super) async fn finish_claim(
    tx: &DatabaseTransaction,
    claim: &RebuildClaim,
    receipt: &AppraisalReceipt,
) -> ContextResult<()> {
    let affected = tx.execute(stmt("UPDATE appraisal_rebuild_items SET status='committed',claim_token=NULL,claim_until=NULL,receipt=$4 WHERE operation_id=$1 AND source_event_id=$2 AND status='claimed' AND claim_token=$3", vec![claim.operation.into(), claim.source.into(), claim.token.into(), json!(receipt).into()])).await?.rows_affected();
    if affected != 1 {
        return Err(revoked("rebuild result lost its original claim"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
