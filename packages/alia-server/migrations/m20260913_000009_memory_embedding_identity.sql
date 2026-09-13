-- An old alias and a fixed vector width do not establish which model request
-- produced a vector. Discard those derived indexes and rebuild from canonical
-- Memory revisions; never manufacture request provenance for old values.
DROP TABLE memory_embeddings_1536;

CREATE TABLE memory_embeddings (
    memory_id UUID NOT NULL REFERENCES memory_items(id) ON DELETE CASCADE,
    profile_id TEXT NOT NULL REFERENCES embedding_profiles(id),
    source_revision BIGINT NOT NULL CHECK (source_revision > 0),
    input_digest TEXT NOT NULL CHECK (input_digest ~ '^[0-9a-f]{64}$'),
    policy_version BIGINT NOT NULL CHECK (policy_version > 0),
    privacy_epoch BIGINT NOT NULL CHECK (privacy_epoch >= 0),
    dimensions INTEGER NOT NULL CHECK (dimensions BETWEEN 1 AND 16000),
    embedding vector NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (memory_id, profile_id),
    CHECK (vector_dims(embedding) = dimensions)
);
CREATE INDEX memory_embeddings_profile ON memory_embeddings(profile_id, dimensions);
