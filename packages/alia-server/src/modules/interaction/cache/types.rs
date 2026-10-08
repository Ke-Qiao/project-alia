//! Cache identity, admission and storage contract owned by Interaction.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::providers::adapter::{EmbeddingProfile, EmbeddingVector};

pub const DEFAULT_CACHE_SCOPE: &str = "default";
pub const CACHE_METADATA_SCHEMA: &str = "alia.cache_metadata.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheVersion {
    pub cache_id: Uuid,
    pub revision: i64,
    pub context_id: Uuid,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CacheStatus {
    PendingInsightsReview,
    Active,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    pub version: CacheVersion,
    pub owner_id: Uuid,
    pub scope: String,
    pub query_digest: String,
    pub content_digest: String,
    pub origin_response_id: Uuid,
    pub query_text: String,
    pub response_text: String,
    pub metadata: serde_json::Value,
    pub status: CacheStatus,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

impl CachedResponse {
    pub fn context_identity(&self) -> crate::modules::memory::context::CacheIdentity {
        crate::modules::memory::context::CacheIdentity {
            cache_id: self.version.cache_id,
            revision: self.version.revision,
            context_id: self.version.context_id,
            scope: self.scope.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCacheCandidate {
    pub cached: CachedResponse,
    pub similarity: f32,
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub context: crate::modules::memory::context::ContextBinding,
    pub origin_response_id: Uuid,
    pub scope: String,
    pub query_text: String,
    pub response_text: String,
    pub metadata: serde_json::Value,
    pub ttl_seconds: u64,
}

#[derive(Debug, Clone)]
pub enum CacheReview {
    Approve {
        response_text: String,
        metadata: serde_json::Value,
    },
    Reject,
}

#[derive(Debug, Clone, Copy)]
pub enum CacheWork {
    Review,
    Embedding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheCas<T> {
    Applied(T),
    Stale,
}

#[async_trait]
pub trait CacheStore: Send + Sync {
    async fn get(
        &self,
        owner_id: Uuid,
        scope: &str,
        query: &str,
    ) -> anyhow::Result<Option<CachedResponse>>;
    async fn put(&self, entry: CacheEntry) -> anyhow::Result<CachedResponse>;
    async fn get_semantic_candidates(
        &self,
        owner_id: Uuid,
        scope: &str,
        profile: &EmbeddingProfile,
        query_embedding: &EmbeddingVector,
        limit: usize,
    ) -> anyhow::Result<Vec<SemanticCacheCandidate>>;
    async fn load_pending(
        &self,
        owner_id: Uuid,
        scope: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<CachedResponse>>;
    async fn apply_review(
        &self,
        expected: &CachedResponse,
        review: CacheReview,
    ) -> anyhow::Result<CacheCas<Option<CachedResponse>>>;
    /// Recheck immediately before sending private query/response text to a model.
    async fn authorize_work(
        &self,
        expected: &CachedResponse,
        work: CacheWork,
    ) -> anyhow::Result<bool>;
    async fn fill_embedding(
        &self,
        expected: &CachedResponse,
        profile: &EmbeddingProfile,
        embedding: &EmbeddingVector,
    ) -> anyhow::Result<CacheCas<()>>;
    async fn cleanup_expired(&self, limit: usize) -> anyhow::Result<u64>;
}
