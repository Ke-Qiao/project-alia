//! Two-Layer Cache Backend
//!
//! A response cache that supports two layers of matching:
//! - L1: Exact match using query hash (SHA-256)
//! - L2: Semantic match using embedding similarity (optional)
//!
//! This implementation uses in-memory storage. For production, consider
//! using PostgreSQL or Redis.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{CacheBackend, CacheEntry, CacheFilter, CachedResponse};

/// Cache entry with expiration
struct CacheItem {
    response: CachedResponse,
    scope: String,
    user_id: Option<Uuid>,
    expires_at: Instant,
}

/// Two-layer cache backend
pub struct TwoLayerCache {
    /// L1: Exact match cache (key = SHA-256 of scope:user_id:query)
    exact_cache: Arc<RwLock<HashMap<String, CacheItem>>>,
    /// Default TTL for cache entries
    default_ttl: Duration,
}

impl TwoLayerCache {
    pub fn new() -> Self {
        Self {
            exact_cache: Arc::new(RwLock::new(HashMap::new())),
            default_ttl: Duration::from_secs(3600), // 1 hour default
        }
    }

    /// Generate cache key from query components
    fn make_key(scope: &str, user_id: Option<Uuid>, query: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(scope.as_bytes());
        hasher.update(b":");
        if let Some(uid) = user_id {
            hasher.update(uid.to_string().as_bytes());
        } else {
            hasher.update(b"global");
        }
        hasher.update(b":");
        hasher.update(query.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Cleanup expired entries
    async fn cleanup_expired(&self) {
        let mut cache = self.exact_cache.write().await;
        let now = Instant::now();
        cache.retain(|_, item| item.expires_at > now);
    }
}

impl Default for TwoLayerCache {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for TwoLayerCache {
    fn name(&self) -> &'static str {
        "two_layer"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        // Load config
        self.default_ttl = Duration::from_secs(ctx.config.cache.default_ttl_seconds);

        tracing::info!(
            ttl_seconds = self.default_ttl.as_secs(),
            "TwoLayerCache initialized"
        );
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}

#[async_trait]
impl CacheBackend for TwoLayerCache {
    async fn get(
        &self,
        user_id: Option<Uuid>,
        query: &str,
    ) -> anyhow::Result<Option<CachedResponse>> {
        // Try exact match first (L1)
        // Use "default" scope for basic queries
        let key = Self::make_key("default", user_id, query);

        let cache = self.exact_cache.read().await;
        if let Some(item) = cache.get(&key) {
            if item.expires_at > Instant::now() {
                tracing::debug!(query = %query, "cache hit (exact)");
                return Ok(Some(item.response.clone()));
            }
        }

        // TODO: L2 semantic match would go here
        // This would require access to embedding module and a vector index

        tracing::debug!(query = %query, "cache miss");
        Ok(None)
    }

    async fn put(&self, entry: CacheEntry) -> anyhow::Result<()> {
        let key = Self::make_key(&entry.scope, entry.user_id, &entry.query_text);

        let ttl = if entry.ttl_seconds > 0 {
            Duration::from_secs(entry.ttl_seconds)
        } else {
            self.default_ttl
        };

        let item = CacheItem {
            response: CachedResponse {
                response_text: entry.response_text,
                metadata: entry.metadata,
            },
            scope: entry.scope,
            user_id: entry.user_id,
            expires_at: Instant::now() + ttl,
        };

        let mut cache = self.exact_cache.write().await;
        cache.insert(key, item);

        tracing::debug!(
            query = %entry.query_text,
            ttl_seconds = ttl.as_secs(),
            "cache put"
        );

        // Periodic cleanup (every 100 inserts or so)
        if cache.len() % 100 == 0 {
            drop(cache); // Release write lock before cleanup
            self.cleanup_expired().await;
        }

        Ok(())
    }

    async fn invalidate(&self, filter: CacheFilter) -> anyhow::Result<u64> {
        let mut cache = self.exact_cache.write().await;
        let original_len = cache.len();

        cache.retain(|_, item| {
            // Keep item if it doesn't match filter
            let user_match = filter.user_id.is_none() || item.user_id == filter.user_id;
            let scope_match = filter.scope.is_none() || Some(&item.scope) == filter.scope.as_ref();

            // Remove if matches filter
            !(user_match && scope_match)
        });

        let removed = (original_len - cache.len()) as u64;
        tracing::info!(removed_count = removed, "cache invalidated");

        Ok(removed)
    }
}
