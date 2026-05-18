//! Cache backends for response caching.
//!
//! Available implementations:
//! - `DbCache`: PostgreSQL-backed cache with exact hash + semantic vector matching
//! - `NoopCache`: No-op placeholder (no caching)

pub mod db;

pub use db::DbCache;

use async_trait::async_trait;
use uuid::Uuid;

use crate::module::ModuleLifecycle;
use crate::providers::adapter::{CacheBackend, CacheEntry, CacheFilter, CachedResponse};

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
