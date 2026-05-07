-- Migration: Create llm_cache table for DB-backed LLM response caching
-- Supports exact (hash) matching and semantic (vector) matching

CREATE TABLE llm_cache (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID,                            -- NULL = global/shared cache entry
    scope VARCHAR(100) NOT NULL DEFAULT 'default',  -- Cache scope (e.g., 'chat', 'skill', 'tool')
    query_hash VARCHAR(64) NOT NULL,         -- SHA-256 of scope:user_id:query for exact match
    query_text TEXT NOT NULL,                -- Original query text
    response_text TEXT NOT NULL,             -- Cached response
    metadata JSONB DEFAULT '{}',             -- Additional context (model, temperature, etc.)
    embedding VECTOR(1024),                  -- Query embedding for semantic match
    hit_count INTEGER DEFAULT 0,             -- Number of cache hits
    created_at TIMESTAMPTZ DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL          -- TTL-based expiration
);

-- Exact match index (primary lookup path)
CREATE UNIQUE INDEX idx_llm_cache_query_hash ON llm_cache(query_hash);

-- User + scope index for filtered queries and invalidation
CREATE INDEX idx_llm_cache_user_scope ON llm_cache(user_id, scope);

-- Expiration index for cleanup
CREATE INDEX idx_llm_cache_expires ON llm_cache(expires_at);

-- Vector similarity index for semantic matching (HNSW)
CREATE INDEX idx_llm_cache_embedding ON llm_cache
    USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 64);
