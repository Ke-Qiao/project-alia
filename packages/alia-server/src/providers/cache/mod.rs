//! Cache backends for response caching.
//!
//! Available implementations:
//! - `DbCache`: PostgreSQL-backed cache with exact hash + semantic vector matching
//! - `TwoLayerCache`: In-memory cache with exact hash matching + semantic fallback (planned)
//! - `NoopCache`: No-op placeholder (no caching)

pub mod db;
#[cfg(feature = "cache-semantic")]
pub mod two_layer;

pub use db::DbCache;
#[cfg(feature = "cache-semantic")]
pub use two_layer::TwoLayerCache;

use async_trait::async_trait;
use uuid::Uuid;

use crate::providers::adapter::{CacheBackend, CacheEntry, CacheFilter, CachedResponse};
use crate::module::ModuleLifecycle;

pub struct NoopCache;

#[async_trait]
impl ModuleLifecycle for NoopCache {
    fn name(&self) -> &'static str {
        "noop"
    }
}

#[async_trait]
impl CacheBackend for NoopCache {
    async fn get(
        &self,
        _user_id: Option<Uuid>,
        _query: &str,
    ) -> anyhow::Result<Option<CachedResponse>> {
        Ok(None)
    }

    async fn put(&self, _entry: CacheEntry) -> anyhow::Result<()> {
        Ok(())
    }

    async fn invalidate(&self, _filter: CacheFilter) -> anyhow::Result<u64> {
        Ok(0)
    }
}
