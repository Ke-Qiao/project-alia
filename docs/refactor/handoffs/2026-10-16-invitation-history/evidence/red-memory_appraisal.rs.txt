//! Flat canonical roots for stored memories. A memory retains the evidence of
//! actual Appraisal inputs without acquiring an unrelated generation context.
use super::{repository::*, *};
use crate::modules::memory::MemoryWriteContext;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction};
use serde_json::{Value, json};
use uuid::Uuid;

fn revoked(reason: &str) -> anyhow::Error {
    ContextError::Revoked(reason.into()).into()
}

fn source_ids(value: Value) -> ContextResult<Vec<Uuid>> {
    let mut ids: Vec<Uuid> = serde_json::from_value(value)?;
    if ids.len() > MAX_EVIDENCE_REFS {
        return Err(ContextError::BoundsExceeded.into());
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

async fn memory_message_ids(
    db: &(impl ConnectionTrait + Sync),
    owner: Uuid,
    id: Uuid,
) -> ContextResult<Vec<Uuid>> {
    let row=db.query_one(stmt("SELECT to_jsonb(source_event_ids) AS sources FROM memory_items WHERE id=$1 AND user_id=$2 AND tenant_id=$3 FOR SHARE",vec![id.into(),owner.into(),Uuid::nil().into()])).await?.ok_or_else(||revoked("memory source is missing"))?;
    source_ids(row.try_get("", "sources")?)
}

/// Partial coverage is accepted only while binding a just-written canonical
/// revision. Existing mutations validate complete coverage before changing it.
async fn message_pins(
    db: &(impl ConnectionTrait + Sync),
    owner: Uuid,
    id: Uuid,
    expected: &[Uuid],
    complete: bool,
) -> ContextResult<Vec<ContextEvidence>> {
    let pins=db.query_all(stmt("SELECT source_event_id,owner_id,source_privacy_epoch,content_digest FROM memory_message_sources WHERE memory_id=$1 ORDER BY source_event_id LIMIT 257 FOR SHARE",vec![id.into()])).await?;
    if pins.len() > MAX_EVIDENCE_REFS {
        return Err(ContextError::BoundsExceeded.into());
    }
    if complete && pins.len() != expected.len() {
        return Err(revoked("memory message source pin set is incomplete"));
    }
    let mut roots = Vec::new();
    for pin in pins {
        let source: Uuid = pin.try_get("", "source_event_id")?;
        if pin.try_get::<Uuid>("", "owner_id")? != owner || expected.binary_search(&source).is_err()
        {
            return Err(revoked(
                "memory message source pin set differs from canonical sources",
            ));
        }
        let epoch: i64 = pin.try_get("", "source_privacy_epoch")?;
        let content_digest: String = pin.try_get("", "content_digest")?;
        let message=db.query_one(stmt("SELECT m.content FROM chat_messages m JOIN chat_sessions s ON s.id=m.session_id WHERE m.id=$1 AND m.user_id=$2 AND s.creator_id=$2 AND m.role='user' AND m.source_received_at IS NOT NULL AND m.source_privacy_epoch=$3 FOR SHARE OF m",vec![source.into(),owner.to_string().into(),epoch.into()])).await?.ok_or_else(||revoked("pinned memory message source disappeared or changed epoch"))?;
        if digest(&json!(message.try_get::<String>("", "content")?))? != content_digest {
            return Err(revoked("pinned memory message content changed"));
        }
        roots.push(ContextEvidence::MemorySourceMessage {
            id: source,
            source_privacy_epoch: epoch,
            content_digest,
        });
    }
    Ok(roots)
}

/// The caller owns policy/account/privacy/relationship authority. This gate
/// must run before patch/reinforcement; it never repairs an unpinned old row.
pub(crate) async fn validate_memory_message_sources(
    db: &(impl ConnectionTrait + Sync),
    owner: Uuid,
    id: Uuid,
) -> ContextResult<()> {
    let expected = memory_message_ids(db, owner, id).await?;
    message_pins(db, owner, id, &expected, true).await?;
    Ok(())
}

async fn bind_memory_message_sources(
    tx: &DatabaseTransaction,
    context: &MemoryWriteContext,
    id: Uuid,
) -> ContextResult<()> {
    let expected = memory_message_ids(tx, context.user_id, id).await?;
    if context
        .source_event_ids
        .iter()
        .any(|source| expected.binary_search(source).is_err())
    {
        return Err(revoked(
            "canonical memory omitted an admitted message source",
        ));
    }
    let existing = message_pins(tx, context.user_id, id, &expected, false).await?;
    for source in &expected {
        if existing
            .iter()
            .any(|pin| matches!(pin,ContextEvidence::MemorySourceMessage{id,..} if id==source))
        {
            continue;
        }
        if !context.source_event_ids.contains(source) {
            return Err(revoked(
                "unproven historical memory message source cannot acquire a new pin",
            ));
        }
        let row=tx.query_one(stmt("SELECT m.content,m.source_privacy_epoch FROM chat_messages m JOIN chat_sessions s ON s.id=m.session_id WHERE m.id=$1 AND m.user_id=$2 AND s.creator_id=$2 AND m.role='user' AND m.source_received_at IS NOT NULL AND m.source_privacy_epoch=$3 FOR SHARE OF m",vec![(*source).into(),context.user_id.to_string().into(),context.privacy_epoch.into()])).await?.ok_or_else(||revoked("new memory message source is outside the admitted owner/epoch"))?;
        let content_digest = digest(&json!(row.try_get::<String>("", "content")?))?;
        tx.execute(stmt("INSERT INTO memory_message_sources(memory_id,source_event_id,owner_id,source_privacy_epoch,content_digest) VALUES($1,$2,$3,$4,$5) ON CONFLICT(memory_id,source_event_id) DO NOTHING",vec![id.into(),(*source).into(),context.user_id.into(),context.privacy_epoch.into(),content_digest.into()])).await?;
    }
    message_pins(tx, context.user_id, id, &expected, true).await?;
    Ok(())
}

pub(crate) async fn direct_roots(
    tx: &DatabaseTransaction,
    owner: Uuid,
    id: Uuid,
) -> ContextResult<(Vec<ContextEvidence>, Option<DateTime<Utc>>)> {
    let row=tx.query_one(stmt("SELECT to_jsonb(source_event_ids) AS sources,expires_at FROM memory_items WHERE id=$1 AND user_id=$2 AND tenant_id=$3 FOR SHARE",vec![id.into(),owner.into(),Uuid::nil().into()])).await?.ok_or_else(||revoked("memory source is missing"))?;
    let sources = source_ids(row.try_get("", "sources")?)?;
    let mut roots = message_pins(tx, owner, id, &sources, true).await?;
    let mut expiry = row.try_get("", "expires_at")?;
    let parents=tx.query_all(stmt("SELECT source_id,source_revision FROM memory_sources WHERE memory_id=$1 ORDER BY source_id LIMIT 257",vec![id.into()])).await?;
    if parents.len() > MAX_EVIDENCE_REFS {
        return Err(ContextError::BoundsExceeded.into());
    }
    for row in parents {
        merge(
            &mut roots,
            &[ContextEvidence::Memory {
                id: row.try_get("", "source_id")?,
                revision: row.try_get("", "source_revision")?,
            }],
        )?;
    }
    if let Some(row)=tx.query_one(stmt("SELECT roots,roots_digest,valid_until FROM memory_appraisal_sources WHERE memory_id=$1 AND owner_id=$2 FOR SHARE",vec![id.into(),owner.into()])).await? {
        let value:Value=row.try_get("","roots")?;
        if digest(&value)?!=row.try_get::<String>("","roots_digest")? {return Err(revoked("memory appraisal roots changed"));}
        merge(&mut roots,&serde_json::from_value::<Vec<ContextEvidence>>(value)?)?;
        expiry=earliest(expiry,row.try_get("","valid_until")?);
    }
    ensure_live(tx, expiry).await?;
    Ok((roots, expiry))
}

pub(crate) async fn memory_source_roots(
    tx: &DatabaseTransaction,
    owner: Uuid,
    id: Uuid,
    revision: i64,
) -> ContextResult<(Vec<ContextEvidence>, Option<DateTime<Utc>>)> {
    let mut roots = vec![ContextEvidence::Memory { id, revision }];
    let mut visited = std::collections::BTreeSet::new();
    let mut frontier = vec![(id, revision, Vec::<Uuid>::new())];
    let mut expiry = None;
    while let Some((id, revision, mut path)) = frontier.pop() {
        if path.contains(&id) {
            return Err(revoked("memory evidence cycle"));
        }
        if !visited.insert(id) {
            continue;
        }
        if visited.len() > MAX_EVIDENCE_REFS {
            return Err(ContextError::BoundsExceeded.into());
        }
        if tx.query_one(stmt("SELECT id FROM memory_items WHERE id=$1 AND user_id=$2 AND tenant_id=$3 AND revision=$4 AND status='active' AND NOT sensitive AND visibility<>'private_runtime' AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR SHARE",vec![id.into(),owner.into(),Uuid::nil().into(),revision.into()])).await?.is_none() {return Err(revoked("memory ancestor revision is unavailable"));}
        let (direct, until) = direct_roots(tx, owner, id).await?;
        merge(&mut roots, &direct)?;
        expiry = earliest(expiry, until);
        path.push(id);
        for root in direct {
            if let ContextEvidence::Memory { id, revision } = root {
                frontier.push((id, revision, path.clone()));
            }
        }
    }
    Ok((roots, expiry))
}

/// Validate before exporting memory to a provider and again before writing its
/// projection. Lock order is policy/account/privacy/relationship, then evidence.
pub(crate) async fn validate_memory_roots_in_transaction(
    tx: &DatabaseTransaction,
    owner: Uuid,
    id: Uuid,
    revision: i64,
) -> ContextResult<()> {
    let policy = policy::verify(tx, None, ContextUse::AffectRead).await?;
    let privacy = tx
        .query_one(stmt(
            "SELECT privacy_epoch FROM memory_privacy_state WHERE user_id=$1",
            vec![owner.into()],
        ))
        .await?
        .ok_or_else(|| revoked("memory owner privacy state missing"))?
        .try_get("", "privacy_epoch")?;
    let source = ContextSource {
        owner_id: owner,
        request_id: Uuid::new_v4(),
        channel_id: "memory-root-validation".into(),
        conversation_id: None,
        source_event_id: None,
        source_received_at: Utc::now(),
        source_privacy_epoch: privacy,
    };
    lock_authority(tx, &source, policy.version).await?;
    let (dependencies, valid_until) = memory_source_roots(tx, owner, id, revision).await?;
    let lease = ContextLease {
        id: Uuid::nil(),
        purpose: ContextPurpose::Generation,
        materialized_digest: String::new(),
        source,
        policy_version: policy.version,
        dependencies,
        valid_until,
    };
    validate_evidence(tx, &lease).await
}

pub(crate) async fn validated_graph_projection(
    tx: &DatabaseTransaction,
    owner: Uuid,
    id: Uuid,
) -> ContextResult<Option<Value>> {
    let policy = policy::verify(tx, None, ContextUse::AffectRead).await?;
    let privacy = tx
        .query_one(stmt(
            "SELECT privacy_epoch FROM memory_privacy_state WHERE user_id=$1",
            vec![owner.into()],
        ))
        .await?
        .ok_or_else(|| revoked("graph owner privacy state missing"))?
        .try_get("", "privacy_epoch")?;
    let source = ContextSource {
        owner_id: owner,
        request_id: Uuid::new_v4(),
        channel_id: "graph-root-validation".into(),
        conversation_id: None,
        source_event_id: None,
        source_received_at: Utc::now(),
        source_privacy_epoch: privacy,
    };
    lock_authority(tx, &source, policy.version).await?;
    let Some((projection, mut dependencies, valid_until)) = graph_projection(tx, owner, id).await?
    else {
        return Ok(None);
    };
    merge(
        &mut dependencies,
        &[ContextEvidence::Graph {
            edge_id: id,
            projection_digest: digest(&projection)?,
        }],
    )?;
    let lease = ContextLease {
        id: Uuid::nil(),
        purpose: ContextPurpose::Generation,
        materialized_digest: String::new(),
        source,
        policy_version: policy.version,
        dependencies,
        valid_until,
    };
    Box::pin(validate_evidence(tx, &lease)).await?;
    Ok(Some(projection))
}

pub(crate) async fn bind_memory_roots(
    tx: &DatabaseTransaction,
    context: &MemoryWriteContext,
    id: Uuid,
) -> ContextResult<()> {
    bind_memory_message_sources(tx, context, id).await?;
    let mut roots = Vec::new();
    let mut expiry = None;
    if let Some(row)=tx.query_one(stmt("SELECT roots,roots_digest,valid_until FROM memory_appraisal_sources WHERE memory_id=$1 AND owner_id=$2 FOR UPDATE",vec![id.into(),context.user_id.into()])).await? {
        let old:Value=row.try_get("","roots")?;
        if digest(&old)?!=row.try_get::<String>("","roots_digest")? {return Err(revoked("stored memory domain roots changed"));}
        merge(&mut roots,&serde_json::from_value::<Vec<ContextEvidence>>(old)?)?;
        expiry=row.try_get("","valid_until")?;
    }
    let Some(context_id) = context.context_id else {
        return Ok(());
    };
    let lease = load(tx, context.user_id, context_id).await?;
    for root in &lease.dependencies {
        let row=match root {
            ContextEvidence::Appraisal{id, ..}=>tx.query_one(stmt("SELECT roots,roots_digest,valid_until FROM appraisal_records WHERE appraisal_id=$1 AND owner_id=$2 AND status='active'",vec![(*id).into(),context.user_id.into()])).await?,
            ContextEvidence::AffectProjection{id,..}=>tx.query_one(stmt("SELECT roots,roots_digest,valid_until FROM affect_projections WHERE projection_id=$1 AND owner_id=$2",vec![(*id).into(),context.user_id.into()])).await?,
            ContextEvidence::Relationship{..}=>{merge(&mut roots,std::slice::from_ref(root))?;continue;},
            _=>continue,
        }.ok_or_else(||revoked("memory appraisal or affect root was retired"))?;
        merge(&mut roots, &appraisal::record_roots(&row)?)?;
        merge(&mut roots, std::slice::from_ref(root))?;
        expiry = earliest(expiry, row.try_get("", "valid_until")?);
    }
    if roots.is_empty() {
        return Ok(());
    }
    if roots
        .iter()
        .any(|root| matches!(root,ContextEvidence::Memory{id:source,..} if *source==id))
    {
        return Err(revoked(
            "appraisal-derived memory cannot rewrite an ancestor consumed by that appraisal",
        ));
    }
    let value = json!(roots);
    tx.execute(stmt("INSERT INTO memory_appraisal_sources(memory_id,owner_id,roots,roots_digest,source,policy_version,valid_until) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(memory_id) DO UPDATE SET roots=EXCLUDED.roots,roots_digest=EXCLUDED.roots_digest,valid_until=EXCLUDED.valid_until WHERE memory_appraisal_sources.owner_id=EXCLUDED.owner_id",vec![id.into(),context.user_id.into(),value.clone().into(),digest(&value)?.into(),json!(lease.source).into(),lease.policy_version.into(),expiry.into()])).await?;
    Ok(())
}

/// The caller holds policy/account/privacy/relationship authority. Combine
/// the original selection with the domain being revoked before deleting any
/// roots; the shared SQL closure counts each actual tombstone transition once.
pub(crate) async fn erase_revoked_memories(
    tx: &DatabaseTransaction,
    owner: Uuid,
    selected: &[String],
) -> ContextResult<u64> {
    let row = tx.query_one(stmt(
        "SELECT erase_memory_closure($1,$2,ARRAY(SELECT jsonb_array_elements_text($3::jsonb)::uuid UNION SELECT a.memory_id FROM memory_appraisal_sources a JOIN memory_items m ON m.id=a.memory_id WHERE a.owner_id=$1 AND m.user_id=$1 AND m.tenant_id=$2)) AS erased",
        vec![owner.into(),Uuid::nil().into(),json!(selected).into()],
    )).await?.ok_or_else(||revoked("memory erasure result missing"))?;
    Ok(u64::try_from(row.try_get::<i64>("", "erased")?)?)
}

pub(super) async fn erase_relationship_memories(
    tx: &DatabaseTransaction,
    owner: Uuid,
) -> ContextResult<u64> {
    erase_revoked_memories(tx, owner, &[]).await
}
