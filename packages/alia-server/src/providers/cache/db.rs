//! Database-Backed Cache Backend
//!
//! Stores LLM response cache in PostgreSQL with two matching modes:
//! - L1: Exact match using SHA-256 query hash
//! - L2: Semantic match using pgvector embedding similarity (optional)
//!
//! Supports TTL-based expiration with background cleanup.

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, Set, Statement,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::entity::llm_cache;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    CacheBackend, CacheEntry, CacheFilter, CachedResponse, DEFAULT_CACHE_SCOPE,
};

/// Database-backed cache backend
pub struct DbCache {
    db: Option<DatabaseConnection>,
    /// Default TTL for cache entries
    default_ttl: Duration,
    /// Semantic similarity threshold (0.0 - 1.0)
    semantic_threshold: f32,
    /// Whether semantic matching is enabled
    semantic_enabled: bool,
}

impl DbCache {
    pub fn new() -> Self {
        Self {
            db: None,
            default_ttl: Duration::from_secs(3600),
            semantic_threshold: 0.85,
            semantic_enabled: false,
        }
    }

    /// Generate cache key hash from query components
    fn make_hash(scope: &str, user_id: Option<Uuid>, query: &str) -> String {
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

    fn db(&self) -> anyhow::Result<&DatabaseConnection> {
        self.db
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("DbCache not initialized - call init() first"))
    }

    /// Try exact hash match (L1)
    async fn exact_match(
        &self,
        user_id: Option<Uuid>,
        query: &str,
    ) -> anyhow::Result<Option<CachedResponse>> {
        let db = self.db()?;
        let hash = Self::make_hash(DEFAULT_CACHE_SCOPE, user_id, query);

        let entry = llm_cache::Entity::find()
            .filter(llm_cache::Column::QueryHash.eq(&hash))
            .filter(llm_cache::Column::ExpiresAt.gt(Utc::now()))
            .one(db)
            .await?;

        if let Some(entry) = entry {
            // Increment hit count
            let mut active: llm_cache::ActiveModel = entry.clone().into();
            active.hit_count = Set(entry.hit_count + 1);
            active.update(db).await?;

            tracing::debug!(query_hash = %hash, "cache hit (exact)");
            return Ok(Some(CachedResponse {
                response_text: entry.response_text,
                metadata: entry.metadata,
            }));
        }

        Ok(None)
    }

    /// Try semantic similarity match (L2) via pgvector
    async fn semantic_match(
        &self,
        user_id: Option<Uuid>,
        query_embedding: &[f32],
    ) -> anyhow::Result<Option<CachedResponse>> {
        let db = self.db()?;

        let vector_str = format!(
            "[{}]",
            query_embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let user_filter = if let Some(uid) = user_id {
            format!("AND (user_id = '{}' OR user_id IS NULL)", uid)
        } else {
            "AND user_id IS NULL".to_string()
        };

        let sql = format!(
            r#"
            SELECT 
                id, response_text, metadata, hit_count,
                1 - (embedding <=> $1::vector) as similarity
            FROM llm_cache
            WHERE embedding IS NOT NULL
                AND expires_at > NOW()
                {}
            ORDER BY embedding <=> $1::vector
            LIMIT 1
            "#,
            user_filter
        );

        let results = db
            .query_all(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                &sql,
                [vector_str.into()],
            ))
            .await?;

        if let Some(row) = results.first() {
            let similarity: f32 = row.try_get("", "similarity").unwrap_or(0.0);

            if similarity >= self.semantic_threshold {
                let id_str: String = row.try_get("", "id")?;
                let id = Uuid::parse_str(&id_str)?;
                let hit_count: i32 = row.try_get("", "hit_count")?;

                // Increment hit count
                let node = llm_cache::Entity::find_by_id(id).one(db).await?;
                if let Some(node) = node {
                    let mut active: llm_cache::ActiveModel = node.into();
                    active.hit_count = Set(hit_count + 1);
                    active.update(db).await?;
                }

                tracing::debug!(similarity = similarity, "cache hit (semantic)");
                return Ok(Some(CachedResponse {
                    response_text: row.try_get("", "response_text")?,
                    metadata: row.try_get("", "metadata")?,
                }));
            }
        }

        Ok(None)
    }

    /// Delete expired entries
    async fn cleanup_expired(&self) -> anyhow::Result<u64> {
        let db = self.db()?;

        let result = llm_cache::Entity::delete_many()
            .filter(llm_cache::Column::ExpiresAt.lt(Utc::now()))
            .exec(db)
            .await?;

        if result.rows_affected > 0 {
            tracing::info!(
                removed = result.rows_affected,
                "expired cache entries cleaned"
            );
        }

        Ok(result.rows_affected)
    }
}

impl Default for DbCache {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for DbCache {
    fn name(&self) -> &'static str {
        "db"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        self.db = Some(ctx.db.clone());
        self.default_ttl = Duration::from_secs(ctx.config.cache.default_ttl_seconds);
        self.semantic_threshold = ctx.config.cache.semantic_threshold;
        self.semantic_enabled = ctx.config.cache.semantic_match_enabled;

        // Run initial cleanup of expired entries
        self.cleanup_expired().await?;

        tracing::info!(
            ttl_seconds = self.default_ttl.as_secs(),
            semantic_enabled = self.semantic_enabled,
            semantic_threshold = self.semantic_threshold,
            "DbCache initialized"
        );
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if self.db.is_some() {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Unhealthy("Database not initialized".into())
        }
    }
}

#[async_trait]
impl CacheBackend for DbCache {
    async fn get(
        &self,
        user_id: Option<Uuid>,
        query: &str,
    ) -> anyhow::Result<Option<CachedResponse>> {
        // L1: Exact hash match
        if let Some(cached) = self.exact_match(user_id, query).await? {
            return Ok(Some(cached));
        }

        // L2: Semantic match (requires external embedding — skipped if no embedding stored)
        // Semantic matching is handled at a higher level where embedding adapter is available.
        // The `get_semantic` method can be called directly when an embedding is provided.

        tracing::debug!(query = %query, "cache miss");
        Ok(None)
    }

    async fn put(&self, entry: CacheEntry) -> anyhow::Result<()> {
        let db = self.db()?;
        let hash = Self::make_hash(&entry.scope, entry.user_id, &entry.query_text);

        let ttl = if entry.ttl_seconds > 0 {
            Duration::from_secs(entry.ttl_seconds)
        } else {
            self.default_ttl
        };

        let expires_at = Utc::now() + chrono::Duration::from_std(ttl)?;

        // Upsert: update if same hash exists, insert otherwise
        let existing = llm_cache::Entity::find()
            .filter(llm_cache::Column::QueryHash.eq(&hash))
            .one(db)
            .await?;

        if let Some(existing) = existing {
            // Update existing entry
            let mut active: llm_cache::ActiveModel = existing.into();
            active.response_text = Set(entry.response_text);
            active.metadata = Set(entry.metadata);
            active.expires_at = Set(expires_at);
            active.hit_count = Set(0); // Reset hit count on update
            active.update(db).await?;

            tracing::debug!(query_hash = %hash, "cache updated");
        } else {
            // Insert new entry
            let model = llm_cache::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(entry.user_id),
                scope: Set(entry.scope),
                query_hash: Set(hash.clone()),
                query_text: Set(entry.query_text),
                response_text: Set(entry.response_text),
                metadata: Set(entry.metadata),
                embedding: Set(None), // Set separately via update_embedding
                hit_count: Set(0),
                created_at: Set(Utc::now()),
                expires_at: Set(expires_at),
            };

            model.insert(db).await?;
            tracing::debug!(query_hash = %hash, ttl_seconds = ttl.as_secs(), "cache put");
        }

        Ok(())
    }

    async fn invalidate(&self, filter: CacheFilter) -> anyhow::Result<u64> {
        let db = self.db()?;

        let mut query = llm_cache::Entity::delete_many();

        if let Some(user_id) = filter.user_id {
            query = query.filter(llm_cache::Column::UserId.eq(user_id));
        }

        if let Some(scope) = filter.scope {
            query = query.filter(llm_cache::Column::Scope.eq(scope));
        }

        let result = query.exec(db).await?;

        tracing::info!(removed = result.rows_affected, "cache invalidated");
        Ok(result.rows_affected)
    }
}

impl DbCache {
    /// Semantic cache lookup with a pre-computed query embedding.
    ///
    /// Call this directly when the embedding adapter is available
    /// at the call site (e.g., in the chat pipeline).
    pub async fn get_semantic(
        &self,
        user_id: Option<Uuid>,
        query_embedding: &[f32],
    ) -> anyhow::Result<Option<CachedResponse>> {
        if !self.semantic_enabled {
            return Ok(None);
        }
        self.semantic_match(user_id, query_embedding).await
    }

    /// Store the query embedding for an existing cache entry (by hash).
    ///
    /// Called after the embedding adapter produces a vector for the query.
    pub async fn update_embedding(
        &self,
        user_id: Option<Uuid>,
        query: &str,
        scope: &str,
        embedding: &[f32],
    ) -> anyhow::Result<()> {
        let db = self.db()?;
        let hash = Self::make_hash(scope, user_id, query);

        let vector_str = format!(
            "[{}]",
            embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let sql = r#"
            UPDATE llm_cache
            SET embedding = $1::vector
            WHERE query_hash = $2
        "#;

        db.execute(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            sql,
            [vector_str.into(), hash.into()],
        ))
        .await?;

        Ok(())
    }

    /// Get cache statistics
    pub async fn stats(&self) -> anyhow::Result<CacheStats> {
        let db = self.db()?;

        let total: u64 = llm_cache::Entity::find().count(db).await?;

        let active: u64 = llm_cache::Entity::find()
            .filter(llm_cache::Column::ExpiresAt.gt(Utc::now()))
            .count(db)
            .await?;

        let expired = total.saturating_sub(active);

        Ok(CacheStats {
            total_entries: total,
            active_entries: active,
            expired_entries: expired,
        })
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total_entries: u64,
    pub active_entries: u64,
    pub expired_entries: u64,
}
