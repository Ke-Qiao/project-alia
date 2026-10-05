use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    ContextError, ContextResult, ContextUse,
    repository::{digest, stmt},
};
use crate::config::{
    CacheSection, ConversationSection, MemoryCaptureMode, MemorySection, ServerConfig,
};

/// One deployment policy, persisted before publishing any runtime projection.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContextPolicy {
    pub appraisal: super::AppraisalPolicy,
    pub cache_enabled: bool,
    pub cache_exact_enabled: bool,
    pub cache_semantic_enabled: bool,
    pub cache_semantic_threshold: f32,
    pub cache_default_ttl_seconds: u64,
    pub cache_skill_ttl_seconds: u64,
    pub cache_embedding_profile_id: Option<String>,
    pub memory_enabled: bool,
    pub consolidation_enabled: bool,
    pub graph_depth: u32,
    pub capture_mode: MemoryCaptureMode,
    pub sensitive_filter_enabled: bool,
    pub paused_until: Option<DateTime<Utc>>,
    /// The actual request profile approved for Memory's derived index. This
    /// is populated from the provider preview, never a free-form model alias.
    pub active_embedding_profile: Option<String>,
    pub retrieval_layers: Vec<String>,
    pub max_memory_chars: usize,
    pub max_memory_results: usize,
    pub similarity_threshold: f32,
    pub max_history_messages: usize,
    pub capture_min_confidence: f32,
    pub capture_min_importance: f32,
}
impl ContextPolicy {
    pub fn from_config(
        memory: &MemorySection,
        history: &ConversationSection,
    ) -> ContextResult<Self> {
        let paused_until = memory
            .paused_until
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| DateTime::parse_from_rfc3339(s).map(|t| t.with_timezone(&Utc)))
            .transpose()?;
        let value = Self {
            appraisal: super::AppraisalPolicy::default(),
            cache_enabled: true,
            cache_exact_enabled: true,
            cache_semantic_enabled: false,
            cache_semantic_threshold: 0.9,
            cache_default_ttl_seconds: 3600,
            cache_skill_ttl_seconds: 3600,
            cache_embedding_profile_id: None,
            memory_enabled: memory.enabled,
            consolidation_enabled: memory.consolidation_enabled,
            graph_depth: memory.graph_depth.clamp(1, 4),
            capture_mode: memory.capture_mode,
            sensitive_filter_enabled: memory.sensitive_filter_enabled,
            paused_until,
            active_embedding_profile: None,
            retrieval_layers: crate::memory_policy::normalize_memory_layers(
                memory.retrieval_layers.clone(),
            ),
            max_memory_chars: memory.max_injected_memory_chars.clamp(256, 12_000),
            max_memory_results: (memory.max_retrieval_results as usize)
                .clamp(1, super::MAX_MEMORY_RESULTS),
            similarity_threshold: memory.similarity_threshold,
            max_history_messages: history
                .max_context_messages
                .clamp(1, super::MAX_HISTORY_MESSAGES),
            capture_min_confidence: memory.capture_min_confidence,
            capture_min_importance: memory.capture_min_importance,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn from_server_config(config: &ServerConfig) -> ContextResult<Self> {
        let mut policy = Self::from_config(&config.policies.memory, &config.policies.conversation)?
            .with_cache(&config.policies.cache)?;
        policy.cache_embedding_profile_id =
            crate::providers::adapter::EmbeddingProfile::from_server_config(config)?
                .map(|profile| profile.id())
                .transpose()?;
        policy.active_embedding_profile = policy.cache_embedding_profile_id.clone();
        policy.appraisal = super::AppraisalPolicy::from_config(config);
        policy.validate()?;
        Ok(policy)
    }
    pub fn with_cache(mut self, cache: &CacheSection) -> ContextResult<Self> {
        self.cache_enabled = cache.enabled;
        self.cache_exact_enabled = cache.exact_match_enabled;
        self.cache_semantic_enabled = cache.semantic_match_enabled;
        self.cache_semantic_threshold = cache.semantic_threshold;
        self.cache_default_ttl_seconds = cache.default_ttl_seconds;
        self.cache_skill_ttl_seconds = cache.skill_cache_ttl_seconds;
        self.validate()?;
        Ok(self)
    }
    pub fn validate(&self) -> ContextResult<()> {
        self.appraisal.validate()?;
        anyhow::ensure!(
            (256..=12_000).contains(&self.max_memory_chars)
                && (1..=super::MAX_MEMORY_RESULTS).contains(&self.max_memory_results)
                && (1..=super::MAX_HISTORY_MESSAGES).contains(&self.max_history_messages)
                && (1..=4).contains(&self.graph_depth),
            "invalid context policy bounds"
        );
        anyhow::ensure!(
            self.active_embedding_profile
                .as_ref()
                .is_none_or(|id| !id.is_empty() && id.len() <= 256)
                && self
                    .cache_embedding_profile_id
                    .as_ref()
                    .is_none_or(|id| !id.is_empty() && id.len() <= 256)
                && self.retrieval_layers.len() <= 16
                && self.retrieval_layers.iter().all(|v| v.len() <= 64),
            "invalid context profile or layers"
        );
        anyhow::ensure!(
            [
                self.similarity_threshold,
                self.capture_min_confidence,
                self.capture_min_importance,
                self.cache_semantic_threshold
            ]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            "invalid context policy threshold"
        );
        Ok(())
    }
    pub(crate) fn reads_memory(&self) -> bool {
        self.memory_enabled && self.paused_until.is_none()
    }
    pub(crate) fn allows(&self, purpose: ContextUse) -> bool {
        match purpose {
            ContextUse::AppraisalWrite => self.appraisal.affect.enabled,
            ContextUse::AffectRead | ContextUse::AppraisalMaintenance => true,
            ContextUse::AutomaticMemoryWrite => {
                self.reads_memory() && self.capture_mode == MemoryCaptureMode::LlmSelected
            }
            ContextUse::AiMemoryWrite => {
                self.reads_memory() && self.capture_mode != MemoryCaptureMode::None
            }
            ContextUse::MemoryMaintenance => self.reads_memory() && self.consolidation_enabled,
            ContextUse::MemoryEmbedding => {
                self.reads_memory() && self.active_embedding_profile.is_some()
            }
            ContextUse::CacheWrite | ContextUse::CacheReview => {
                self.cache_enabled && self.cache_exact_enabled
            }
            ContextUse::CacheReuse => {
                self.cache_enabled && (self.cache_exact_enabled || self.cache_semantic_enabled)
            }
            ContextUse::CacheEmbedding => self.cache_enabled && self.cache_semantic_enabled,
            _ => true,
        }
    }
}

pub(crate) async fn verify(
    tx: &DatabaseTransaction,
    expected: Option<i64>,
    purpose: ContextUse,
) -> ContextResult<PolicySnapshot> {
    let snapshot = read(tx, "FOR SHARE").await?;
    if expected.is_some_and(|version| version != snapshot.version) {
        return Err(ContextError::PolicyChanged.into());
    }
    let row = tx
        .query_one(stmt("SELECT clock_timestamp() AS now", vec![]))
        .await?
        .unwrap();
    let now: DateTime<Utc> = row.try_get("", "now")?;
    if snapshot
        .policy
        .paused_until
        .is_some_and(|until| until <= now)
        && purpose != ContextUse::ManualMemoryWrite
    {
        return Err(ContextError::PolicyChanged.into());
    }
    if !snapshot.policy.allows(purpose) {
        return Err(ContextError::Revoked("context policy disallows this operation".into()).into());
    }
    Ok(snapshot)
}

#[derive(Clone, Debug)]
pub struct PolicySnapshot {
    pub version: i64,
    pub policy: ContextPolicy,
}

#[derive(Clone)]
pub struct ContextPolicyStore {
    db: DatabaseConnection,
}
impl ContextPolicyStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
    pub async fn initialize_or_load(&self, seed: ContextPolicy) -> ContextResult<PolicySnapshot> {
        seed.validate()?;
        let value = serde_json::to_value(&seed)?;
        self.db.execute(stmt("INSERT INTO context_policy(singleton,version,policy,policy_digest) VALUES(TRUE,1,$1,$2) ON CONFLICT DO NOTHING", vec![value.clone().into(),digest(&value)?.into()])).await?;
        self.current().await
    }
    pub async fn current(&self) -> ContextResult<PolicySnapshot> {
        // A timed resume is a real policy transition, not local clock permission.
        let tx = self.db.begin().await?;
        let mut current = read(&tx, "FOR UPDATE").await?;
        let due = tx
            .query_one(stmt("SELECT clock_timestamp() AS now", vec![]))
            .await?
            .unwrap()
            .try_get::<DateTime<Utc>>("", "now")?;
        if current
            .policy
            .paused_until
            .is_some_and(|until| until <= due)
        {
            current.policy.paused_until = None;
            current.version = write(&tx, current.version, &current.policy).await?;
        }
        tx.commit().await?;
        Ok(current)
    }
    pub async fn replace(
        &self,
        expected_version: i64,
        next: ContextPolicy,
    ) -> ContextResult<PolicySnapshot> {
        self.replace_inner(expected_version, next, None, None).await
    }
    pub async fn replace_for_actor(
        &self,
        expected_version: i64,
        next: ContextPolicy,
        actor: uuid::Uuid,
    ) -> ContextResult<PolicySnapshot> {
        self.replace_inner(expected_version, next, Some(actor), None)
            .await
    }
    pub(crate) async fn replace_for_actor_with_embedding(
        &self,
        expected_version: i64,
        next: ContextPolicy,
        actor: uuid::Uuid,
        profile: Option<&crate::providers::adapter::EmbeddingProfile>,
    ) -> ContextResult<PolicySnapshot> {
        self.replace_inner(expected_version, next, Some(actor), profile)
            .await
    }
    async fn replace_inner(
        &self,
        expected_version: i64,
        next: ContextPolicy,
        actor: Option<uuid::Uuid>,
        profile: Option<&crate::providers::adapter::EmbeddingProfile>,
    ) -> ContextResult<PolicySnapshot> {
        next.validate()?;
        let tx = self.db.begin().await?;
        let current = read(&tx, "FOR UPDATE").await?;
        if let Some(actor) = actor {
            if tx.query_one(stmt("SELECT id FROM users WHERE id=$1 AND status='active' AND role='admin' FOR SHARE",vec![actor.into()])).await?.is_none() {return Err(ContextError::Forbidden.into());}
        }
        if current.version != expected_version {
            return Err(ContextError::PolicyChanged.into());
        }
        if let Some(profile) = profile {
            let id = profile.id()?;
            anyhow::ensure!(
                next.active_embedding_profile.as_deref() == Some(id.as_str())
                    && next.cache_embedding_profile_id.as_deref() == Some(id.as_str()),
                "embedding transition descriptor differs from policy pins"
            );
            // A new pin must be usable without restarting Memory or waiting for
            // Cache to register the descriptor as an unrelated side effect.
            crate::providers::embedding::profile_store::ensure_profile(&tx, profile).await?;
        }
        let version = if current.policy == next {
            current.version
        } else {
            write(&tx, current.version, &next).await?
        };
        tx.commit().await?;
        Ok(PolicySnapshot {
            version,
            policy: next,
        })
    }
}
async fn write(
    tx: &DatabaseTransaction,
    expected: i64,
    next: &ContextPolicy,
) -> ContextResult<i64> {
    let value = json!(next);
    let row = tx.query_one(stmt("UPDATE context_policy SET version=version+1,policy=$2,policy_digest=$3,updated_at=clock_timestamp() WHERE singleton AND version=$1 RETURNING version",vec![expected.into(),value.clone().into(),digest(&value)?.into()])).await?.ok_or(ContextError::PolicyChanged)?;
    Ok(row.try_get("", "version")?)
}
pub(crate) async fn read(tx: &DatabaseTransaction, lock: &str) -> ContextResult<PolicySnapshot> {
    let row = tx.query_one(stmt(&format!("SELECT version,policy,policy_digest,clock_timestamp() AS now FROM context_policy WHERE singleton {lock}"),vec![])).await?.ok_or_else(||ContextError::Unavailable("persistent policy has not been initialized".into()))?;
    let value: serde_json::Value = row.try_get("", "policy")?;
    // JSONB normalizes decimal text. Hash the reconstructed f32 policy rather
    // than a transient f64 Value produced while parsing its decimal spelling.
    let policy: ContextPolicy = serde_json::from_value(value)?;
    anyhow::ensure!(
        digest(&serde_json::to_value(&policy)?)? == row.try_get::<String>("", "policy_digest")?,
        "context policy digest mismatch"
    );
    policy.validate()?;
    Ok(PolicySnapshot {
        version: row.try_get("", "version")?,
        policy,
    })
}
