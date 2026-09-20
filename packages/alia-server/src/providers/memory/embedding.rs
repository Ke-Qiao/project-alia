//! Vectors are projections of one committed canonical Memory revision.
//! Inference never holds database locks and never refreshes a work item's authority.
use std::time::Duration;

use sea_orm::{DatabaseTransaction, TransactionTrait};

use super::*;
use crate::modules::memory::context::{ContextError, ContextService, ContextUse, PolicySnapshot};
use crate::providers::adapter::{EmbeddingProfile, EmbeddingVector, embedding_text_digest};

pub(super) const DB_BUDGET: Duration = Duration::from_secs(2);
pub(super) const INLINE_BUDGET: Duration = Duration::from_secs(2);
pub(super) const REBUILD_BUDGET: Duration = Duration::from_secs(30);
const MAX_INPUT_BYTES: usize = 256 * 1024;

pub(super) fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}

#[derive(Clone, Debug)]
pub(super) struct WorkSeed {
    pub owner: Uuid,
    pub id: Uuid,
    pub revision: i64,
    pub epoch: i64,
    pub policy_version: i64,
}

#[derive(Clone, Debug)]
pub(super) struct MemoryEmbeddingWork {
    pub seed: WorkSeed,
    profile: EmbeddingProfile,
    input_digest: String,
    text: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WriteResult {
    Stored,
    AlreadyPresent,
    Stale,
}

/// Only this module can construct a validated query vector.
pub(crate) struct ProfiledMemoryQuery {
    pub(super) profile: EmbeddingProfile,
    pub(super) vector: EmbeddingVector,
}

async fn read_profile(
    db: &(impl ConnectionTrait + Sync),
    id: &str,
) -> anyhow::Result<Option<EmbeddingProfile>> {
    let Some(row) = db.query_one(sql(
        "SELECT provider,model,dimensions,distance,vector_type,request_descriptor FROM embedding_profiles WHERE id=$1",
        vec![id.into()],
    )).await? else { return Ok(None); };
    let Some(descriptor) = row.try_get::<Option<Value>>("", "request_descriptor")? else {
        return Ok(None);
    };
    let profile: EmbeddingProfile = serde_json::from_value(descriptor)?;
    anyhow::ensure!(
        profile.id()? == id
            && row.try_get::<String>("", "provider")? == profile.provider
            && row.try_get::<String>("", "model")? == profile.model
            && row.try_get::<i32>("", "dimensions")? == profile.dimensions as i32
            && row.try_get::<String>("", "distance")? == profile.distance
            && row.try_get::<String>("", "vector_type")? == profile.vector_type,
        "Memory embedding profile identity mismatch"
    );
    Ok(Some(profile))
}

async fn adapter_matches(
    adapter: &dyn EmbeddingAdapter,
    expected: &EmbeddingProfile,
) -> anyhow::Result<bool> {
    let actual = tokio::time::timeout(
        DB_BUDGET,
        adapter.preview_profile_settings(&BTreeMap::new()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Memory embedding profile preview timed out"))??;
    Ok(actual.as_ref() == Some(expected))
}

pub(crate) async fn query_for_policy(
    db: &DatabaseConnection,
    adapter: &dyn EmbeddingAdapter,
    policy: &PolicySnapshot,
    text: &str,
) -> anyhow::Result<Option<ProfiledMemoryQuery>> {
    if !policy.policy.reads_memory() || text.len() > MAX_INPUT_BYTES {
        return Ok(None);
    }
    let Some(id) = policy.policy.active_embedding_profile.as_deref() else {
        return Ok(None);
    };
    let profile = tokio::time::timeout(DB_BUDGET, read_profile(db, id))
        .await
        .map_err(|_| anyhow::anyhow!("Memory query profile read timed out"))??;
    let Some(profile) = profile else {
        return Ok(None);
    };
    if !adapter_matches(adapter, &profile).await? {
        return Ok(None);
    }
    let result =
        tokio::time::timeout(INLINE_BUDGET, adapter.embed_profiled(&[text.to_owned()])).await;
    let Ok(Ok(Some(mut batch))) = result else {
        return Ok(None);
    };
    if batch.profile != profile || batch.vectors.len() != 1 {
        return Ok(None);
    }
    let vector = batch.vectors.remove(0);
    if vector.input_digest != embedding_text_digest(text) || vector.validate(&profile).is_err() {
        return Ok(None);
    }
    Ok(Some(ProfiledMemoryQuery { profile, vector }))
}

async fn roots_current(tx: &DatabaseTransaction, seed: &WorkSeed) -> anyhow::Result<bool> {
    match crate::modules::memory::context::memory_appraisal::validate_memory_roots_in_transaction(
        tx,
        seed.owner,
        seed.id,
        seed.revision,
    )
    .await
    {
        Ok(()) => Ok(true),
        Err(error)
            if error.downcast_ref::<ContextError>().is_some_and(|error| {
                matches!(
                    error,
                    ContextError::Revoked(_)
                        | ContextError::PolicyChanged
                        | ContextError::BoundsExceeded
                )
            }) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

pub(super) async fn prepare(
    tx: &DatabaseTransaction,
    seed: WorkSeed,
) -> anyhow::Result<Option<MemoryEmbeddingWork>> {
    // Manual canonical writes remain possible while projection is disabled.
    let policy = ContextService::policy_in_transaction(
        tx,
        Some(seed.policy_version),
        ContextUse::ManualMemoryWrite,
    )
    .await?;
    if !policy.policy.reads_memory() {
        return Ok(None);
    }
    let Some(id) = policy.policy.active_embedding_profile.as_deref() else {
        return Ok(None);
    };
    let current = privacy::lock_user(tx, seed.owner).await?;
    anyhow::ensure!(
        current.privacy_epoch == seed.epoch,
        "Memory projection epoch changed"
    );
    if !roots_current(tx, &seed).await? {
        return Ok(None);
    }
    let Some(row) = tx.query_one(sql(
        "SELECT content,sensitive,visibility FROM memory_items WHERE id=$1 AND user_id=$2 AND tenant_id=$3 AND revision=$4 AND status='active' AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR SHARE",
        vec![seed.id.into(),seed.owner.into(),DEFAULT_TENANT_ID.into(),seed.revision.into()],
    )).await? else { return Ok(None); };
    let text: String = row.try_get("", "content")?;
    if text.trim().is_empty()
        || text.len() > MAX_INPUT_BYTES
        || row.try_get::<bool>("", "sensitive")?
        || row.try_get::<String>("", "visibility")? == "private_runtime"
        || (policy.policy.sensitive_filter_enabled
            && crate::memory_policy::detect_sensitive_memory(&text).is_some())
    {
        return Ok(None);
    }
    let Some(profile) = read_profile(tx, id).await? else {
        return Ok(None);
    };
    Ok(Some(MemoryEmbeddingWork {
        seed,
        profile,
        input_digest: embedding_text_digest(&text),
        text,
    }))
}

async fn valid(tx: &DatabaseTransaction, work: &MemoryEmbeddingWork) -> anyhow::Result<bool> {
    let policy = match ContextService::policy_in_transaction(
        tx,
        Some(work.seed.policy_version),
        ContextUse::MemoryEmbedding,
    )
    .await
    {
        Ok(policy) => policy,
        Err(error)
            if matches!(
                error.downcast_ref::<ContextError>(),
                Some(ContextError::PolicyChanged | ContextError::Revoked(_))
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    if policy.policy.active_embedding_profile.as_deref() != Some(work.profile.id()?.as_str()) {
        return Ok(false);
    }
    let current = privacy::lock_user(tx, work.seed.owner).await?;
    if current.privacy_epoch != work.seed.epoch {
        return Ok(false);
    }
    // Both provider admission and commit call this gate. A new worker or an
    // unchanged child revision cannot renew a changed ancestor's authority.
    if !roots_current(tx, &work.seed).await? {
        return Ok(false);
    }
    let Some(row) = tx.query_one(sql(
        "SELECT content,sensitive,visibility FROM memory_items WHERE id=$1 AND user_id=$2 AND tenant_id=$3 AND revision=$4 AND status='active' AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR SHARE",
        vec![work.seed.id.into(),work.seed.owner.into(),DEFAULT_TENANT_ID.into(),work.seed.revision.into()],
    )).await? else { return Ok(false); };
    let text: String = row.try_get("", "content")?;
    Ok(text == work.text
        && embedding_text_digest(&text) == work.input_digest
        && !row.try_get::<bool>("", "sensitive")?
        && row.try_get::<String>("", "visibility")? != "private_runtime"
        && (!policy.policy.sensitive_filter_enabled
            || crate::memory_policy::detect_sensitive_memory(&text).is_none()))
}

async fn already_present(
    tx: &DatabaseTransaction,
    work: &MemoryEmbeddingWork,
) -> anyhow::Result<bool> {
    Ok(tx.query_one(sql(
        "SELECT memory_id FROM memory_embeddings WHERE memory_id=$1 AND profile_id=$2 AND source_revision=$3 AND input_digest=$4",
        vec![work.seed.id.into(),work.profile.id()?.into(),work.seed.revision.into(),work.input_digest.clone().into()],
    )).await?.is_some())
}

pub(super) async fn project(
    db: &DatabaseConnection,
    adapter: &dyn EmbeddingAdapter,
    work: &MemoryEmbeddingWork,
    budget: Duration,
) -> anyhow::Result<WriteResult> {
    let ready = tokio::time::timeout(DB_BUDGET, async {
        let tx = db.begin().await?;
        tx.execute(sql("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let result = if !valid(&tx, work).await? {
            Some(WriteResult::Stale)
        } else if already_present(&tx, work).await? {
            Some(WriteResult::AlreadyPresent)
        } else {
            None
        };
        tx.commit().await?;
        Ok::<_, anyhow::Error>(result)
    })
    .await
    .map_err(|_| anyhow::anyhow!("Memory embedding admission timed out"))??;
    if let Some(result) = ready {
        return Ok(result);
    }
    if !adapter_matches(adapter, &work.profile).await? {
        return Ok(WriteResult::Stale);
    }
    let batch = tokio::time::timeout(budget, adapter.embed_profiled(&[work.text.clone()]))
        .await
        .map_err(|_| anyhow::anyhow!("Memory embedding request timed out"))??
        .ok_or_else(|| anyhow::anyhow!("embedding adapter cannot prove its request identity"))?;
    anyhow::ensure!(
        batch.profile == work.profile && batch.vectors.len() == 1,
        "Memory embedding result profile or count mismatch"
    );
    let vector = &batch.vectors[0];
    vector.validate(&batch.profile)?;
    anyhow::ensure!(
        vector.input_digest == work.input_digest,
        "Memory embedding input mismatch"
    );
    tokio::time::timeout(DB_BUDGET, async {
        let tx = db.begin().await?;
        tx.execute(sql("SET LOCAL statement_timeout='2s'", vec![])).await?;
        if !valid(&tx, work).await? { return Ok(WriteResult::Stale); }
        let profile_id = crate::providers::embedding::profile_store::ensure_profile(&tx, &batch.profile).await?;
        let result = tx.execute(sql(
            "INSERT INTO memory_embeddings(memory_id,profile_id,source_revision,input_digest,policy_version,privacy_epoch,dimensions,embedding) VALUES($1,$2,$3,$4,$5,$6,$7,$8::vector) ON CONFLICT(memory_id,profile_id) DO NOTHING",
            vec![work.seed.id.into(),profile_id.into(),work.seed.revision.into(),work.input_digest.clone().into(),work.seed.policy_version.into(),work.seed.epoch.into(),(batch.profile.dimensions as i32).into(),vector_literal(&vector.values).into()],
        )).await?;
        let result = if result.rows_affected() == 1 { WriteResult::Stored }
            else if already_present(&tx, work).await? { WriteResult::AlreadyPresent }
            else { WriteResult::Stale };
        tx.commit().await?;
        Ok(result)
    }).await.map_err(|_| anyhow::anyhow!("Memory embedding commit was not confirmed"))?
}
