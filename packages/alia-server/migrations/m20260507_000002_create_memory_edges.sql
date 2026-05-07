-- Migration: Create memory_edges table for GraphRAG relationships

-- Graph edges (relationships between entities)
CREATE TABLE memory_edges (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id UUID NOT NULL REFERENCES memory_nodes(id) ON DELETE CASCADE,
    target_id UUID NOT NULL REFERENCES memory_nodes(id) ON DELETE CASCADE,
    relation_type VARCHAR(50) NOT NULL,      -- 'prefers' | 'lives_in' | 'owns' | 'habit' | 'related_to'
    weight FLOAT DEFAULT 1.0,                -- Relationship strength (time decay + frequency weighting)
    metadata JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(source_id, target_id, relation_type)
);

-- Indexes
CREATE INDEX idx_memory_edges_source ON memory_edges(source_id);
CREATE INDEX idx_memory_edges_target ON memory_edges(target_id);
CREATE INDEX idx_memory_edges_relation ON memory_edges(relation_type);
CREATE INDEX idx_memory_edges_weight ON memory_edges(weight DESC);
