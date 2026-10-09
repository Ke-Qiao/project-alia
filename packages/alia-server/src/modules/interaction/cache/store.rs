//! Private, versioned derived responses. All reuse and writes revalidate the
//! original ContextLease inside the transaction which reads or changes a row.
use std::{future::Future, time::Duration};

use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, QueryResult,
    Statement, TransactionTrait,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::types::{
    CACHE_METADATA_SCHEMA, CacheCas, CacheEntry, CacheReview, CacheStatus, CacheStore, CacheWork,
    CachedResponse, SemanticCacheCandidate,
};
use crate::modules::memory::context::{
    ContextError, ContextEvidence, ContextService, ContextUse, PolicySnapshot, evidence_digest,
};
use crate::modules::platform::lifecycle::{ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{EmbeddingProfile, EmbeddingVector, embedding_text_digest};
use crate::providers::embedding::profile_store::ensure_profile;

const MAX_TEXT: usize = 256 * 1024;
const MAX_METADATA: usize = 32 * 1024;
const SNAPSHOT: &str = "SELECT to_jsonb(c) || jsonb_build_object('version',jsonb_build_object('cache_id',c.id,'revision',c.revision,'context_id',c.context_id),'owner_id',c.user_id,'query_digest',c.query_hash) AS record FROM llm_cache c ";

#[derive(Default)]
pub struct CacheService {
    db: Option<DatabaseConnection>,
    background: crate::modules::platform::background::OwnedBackground,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
}

impl CacheService {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_database(db: DatabaseConnection) -> Self {
        Self {
            db: Some(db),
            ..Self::default()
        }
    }
    pub fn with_runtime(
        db: DatabaseConnection,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            cancel_token: Some(cancel_token),
            ..Self::with_database(db)
        }
    }
    fn db(&self) -> anyhow::Result<&DatabaseConnection> {
        self.db
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("cache is not initialized"))
    }

    /// The caller already holds the shared policy/account/privacy lock order
    /// and advances the existing privacy epoch in the same erase transaction.
    pub async fn purge_owner_in_transaction(
        tx: &DatabaseTransaction,
        owner: Uuid,
    ) -> anyhow::Result<u64> {
        anyhow::ensure!(!owner.is_nil(), "cache owner is required");
        Ok(tx
            .execute(stmt(
                "DELETE FROM llm_cache WHERE user_id=$1",
                vec![owner.into()],
            ))
            .await?
            .rows_affected())
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}
async fn bounded<T>(future: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .map_err(|_| anyhow::anyhow!("cache database confirmation timed out"))?
}
fn scope_valid(scope: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !scope.trim().is_empty()
            && scope.chars().count() <= 100
            && !scope.chars().any(char::is_control),
        "invalid private cache scope"
    );
    Ok(())
}
fn content_valid(text: &str, metadata: &Value) -> anyhow::Result<()> {
    anyhow::ensure!(
        !text.trim().is_empty() && text.len() <= MAX_TEXT,
        "cache response is empty or exceeds its bound"
    );
    anyhow::ensure!(
        metadata.is_object() && serde_json::to_vec(metadata)?.len() <= MAX_METADATA,
        "invalid cache metadata"
    );
    Ok(())
}
pub fn response_digest(text: &str) -> anyhow::Result<String> {
    evidence_digest(&json!({"response_text": text}))
}
fn snapshot(row: QueryResult) -> anyhow::Result<CachedResponse> {
    let result: CachedResponse = serde_json::from_value(row.try_get("", "record")?)?;
    anyhow::ensure!(
        embedding_text_digest(&result.query_text) == result.query_digest
            && response_digest(&result.response_text)? == result.content_digest,
        "cache content does not match its canonical digest"
    );
    Ok(result)
}
fn expected_valid(expected: &CachedResponse) -> anyhow::Result<()> {
    scope_valid(&expected.scope)?;
    anyhow::ensure!(
        !expected.owner_id.is_nil()
            && !expected.version.cache_id.is_nil()
            && expected.version.revision > 0
            && !expected.version.context_id.is_nil()
            && embedding_text_digest(&expected.query_text) == expected.query_digest
            && response_digest(&expected.response_text)? == expected.content_digest,
        "invalid cache version"
    );
    Ok(())
}
async fn owner_gate(
    tx: &DatabaseTransaction,
    owner: Uuid,
    purpose: ContextUse,
) -> anyhow::Result<PolicySnapshot> {
    anyhow::ensure!(!owner.is_nil(), "cache owner is required");
    let policy = ContextService::policy_in_transaction(tx, None, purpose).await?;
    anyhow::ensure!(
        tx.query_one(stmt(
            "SELECT id FROM users WHERE id=$1 AND status='active' FOR SHARE",
            vec![owner.into()],
        ))
        .await?
        .is_some(),
        "cache requires an active account"
    );
    anyhow::ensure!(
        tx.query_one(stmt(
            "SELECT user_id FROM memory_privacy_state WHERE user_id=$1 FOR UPDATE",
            vec![owner.into()],
        ))
        .await?
        .is_some(),
        "cache source privacy state is missing"
    );
    Ok(policy)
}
fn revoked(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<ContextError>(),
        Some(ContextError::Revoked(_) | ContextError::PolicyChanged)
    )
}
async fn current_version(
    tx: &DatabaseTransaction,
    expected: &CachedResponse,
    purpose: ContextUse,
    lock: &str,
) -> anyhow::Result<Option<CachedResponse>> {
    expected_valid(expected)?;
    match ContextService::validate_lease_in_transaction(
        tx,
        expected.owner_id,
        expected.version.context_id,
        purpose,
    )
    .await
    {
        Ok(_) => {}
        Err(error) if revoked(&error) => return Ok(None),
        Err(error) => return Err(error),
    }
    tx.query_one(stmt(&format!(
        "{SNAPSHOT} WHERE c.id=$1 AND c.revision=$2 AND c.context_id=$3 AND c.user_id=$4 AND c.scope=$5 AND c.query_hash=$6 AND c.content_digest=$7 AND c.origin_response_id=$8 AND c.expires_at>clock_timestamp() {lock}"
    ), vec![
        expected.version.cache_id.into(), expected.version.revision.into(),
        expected.version.context_id.into(), expected.owner_id.into(),
        expected.scope.clone().into(), expected.query_digest.clone().into(),
        expected.content_digest.clone().into(), expected.origin_response_id.into(),
    ])).await?.map(snapshot).transpose()
}
async fn by_id(tx: &DatabaseTransaction, id: Uuid) -> anyhow::Result<CachedResponse> {
    snapshot(
        tx.query_one(stmt(&format!("{SNAPSHOT} WHERE c.id=$1"), vec![id.into()]))
            .await?
            .ok_or_else(|| anyhow::anyhow!("cache row disappeared"))?,
    )
}
fn safe_review_metadata(metadata: &Value) -> bool {
    metadata.get("schema").and_then(Value::as_str) == Some(CACHE_METADATA_SCHEMA)
        && metadata.get("policy").and_then(Value::as_str) == Some("private_exact")
        && [
            "safe_direct_return",
            "memory_independent",
            "tool_free",
            "dynamic_free",
        ]
        .iter()
        .all(|key| metadata.get(*key).and_then(Value::as_bool) == Some(true))
}
fn vector_text(vector: &EmbeddingVector) -> String {
    format!(
        "[{}]",
        vector
            .values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[async_trait]
impl ModuleLifecycle for CacheService {
    fn name(&self) -> &'static str {
        "db"
    }
    async fn init(&mut self) -> anyhow::Result<()> {
        self.cleanup_expired(256).await?;
        Ok(())
    }
    async fn start(&self) -> anyhow::Result<()> {
        let parent = self
            .cancel_token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("cache is not initialized"))?;
        let mut startup = self.background.begin_start(parent).await?;
        let db = self.db()?.clone();
        startup.spawn_loop("cache expiry cleanup", async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if let Err(error) = cleanup_expired_in_db(&db, 256).await {
                    tracing::warn!(error=?error,"cache expiry cleanup was not confirmed");
                }
            }
        });
        startup.commit();
        Ok(())
    }
    async fn stop(&self) -> anyhow::Result<()> {
        self.background.stop().await
    }
    async fn health_check(&self) -> ModuleHealth {
        if self.db.is_some() {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Unhealthy("cache is not initialized".into())
        }
    }
}

#[async_trait]
impl CacheStore for CacheService {
    async fn get(
        &self,
        owner: Uuid,
        scope: &str,
        query: &str,
    ) -> anyhow::Result<Option<CachedResponse>> {
        scope_valid(scope)?;
        bounded(async {
            let tx = self.db()?.begin().await?;
            let policy = owner_gate(&tx, owner, ContextUse::CacheReuse).await?;
            if !policy.policy.cache_exact_enabled { return Ok(None); }
            let old = tx.query_one(stmt(&format!(
                "{SNAPSHOT} WHERE c.user_id=$1 AND c.scope=$2 AND c.query_hash=$3 AND c.query_text=$4 AND c.status='active' AND c.expires_at>clock_timestamp()"
            ),vec![owner.into(),scope.into(),embedding_text_digest(query).into(),query.into()]))
                .await?.map(snapshot).transpose()?;
            let Some(old) = old else { return Ok(None); };
            let Some(current) = current_version(&tx,&old,ContextUse::CacheReuse,"FOR SHARE").await?
                else { return Ok(None); };
            if current.status != CacheStatus::Active { return Ok(None); }
            // No ORM snapshot update: telemetry cannot overwrite newer content.
            tx.execute(stmt("UPDATE llm_cache SET hit_count=LEAST(hit_count::bigint+1,2147483647)::integer WHERE id=$1 AND revision=$2",
                vec![current.version.cache_id.into(),current.version.revision.into()])).await?;
            tx.commit().await?;
            Ok(Some(current))
        }).await
    }

    async fn put(&self, entry: CacheEntry) -> anyhow::Result<CachedResponse> {
        scope_valid(&entry.scope)?;
        content_valid(&entry.response_text, &entry.metadata)?;
        anyhow::ensure!(
            !entry.query_text.trim().is_empty()
                && entry.query_text.len() <= MAX_TEXT
                && entry.ttl_seconds > 0
                && !entry.origin_response_id.is_nil(),
            "invalid cache input"
        );
        bounded(async {
            let tx = self.db()?.begin().await?;
            let validated = ContextService::validate_in_transaction(
                &tx,&entry.context,ContextUse::CacheWrite,
            ).await?;
            let response = tx.query_one(stmt(
                "SELECT user_id,request_id,context_id,source_event_id,source_privacy_epoch,status,cancel_requested,final_content FROM delivery_responses WHERE response_id=$1 FOR NO KEY UPDATE",
                vec![entry.origin_response_id.into()],
            )).await?.ok_or_else(||anyhow::anyhow!("cache requires its committed response"))?;
            anyhow::ensure!(response.try_get::<Uuid>("","user_id")? == entry.context.owner_id
                && response.try_get::<Uuid>("","request_id")? == entry.context.request_id
                && response.try_get::<Option<Uuid>>("","context_id")? == Some(entry.context.context_id)
                && response.try_get::<Option<Uuid>>("","source_event_id")? == entry.context.source_event_id
                && response.try_get::<Option<i64>>("","source_privacy_epoch")? == Some(entry.context.source_privacy_epoch)
                && response.try_get::<String>("","status")? == "committed"
                && !response.try_get::<bool>("","cancel_requested")?
                && response.try_get::<Option<Value>>("","final_content")?
                    .as_ref().and_then(|value|value.get("text")).and_then(Value::as_str) == Some(&entry.response_text),
                "cache content or source differs from its committed response");
            let hash = embedding_text_digest(&entry.query_text);
            let existing = tx.query_one(stmt(&format!(
                "{SNAPSHOT} WHERE c.user_id=$1 AND c.scope=$2 AND c.query_hash=$3 FOR UPDATE"
            ),vec![entry.context.owner_id.into(),entry.scope.clone().into(),hash.clone().into()]))
                .await?.map(snapshot).transpose()?;
            if let Some(old) = &existing {
                anyhow::ensure!(old.query_text == entry.query_text,"cache query digest conflict");
                anyhow::ensure!(!validated.lease.dependencies().iter().any(|evidence|
                    matches!(evidence,ContextEvidence::Cache{id,..} if *id==old.version.cache_id)),
                    "cache replacement would invalidate its own generation source");
                // A repeated post-commit callback cannot reset an already reviewed
                // entry or create a new revision for the same canonical response.
                if old.origin_response_id == entry.origin_response_id {
                    anyhow::ensure!(old.version.context_id == entry.context.context_id,
                        "cache response reused with different context");
                    tx.commit().await?;
                    return Ok(old.clone());
                }
            }
            let ttl = entry.ttl_seconds.min(
                validated.policy.cache_default_ttl_seconds.max(validated.policy.cache_skill_ttl_seconds)
            );
            anyhow::ensure!(ttl > 0 && ttl <= i64::MAX as u64,"cache TTL is disabled or exceeds its bound");
            let digest = response_digest(&entry.response_text)?;
            let mut metadata = entry.metadata;
            metadata["schema"]=json!(CACHE_METADATA_SCHEMA);
            metadata["policy"]=json!("private_exact");
            metadata["status"]=json!("pending_insights_review");
            metadata["safe_direct_return"]=json!(false);
            let row = tx.query_one(stmt(
                "INSERT INTO llm_cache(id,user_id,scope,query_hash,query_text,response_text,metadata,revision,context_id,origin_response_id,status,content_digest,hit_count,created_at,updated_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,1,$8,$9,'pending_insights_review',$10,0,clock_timestamp(),clock_timestamp(),LEAST(clock_timestamp()+($11::bigint*interval '1 second'),$12::timestamptz)) ON CONFLICT(user_id,scope,query_hash) DO UPDATE SET response_text=EXCLUDED.response_text,metadata=EXCLUDED.metadata,revision=llm_cache.revision+1,context_id=EXCLUDED.context_id,origin_response_id=EXCLUDED.origin_response_id,status=EXCLUDED.status,content_digest=EXCLUDED.content_digest,hit_count=0,updated_at=clock_timestamp(),expires_at=EXCLUDED.expires_at RETURNING id",
                vec![Uuid::new_v4().into(),entry.context.owner_id.into(),entry.scope.into(),hash.into(),
                    entry.query_text.into(),entry.response_text.into(),metadata.into(),
                    entry.context.context_id.into(),entry.origin_response_id.into(),digest.into(),
                    (ttl as i64).into(),validated.lease.valid_until().into()],
            )).await?.ok_or_else(||anyhow::anyhow!("cache upsert was not acknowledged"))?;
            let id: Uuid = row.try_get("","id")?;
            tx.execute(stmt("DELETE FROM llm_cache_embeddings WHERE cache_id=$1",vec![id.into()])).await?;
            let result = by_id(&tx,id).await?;
            tx.commit().await?;
            Ok(result)
        }).await
    }

    async fn get_semantic_candidates(
        &self,
        owner: Uuid,
        scope: &str,
        profile: &EmbeddingProfile,
        query: &EmbeddingVector,
        limit: usize,
    ) -> anyhow::Result<Vec<SemanticCacheCandidate>> {
        scope_valid(scope)?;
        query.validate(profile)?;
        if limit == 0 {
            return Ok(vec![]);
        }
        bounded(async {
            let tx = self.db()?.begin().await?;
            let policy = owner_gate(&tx,owner,ContextUse::CacheReuse).await?;
            if !policy.policy.cache_semantic_enabled { return Ok(vec![]); }
            let profile_id = profile.id()?;
            anyhow::ensure!(policy.policy.cache_embedding_profile_id.as_deref()==Some(profile_id.as_str()),
                "semantic query profile differs from current policy");
            // Materializing the exact profile partition prevents evaluating a
            // distance operator against another profile's vector dimensions.
            let rows = tx.query_all(stmt(
                "WITH matching AS MATERIALIZED (SELECT c.id,e.embedding FROM llm_cache c JOIN llm_cache_embeddings e ON e.cache_id=c.id AND e.cache_revision=c.revision AND e.context_id=c.context_id AND e.query_digest=c.query_hash JOIN embedding_profiles p ON p.id=e.profile_id WHERE c.user_id=$1 AND c.scope=$2 AND c.status='active' AND c.expires_at>clock_timestamp() AND e.profile_id=$3 AND e.dimensions=$4 AND p.request_descriptor=$5) SELECT id,(1.0-(embedding <=> $6::vector))::real AS similarity FROM matching ORDER BY embedding <=> $6::vector,id LIMIT $7",
                vec![owner.into(),scope.into(),profile_id.into(),(profile.dimensions as i32).into(),
                    serde_json::to_value(profile)?.into(),vector_text(query).into(),
                    (limit.min(10) as i64).into()],
            )).await?;
            let mut results=Vec::new();
            for row in rows {
                let similarity: f32=row.try_get("","similarity")?;
                if !similarity.is_finite() || similarity<policy.policy.cache_semantic_threshold { continue; }
                let old=by_id(&tx,row.try_get("","id")?).await?;
                if let Some(cached)=current_version(&tx,&old,ContextUse::CacheReuse,"FOR SHARE").await? {
                    if cached.status==CacheStatus::Active {
                        results.push(SemanticCacheCandidate{cached,similarity});
                    }
                }
            }
            tx.commit().await?;
            Ok(results)
        }).await
    }

    async fn load_pending(
        &self,
        owner: Uuid,
        scope: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<CachedResponse>> {
        if let Some(scope) = scope {
            scope_valid(scope)?;
        }
        if limit == 0 {
            return Ok(vec![]);
        }
        bounded(async {
            let tx=self.db()?.begin().await?;
            owner_gate(&tx,owner,ContextUse::CacheReview).await?;
            let rows=tx.query_all(stmt(&format!(
                "{SNAPSHOT} WHERE c.user_id=$1 AND ($2::text IS NULL OR c.scope=$2) AND c.status='pending_insights_review' AND c.expires_at>clock_timestamp() ORDER BY c.created_at,c.id LIMIT $3"
            ),vec![owner.into(),scope.map(ToOwned::to_owned).into(),(limit.min(100) as i64).into()])).await?;
            let mut result=Vec::new();
            for row in rows {
                let old=snapshot(row)?;
                if let Some(current)=current_version(&tx,&old,ContextUse::CacheReview,"FOR SHARE").await? {
                    if current.status==CacheStatus::PendingInsightsReview {result.push(current);}
                }
            }
            tx.commit().await?;
            Ok(result)
        }).await
    }

    async fn apply_review(
        &self,
        expected: &CachedResponse,
        review: CacheReview,
    ) -> anyhow::Result<CacheCas<Option<CachedResponse>>> {
        expected_valid(expected)?;
        if let CacheReview::Approve {
            response_text,
            metadata,
        } = &review
        {
            content_valid(response_text, metadata)?;
            anyhow::ensure!(
                safe_review_metadata(metadata),
                "cache review lacks required admission flags"
            );
        }
        bounded(async {
            let tx=self.db()?.begin().await?;
            owner_gate(&tx,expected.owner_id,ContextUse::CacheReview).await?;
            let Some(current)=current_version(&tx,expected,ContextUse::CacheReview,"FOR UPDATE").await?
                else {return Ok(CacheCas::Stale);};
            if current.status!=CacheStatus::PendingInsightsReview {return Ok(CacheCas::Stale);}
            let result=match review {
                CacheReview::Reject => {
                    let changed=tx.execute(stmt("DELETE FROM llm_cache WHERE id=$1 AND revision=$2 AND context_id=$3",
                        vec![expected.version.cache_id.into(),expected.version.revision.into(),expected.version.context_id.into()])).await?;
                    anyhow::ensure!(changed.rows_affected()==1,"cache review CAS changed no row");
                    None
                }
                CacheReview::Approve{response_text,mut metadata} => {
                    metadata["status"]=json!("active");
                    let digest=response_digest(&response_text)?;
                    let changed=tx.execute(stmt(
                        "UPDATE llm_cache SET response_text=$4,metadata=$5,content_digest=$6,status='active',revision=revision+1,updated_at=clock_timestamp() WHERE id=$1 AND revision=$2 AND context_id=$3",
                        vec![expected.version.cache_id.into(),expected.version.revision.into(),
                            expected.version.context_id.into(),response_text.into(),metadata.into(),digest.into()],
                    )).await?;
                    anyhow::ensure!(changed.rows_affected()==1,"cache review CAS changed no row");
                    tx.execute(stmt("DELETE FROM llm_cache_embeddings WHERE cache_id=$1",
                        vec![expected.version.cache_id.into()])).await?;
                    Some(by_id(&tx,expected.version.cache_id).await?)
                }
            };
            tx.commit().await?;
            Ok(CacheCas::Applied(result))
        }).await
    }

    async fn authorize_work(
        &self,
        expected: &CachedResponse,
        work: CacheWork,
    ) -> anyhow::Result<bool> {
        let (purpose, status) = match work {
            CacheWork::Review => (ContextUse::CacheReview, CacheStatus::PendingInsightsReview),
            CacheWork::Embedding => (ContextUse::CacheEmbedding, CacheStatus::Active),
        };
        bounded(async {
            let tx = self.db()?.begin().await?;
            owner_gate(&tx, expected.owner_id, purpose).await?;
            let valid = current_version(&tx, expected, purpose, "FOR SHARE")
                .await?
                .is_some_and(|current| current.status == status);
            tx.commit().await?;
            Ok(valid)
        })
        .await
    }

    async fn fill_embedding(
        &self,
        expected: &CachedResponse,
        profile: &EmbeddingProfile,
        embedding: &EmbeddingVector,
    ) -> anyhow::Result<CacheCas<()>> {
        expected_valid(expected)?;
        embedding.validate(profile)?;
        anyhow::ensure!(
            embedding.input_digest == expected.query_digest,
            "embedding belongs to another query"
        );
        bounded(async {
            let tx=self.db()?.begin().await?;
            let policy=owner_gate(&tx,expected.owner_id,ContextUse::CacheEmbedding).await?;
            anyhow::ensure!(policy.policy.cache_embedding_profile_id.as_deref()==Some(profile.id()?.as_str()),
                "embedding profile differs from current policy");
            let Some(current)=current_version(&tx,expected,ContextUse::CacheEmbedding,"FOR UPDATE").await?
                else {return Ok(CacheCas::Stale);};
            if current.status!=CacheStatus::Active {return Ok(CacheCas::Stale);}
            let profile_id=ensure_profile(&tx,profile).await?;
            let inserted=tx.execute(stmt(
                "INSERT INTO llm_cache_embeddings(cache_id,cache_revision,context_id,query_digest,profile_id,dimensions,embedding) SELECT id,revision,context_id,query_hash,$4,$5,$6::vector FROM llm_cache WHERE id=$1 AND revision=$2 AND context_id=$3 AND status='active' AND expires_at>clock_timestamp() ON CONFLICT(cache_id,profile_id) DO NOTHING",
                vec![expected.version.cache_id.into(),expected.version.revision.into(),
                    expected.version.context_id.into(),profile_id.into(),(profile.dimensions as i32).into(),
                    vector_text(embedding).into()],
            )).await?;
            if inserted.rows_affected()==0 {return Ok(CacheCas::Stale);}
            tx.commit().await?;
            Ok(CacheCas::Applied(()))
        }).await
    }

    async fn cleanup_expired(&self, limit: usize) -> anyhow::Result<u64> {
        cleanup_expired_in_db(self.db()?, limit).await
    }
}

async fn cleanup_expired_in_db(db: &DatabaseConnection, limit: usize) -> anyhow::Result<u64> {
    if limit == 0 {
        return Ok(0);
    }
    bounded(async {
        // Lock the selected versions before deleting them. A concurrent put
        // retains the same id, so an id-only stale candidate could otherwise
        // wait for that writer and then delete its refreshed row and vectors.
        // Busy rows are left for the next batch instead of delaying writers.
        let result = db.execute(stmt(
            "WITH expired AS MATERIALIZED (SELECT id,revision,context_id FROM llm_cache WHERE expires_at<=clock_timestamp() ORDER BY expires_at,id LIMIT $1 FOR UPDATE SKIP LOCKED) DELETE FROM llm_cache c USING expired e WHERE c.id=e.id AND c.revision=e.revision AND c.context_id=e.context_id AND c.expires_at<=clock_timestamp()",
            vec![(limit.min(256) as i64).into()],
        )).await?;
        Ok(result.rows_affected())
    }).await
}
