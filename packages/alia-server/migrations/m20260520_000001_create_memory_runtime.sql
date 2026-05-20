-- Built-in Memory Runtime schema.
--
-- Canonical memory is provider-neutral. Embeddings are stored in profile-
-- specific side tables so future 1024/3072-dimensional profiles do not change
-- memory_items.

CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS embedding_profiles (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL CHECK (dimensions > 0),
    distance TEXT NOT NULL CHECK (distance IN ('cosine', 'l2', 'inner_product')),
    vector_type TEXT NOT NULL DEFAULT 'vector',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO embedding_profiles
    (id, provider, model, dimensions, distance, vector_type)
VALUES
    (
        'openai.text-embedding-3-small.1536.cosine.v1',
        'openai',
        'text-embedding-3-small',
        1536,
        'cosine',
        'vector(1536)'
    )
ON CONFLICT (id) DO NOTHING;

INSERT INTO embedding_profiles
    (id, provider, model, dimensions, distance, vector_type)
VALUES
    (
        'openai.text-embedding-3-large.3072.cosine.v1',
        'openai',
        'text-embedding-3-large',
        3072,
        'cosine',
        'vector(3072)'
    ),
    (
        'generic.embedding.1024.cosine.v1',
        'generic',
        'embedding-1024',
        1024,
        'cosine',
        'vector(1024)'
    )
ON CONFLICT (id) DO NOTHING;

CREATE TABLE IF NOT EXISTS memory_items (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    user_id UUID,
    conversation_id UUID,

    kind TEXT NOT NULL CHECK (
        kind IN (
            'episodic',
            'semantic',
            'preference',
            'social',
            'affective_pattern',
            'system',
            'project',
            'boundary'
        )
    ),
    scope TEXT NOT NULL CHECK (scope IN ('turn', 'session', 'user', 'global', 'device')),

    memory_layer TEXT NOT NULL,
    node_type TEXT NOT NULL,
    content TEXT NOT NULL,
    summary TEXT,

    confidence REAL NOT NULL DEFAULT 0.5 CHECK (confidence >= 0.0 AND confidence <= 1.0),
    salience REAL NOT NULL DEFAULT 0.5 CHECK (salience >= 0.0 AND salience <= 1.0),
    importance REAL NOT NULL DEFAULT 0.5 CHECK (importance >= 0.0 AND importance <= 1.0),

    emotional_valence REAL CHECK (emotional_valence >= -1.0 AND emotional_valence <= 1.0),
    emotional_arousal REAL CHECK (emotional_arousal >= 0.0 AND emotional_arousal <= 1.0),

    affect_tags TEXT[] NOT NULL DEFAULT '{}',
    topic_tags TEXT[] NOT NULL DEFAULT '{}',
    source_event_ids UUID[] NOT NULL DEFAULT '{}',
    evidence JSONB NOT NULL DEFAULT '{}',
    metadata JSONB NOT NULL DEFAULT '{}',

    status TEXT NOT NULL DEFAULT 'active' CHECK (
        status IN ('active', 'superseded', 'rejected', 'expired')
    ),
    superseded_by UUID REFERENCES memory_items(id),

    visibility TEXT NOT NULL DEFAULT 'internal' CHECK (
        visibility IN ('user_visible', 'internal', 'private_runtime')
    ),
    sensitive BOOLEAN NOT NULL DEFAULT FALSE,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_accessed_at TIMESTAMPTZ,
    access_count INTEGER NOT NULL DEFAULT 0,
    expires_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS memory_items_user_kind_status_idx
    ON memory_items (tenant_id, user_id, kind, status);

CREATE INDEX IF NOT EXISTS memory_items_scope_status_idx
    ON memory_items (tenant_id, scope, status);

CREATE INDEX IF NOT EXISTS memory_items_topic_tags_gin_idx
    ON memory_items USING GIN (topic_tags);

CREATE INDEX IF NOT EXISTS memory_items_affect_tags_gin_idx
    ON memory_items USING GIN (affect_tags);

CREATE INDEX IF NOT EXISTS memory_items_metadata_gin_idx
    ON memory_items USING GIN (metadata);

CREATE TABLE IF NOT EXISTS memory_embeddings_1536 (
    memory_id UUID NOT NULL REFERENCES memory_items(id) ON DELETE CASCADE,
    profile_id TEXT NOT NULL REFERENCES embedding_profiles(id),
    embedding vector(1536) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (memory_id, profile_id)
);

CREATE INDEX IF NOT EXISTS memory_embeddings_1536_hnsw_cosine_idx
    ON memory_embeddings_1536
    USING hnsw (embedding vector_cosine_ops);

CREATE INDEX IF NOT EXISTS memory_embeddings_1536_profile_idx
    ON memory_embeddings_1536 (profile_id);

CREATE TABLE IF NOT EXISTS memory_nodes (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    user_id UUID,
    node_type TEXT NOT NULL CHECK (
        node_type IN (
            'user',
            'assistant',
            'topic',
            'entity',
            'preference',
            'event',
            'project',
            'routine',
            'boundary',
            'affect_pattern',
            'device',
            'memory'
        )
    ),
    node_key TEXT NOT NULL,
    label TEXT NOT NULL,
    embedding vector(1536),
    properties JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, user_id, node_type, node_key)
);

CREATE INDEX IF NOT EXISTS memory_nodes_user_type_idx
    ON memory_nodes (tenant_id, user_id, node_type);

CREATE INDEX IF NOT EXISTS memory_nodes_properties_gin_idx
    ON memory_nodes USING GIN (properties);

CREATE TABLE IF NOT EXISTS memory_edges (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    user_id UUID,
    src_node_id UUID NOT NULL REFERENCES memory_nodes(id) ON DELETE CASCADE,
    dst_node_id UUID NOT NULL REFERENCES memory_nodes(id) ON DELETE CASCADE,
    relation TEXT NOT NULL CHECK (
        relation IN (
            'PREFERS',
            'DISLIKES',
            'MENTIONED',
            'RELATED_TO',
            'WORKING_ON',
            'TRIGGERED_BY',
            'SOOTHED_BY',
            'CONTRADICTS',
            'SUPERSEDES',
            'EVIDENCED_BY',
            'BOUNDARY_FOR',
            'USES_DEVICE'
        )
    ),
    weight REAL NOT NULL DEFAULT 1.0 CHECK (weight >= 0.0),
    confidence REAL NOT NULL DEFAULT 0.5 CHECK (confidence >= 0.0 AND confidence <= 1.0),
    evidence_memory_id UUID REFERENCES memory_items(id) ON DELETE SET NULL,
    properties JSONB NOT NULL DEFAULT '{}',
    valid_from TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    valid_to TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, user_id, src_node_id, dst_node_id, relation, evidence_memory_id)
);

CREATE INDEX IF NOT EXISTS memory_edges_src_idx
    ON memory_edges (tenant_id, user_id, src_node_id, relation);

CREATE INDEX IF NOT EXISTS memory_edges_dst_idx
    ON memory_edges (tenant_id, user_id, dst_node_id, relation);

CREATE INDEX IF NOT EXISTS memory_edges_relation_idx
    ON memory_edges (tenant_id, user_id, relation);
