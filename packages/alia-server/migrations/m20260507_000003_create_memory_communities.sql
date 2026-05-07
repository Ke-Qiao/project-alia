-- Migration: Create memory_communities table for GraphRAG topic clustering

-- Communities (GraphRAG topic clusters for global retrieval)
CREATE TABLE memory_communities (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL,
    title VARCHAR(200) NOT NULL,             -- Community topic (LLM generated)
    summary TEXT NOT NULL,                   -- Community summary (LLM generated)
    node_ids UUID[] NOT NULL,                -- Member node IDs
    level INTEGER DEFAULT 0,                 -- Hierarchy level (0 = finest granularity)
    embedding VECTOR(1024),                  -- Summary embedding for global search
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- Indexes
CREATE INDEX idx_memory_communities_user_id ON memory_communities(user_id);
CREATE INDEX idx_memory_communities_level ON memory_communities(level);
CREATE INDEX idx_memory_communities_embedding ON memory_communities
    USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 64);
