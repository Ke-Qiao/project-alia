-- Migration: Create memory_nodes table for GraphRAG knowledge graph
-- Layer: L1 (episodic), L2 (core), L3 (procedural)

-- Enable pgvector extension
CREATE EXTENSION IF NOT EXISTS vector;

-- Graph nodes (entities + memories)
CREATE TABLE memory_nodes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL,
    memory_layer VARCHAR(20) NOT NULL,       -- 'episodic' | 'core' | 'procedural'
    node_type VARCHAR(50) NOT NULL,          -- 'entity' | 'fact' | 'preference' | 'habit' | 'event' | 'person'
    content TEXT NOT NULL,                   -- Natural language description
    metadata JSONB DEFAULT '{}',             -- Additional structured data
    embedding VECTOR(1024),                  -- pgvector, dimension matches embedding model
    importance FLOAT DEFAULT 0.5,            -- 0.0-1.0, affects retrieval priority
    access_count INTEGER DEFAULT 0,          -- Access count (for reinforcement/decay)
    last_accessed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    expires_at TIMESTAMPTZ                   -- NULL = never expires, episodic has TTL
);

-- Indexes
CREATE INDEX idx_memory_nodes_user_id ON memory_nodes(user_id);
CREATE INDEX idx_memory_nodes_layer ON memory_nodes(memory_layer);
CREATE INDEX idx_memory_nodes_type ON memory_nodes(node_type);
CREATE INDEX idx_memory_nodes_importance ON memory_nodes(importance DESC);
CREATE INDEX idx_memory_nodes_expires ON memory_nodes(expires_at) WHERE expires_at IS NOT NULL;

-- Vector similarity index (HNSW, suitable for medium scale)
CREATE INDEX idx_memory_nodes_embedding ON memory_nodes
    USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 64);

-- Full-text search index on content
CREATE INDEX idx_memory_nodes_content_fts ON memory_nodes 
    USING gin (to_tsvector('simple', content));
