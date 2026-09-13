-- Old cache rows have no canonical generation provenance. They cannot be
-- upgraded by assigning today's context or retained as a shared fallback.
DELETE FROM llm_cache;
DROP INDEX idx_llm_cache_query_hash;
DROP INDEX idx_llm_cache_embedding;
ALTER TABLE llm_cache
    DROP COLUMN embedding,
    ALTER COLUMN user_id SET NOT NULL,
    ALTER COLUMN metadata SET NOT NULL,
    ALTER COLUMN hit_count SET NOT NULL,
    ALTER COLUMN created_at SET NOT NULL,
    ADD COLUMN revision BIGINT NOT NULL CHECK (revision > 0),
    ADD COLUMN context_id UUID NOT NULL REFERENCES context_leases(context_id),
    ADD COLUMN origin_response_id UUID NOT NULL REFERENCES delivery_responses(response_id),
    ADD COLUMN status TEXT NOT NULL DEFAULT 'pending_insights_review'
        CHECK (status IN ('pending_insights_review','active')),
    ADD COLUMN content_digest TEXT NOT NULL CHECK (content_digest ~ '^[0-9a-f]{64}$'),
    ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    ADD CONSTRAINT llm_cache_owner_fk FOREIGN KEY(user_id) REFERENCES users(id),
    ADD CONSTRAINT llm_cache_scope_bound CHECK (length(scope) BETWEEN 1 AND 100),
    ADD CONSTRAINT llm_cache_query_digest CHECK (query_hash ~ '^[0-9a-f]{64}$'),
    ADD CONSTRAINT llm_cache_query_bound CHECK (octet_length(query_text) BETWEEN 1 AND 262144),
    ADD CONSTRAINT llm_cache_response_bound CHECK (octet_length(response_text) BETWEEN 1 AND 262144),
    ADD CONSTRAINT llm_cache_metadata_bound CHECK (jsonb_typeof(metadata) = 'object' AND octet_length(metadata::text) <= 32768);
CREATE UNIQUE INDEX llm_cache_private_key ON llm_cache(user_id,scope,query_hash);
CREATE INDEX llm_cache_context ON llm_cache(context_id);
CREATE INDEX llm_cache_review ON llm_cache(user_id,status,created_at,id);

-- One Insights phase can use both memory and cache models. Persist the complete
-- labels so a successful phase cannot fail its audit at the old 50-character cap.
ALTER TABLE insight_logs ALTER COLUMN model_used TYPE TEXT;

-- Legacy Memory profiles remain usable. Profiled Cache vectors require the
-- exact immutable descriptor emitted with the corresponding network request.
ALTER TABLE embedding_profiles ADD COLUMN request_descriptor JSONB;
CREATE FUNCTION protect_embedding_profile_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(OLD.id,OLD.provider,OLD.model,OLD.dimensions,OLD.distance,OLD.vector_type,OLD.request_descriptor)
       IS DISTINCT FROM
       ROW(NEW.id,NEW.provider,NEW.model,NEW.dimensions,NEW.distance,NEW.vector_type,NEW.request_descriptor) THEN
        RAISE EXCEPTION 'embedding profile identity is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER immutable_embedding_profile BEFORE UPDATE ON embedding_profiles
    FOR EACH ROW EXECUTE FUNCTION protect_embedding_profile_identity();

CREATE TABLE llm_cache_embeddings (
    cache_id UUID NOT NULL REFERENCES llm_cache(id) ON DELETE CASCADE,
    cache_revision BIGINT NOT NULL CHECK(cache_revision > 0),
    context_id UUID NOT NULL REFERENCES context_leases(context_id),
    query_digest TEXT NOT NULL CHECK(query_digest ~ '^[0-9a-f]{64}$'),
    profile_id TEXT NOT NULL REFERENCES embedding_profiles(id),
    dimensions INTEGER NOT NULL CHECK(dimensions BETWEEN 1 AND 16000),
    embedding vector NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(cache_id,profile_id),
    CHECK(vector_dims(embedding)=dimensions)
);
CREATE INDEX llm_cache_embeddings_profile ON llm_cache_embeddings(profile_id,dimensions);
