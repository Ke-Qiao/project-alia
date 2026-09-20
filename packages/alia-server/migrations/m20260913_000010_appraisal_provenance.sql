-- Canonical judgments are separate from their fallible interpretation content.
-- Old records contain no recoverable authority; never invent lineage for them.
DELETE FROM expression_appraisal_events;
CREATE FUNCTION reject_legacy_appraisal_write() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'appraisal writes require canonical provenance' USING ERRCODE='23514'; END;
$$;
CREATE TRIGGER legacy_appraisal_closed BEFORE INSERT OR UPDATE ON expression_appraisal_events
    FOR EACH ROW EXECUTE FUNCTION reject_legacy_appraisal_write();

CREATE TABLE relationship_authority (
    owner_id UUID PRIMARY KEY REFERENCES users(id),
    relationship_epoch BIGINT NOT NULL DEFAULT 0 CHECK (relationship_epoch>=0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
ALTER TABLE chat_messages ADD COLUMN source_relationship_epoch BIGINT CHECK (source_relationship_epoch>=0);
CREATE FUNCTION preserve_source_relationship_stamp() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.source_relationship_epoch IS DISTINCT FROM OLD.source_relationship_epoch THEN
        RAISE EXCEPTION 'original relationship stamp is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER source_relationship_stamp_immutable BEFORE UPDATE ON chat_messages
    FOR EACH ROW EXECUTE FUNCTION preserve_source_relationship_stamp();

CREATE TABLE appraisal_records (
    appraisal_id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    context_id UUID NOT NULL REFERENCES context_leases(context_id),
    occurrence_key TEXT NOT NULL CHECK (length(occurrence_key) BETWEEN 1 AND 160),
    interpreter_id TEXT NOT NULL CHECK (length(interpreter_id) BETWEEN 1 AND 128),
    interpreter_version TEXT NOT NULL CHECK (length(interpreter_version) BETWEEN 1 AND 128),
    policy_version BIGINT NOT NULL CHECK (policy_version>0),
    interpretation_policy_digest TEXT NOT NULL CHECK (interpretation_policy_digest~'^[0-9a-f]{64}$'),
    source_event_id UUID NOT NULL REFERENCES chat_messages(id),
    source_privacy_epoch BIGINT NOT NULL CHECK (source_privacy_epoch>=0),
    source_relationship_epoch BIGINT NOT NULL CHECK (source_relationship_epoch>=0),
    source_received_at TIMESTAMPTZ NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision>0),
    input_digest TEXT NOT NULL CHECK (input_digest~'^[0-9a-f]{64}$'),
    result_digest TEXT NOT NULL CHECK (result_digest~'^[0-9a-f]{64}$'),
    source_fact JSONB NOT NULL CHECK (octet_length(source_fact::text)<=4096),
    events JSONB NOT NULL CHECK (jsonb_typeof(events)='array' AND jsonb_array_length(events)<=64 AND octet_length(events::text)<=65536),
    roots JSONB NOT NULL CHECK (jsonb_typeof(roots)='array' AND jsonb_array_length(roots)<=256 AND octet_length(roots::text)<=65536),
    roots_digest TEXT NOT NULL CHECK (roots_digest~'^[0-9a-f]{64}$'),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','superseded','revoked')),
    effective_at TIMESTAMPTZ NOT NULL,
    valid_until TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(owner_id,request_id,occurrence_key,interpreter_id,interpreter_version,policy_version),
    CHECK(valid_until>effective_at)
);
CREATE INDEX appraisal_owner_candidates ON appraisal_records(owner_id,source_relationship_epoch,effective_at DESC,appraisal_id) WHERE status='active';
CREATE TABLE affect_projections (
    projection_id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id),
    context_id UUID NOT NULL REFERENCES context_leases(context_id),
    policy_version BIGINT NOT NULL CHECK(policy_version>0),
    source_privacy_epoch BIGINT NOT NULL CHECK(source_privacy_epoch>=0),
    relationship_epoch BIGINT NOT NULL CHECK(relationship_epoch>=0),
    reducer_version TEXT NOT NULL CHECK(length(reducer_version) BETWEEN 1 AND 128),
    content JSONB NOT NULL CHECK(octet_length(content::text)<=65536),
    content_digest TEXT NOT NULL CHECK(content_digest~'^[0-9a-f]{64}$'),
    roots JSONB NOT NULL CHECK(jsonb_typeof(roots)='array' AND jsonb_array_length(roots)<=256 AND octet_length(roots::text)<=65536),
    roots_digest TEXT NOT NULL CHECK(roots_digest~'^[0-9a-f]{64}$'),
    as_of TIMESTAMPTZ NOT NULL,
    valid_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX affect_projection_owner ON affect_projections(owner_id,relationship_epoch);
CREATE TRIGGER affect_projection_immutable BEFORE UPDATE ON affect_projections
    FOR EACH ROW EXECUTE FUNCTION reject_context_lease_update();
CREATE TABLE relationship_clear_operations (
    operation_id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id),
    expected_epoch BIGINT NOT NULL,
    resulting_epoch BIGINT NOT NULL,
    removed_records BIGINT NOT NULL,
    removed_memories BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK(resulting_epoch=expected_epoch+1)
);

-- These legacy trace fields duplicated private material without a root set.
DELETE FROM conversation_events WHERE actor IN ('affect','expression');

-- Privacy writes already own the owner privacy lock. Acquire relationship
-- afterwards so every process fences deletion against appraisal publication.
CREATE FUNCTION erase_private_appraisal_derivatives() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.privacy_epoch IS DISTINCT FROM OLD.privacy_epoch THEN
        PERFORM 1 FROM relationship_authority WHERE owner_id=NEW.user_id FOR UPDATE;
        DELETE FROM affect_projections WHERE owner_id=NEW.user_id;
        DELETE FROM appraisal_records WHERE owner_id=NEW.user_id;
        DELETE FROM conversation_events e USING chat_sessions s
            WHERE e.session_id=s.id AND s.creator_id=NEW.user_id::text
              AND e.actor IN ('affect','expression');
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER privacy_erases_appraisal_derivatives AFTER UPDATE OF privacy_epoch ON memory_privacy_state
    FOR EACH ROW EXECUTE FUNCTION erase_private_appraisal_derivatives();

CREATE TABLE memory_appraisal_sources (
    memory_id UUID PRIMARY KEY REFERENCES memory_items(id),
    owner_id UUID NOT NULL REFERENCES users(id),
    roots JSONB NOT NULL CHECK(jsonb_typeof(roots)='array' AND jsonb_array_length(roots)<=256 AND octet_length(roots::text)<=65536),
    roots_digest TEXT NOT NULL CHECK(roots_digest~'^[0-9a-f]{64}$'),
    source JSONB NOT NULL CHECK(octet_length(source::text)<=4096),
    policy_version BIGINT NOT NULL CHECK(policy_version>0),
    valid_until TIMESTAMPTZ
);
CREATE INDEX memory_appraisal_owner ON memory_appraisal_sources(owner_id,memory_id);

-- Ordinary Memory source pins are independent of relationship/Appraisal roots.
-- Do not backfill old rows from current text: their consumed bytes are unknown.
CREATE TABLE memory_message_sources (
    memory_id UUID NOT NULL REFERENCES memory_items(id) ON DELETE CASCADE,
    source_event_id UUID NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL REFERENCES users(id),
    source_privacy_epoch BIGINT NOT NULL CHECK (source_privacy_epoch>=0),
    content_digest TEXT NOT NULL CHECK (content_digest~'^[0-9a-f]{64}$'),
    PRIMARY KEY(memory_id,source_event_id)
);
CREATE INDEX memory_message_source_owner ON memory_message_sources(owner_id,memory_id);
CREATE FUNCTION preserve_memory_message_pin() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'original Memory message pin is immutable' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER memory_message_pin_immutable BEFORE UPDATE ON memory_message_sources
    FOR EACH ROW EXECUTE FUNCTION preserve_memory_message_pin();
CREATE FUNCTION erase_deleted_memory_message_pins() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.status='deleted' THEN
        DELETE FROM memory_message_sources WHERE memory_id=NEW.id AND owner_id=NEW.user_id;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER deleted_memory_erases_message_pins AFTER UPDATE OF status ON memory_items
    FOR EACH ROW EXECUTE FUNCTION erase_deleted_memory_message_pins();
