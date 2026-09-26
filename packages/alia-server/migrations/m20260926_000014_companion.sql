-- Shadow evidence has no output or execution authority. Erasure retains only
-- opaque intent/dedup identities; it never leaves a private replay archive.
CREATE TABLE companion_policies (
    owner_id UUID PRIMARY KEY REFERENCES users(id),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision>0),
    policy JSONB NOT NULL CHECK (octet_length(policy::text)<=4096),
    policy_digest TEXT NOT NULL CHECK (policy_digest~'^[0-9a-f]{64}$'),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE companion_operations (
    operation_id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id),
    request_digest TEXT NOT NULL CHECK (request_digest~'^[0-9a-f]{64}$'),
    policy_revision BIGINT NOT NULL CHECK (policy_revision>0),
    context_policy_version BIGINT NOT NULL CHECK (context_policy_version>0),
    privacy_epoch BIGINT NOT NULL CHECK (privacy_epoch>=0),
    relationship_epoch BIGINT NOT NULL CHECK (relationship_epoch>=0),
    as_of TIMESTAMPTZ NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('completed','erased')),
    private_snapshot JSONB CHECK (octet_length(private_snapshot::text)<=131072),
    snapshot_digest TEXT CHECK (snapshot_digest~'^[0-9a-f]{64}$'),
    private_until TIMESTAMPTZ,
    CHECK ((state='completed')=(private_snapshot IS NOT NULL AND snapshot_digest IS NOT NULL AND private_until IS NOT NULL)),
    CHECK (state<>'erased' OR (private_snapshot IS NULL AND snapshot_digest IS NULL AND private_until IS NULL))
);
CREATE INDEX companion_operations_owner ON companion_operations(owner_id,operation_id);
CREATE INDEX companion_private_expiry ON companion_operations(private_until,operation_id) WHERE state='completed';
CREATE TABLE companion_candidates (
    candidate_id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL REFERENCES companion_operations(operation_id),
    dedup_key TEXT NOT NULL CHECK (length(dedup_key) BETWEEN 1 AND 160),
    private_candidate JSONB CHECK (octet_length(private_candidate::text)<=32768),
    UNIQUE(owner_id,dedup_key)
);
-- These opaque pins deliberately survive erasure and process restarts.
CREATE TABLE companion_processed_sources (
    owner_id UUID NOT NULL REFERENCES users(id),
    appraisal_id UUID NOT NULL,
    source_event_id UUID NOT NULL,
    operation_id UUID NOT NULL REFERENCES companion_operations(operation_id),
    PRIMARY KEY(owner_id,appraisal_id)
);
CREATE INDEX companion_processed_source ON companion_processed_sources(owner_id,source_event_id);
-- Canonical rows with irreversibly stale parents must not monopolize discovery.
-- No content, root set, refreshed epoch, or permission is retained here.
CREATE TABLE companion_rejected_sources (
    owner_id UUID NOT NULL REFERENCES users(id),
    appraisal_id UUID NOT NULL,
    PRIMARY KEY(owner_id,appraisal_id)
);
CREATE TABLE companion_sources (
    operation_id UUID NOT NULL REFERENCES companion_operations(operation_id),
    kind TEXT NOT NULL CHECK (kind IN ('appraisal','affect','message','memory','graph','cache','execution','response')),
    source_id UUID NOT NULL,
    PRIMARY KEY(operation_id,kind,source_id)
);
CREATE INDEX companion_source_erasure ON companion_sources(kind,source_id);

CREATE FUNCTION preserve_companion_operation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (to_jsonb(NEW)-'state'-'private_snapshot'-'snapshot_digest'-'private_until') IS DISTINCT FROM
       (to_jsonb(OLD)-'state'-'private_snapshot'-'snapshot_digest'-'private_until')
       OR (NEW IS DISTINCT FROM OLD AND NOT (NEW.state='erased' AND NEW.private_snapshot IS NULL AND NEW.snapshot_digest IS NULL AND NEW.private_until IS NULL)) THEN
        RAISE EXCEPTION 'Companion operation is immutable except erasure' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER companion_operation_immutable BEFORE UPDATE ON companion_operations
    FOR EACH ROW EXECUTE FUNCTION preserve_companion_operation();
CREATE FUNCTION preserve_companion_candidate() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (to_jsonb(NEW)-'private_candidate') IS DISTINCT FROM (to_jsonb(OLD)-'private_candidate')
       OR (NEW.private_candidate IS DISTINCT FROM OLD.private_candidate AND NEW.private_candidate IS NOT NULL) THEN
        RAISE EXCEPTION 'Companion candidate is immutable except erasure' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER companion_candidate_immutable BEFORE UPDATE ON companion_candidates
    FOR EACH ROW EXECUTE FUNCTION preserve_companion_candidate();

CREATE FUNCTION erase_companion_operations(p_operations UUID[]) RETURNS void
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
BEGIN
    UPDATE companion_operations SET state='erased',private_snapshot=NULL,snapshot_digest=NULL,private_until=NULL
        WHERE operation_id=ANY(p_operations) AND state<>'erased';
    UPDATE companion_candidates SET private_candidate=NULL
        WHERE operation_id=ANY(p_operations) AND private_candidate IS NOT NULL;
    DELETE FROM companion_sources WHERE operation_id=ANY(p_operations);
END;
$$;
CREATE FUNCTION erase_companion_owner(p_owner UUID) RETURNS void
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
BEGIN
    PERFORM erase_companion_operations(ARRAY(SELECT operation_id FROM companion_operations WHERE owner_id=p_owner AND state<>'erased'));
END;
$$;
CREATE FUNCTION erase_companion_authority() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
BEGIN
    IF TG_TABLE_NAME='memory_privacy_state' THEN
        IF NEW.privacy_epoch IS DISTINCT FROM OLD.privacy_epoch THEN PERFORM erase_companion_owner(NEW.user_id); END IF;
    ELSIF TG_TABLE_NAME='relationship_authority' THEN
        IF NEW.relationship_epoch IS DISTINCT FROM OLD.relationship_epoch THEN PERFORM erase_companion_owner(NEW.owner_id); END IF;
    ELSIF TG_TABLE_NAME='companion_policies' THEN
        IF NEW.revision IS DISTINCT FROM OLD.revision THEN PERFORM erase_companion_owner(NEW.owner_id); END IF;
    ELSE
        IF NEW.version IS DISTINCT FROM OLD.version THEN
            PERFORM erase_companion_operations(ARRAY(SELECT operation_id FROM companion_operations WHERE state<>'erased'));
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER privacy_erases_companion AFTER UPDATE OF privacy_epoch ON memory_privacy_state
    FOR EACH ROW EXECUTE FUNCTION erase_companion_authority();
CREATE TRIGGER relationship_erases_companion AFTER UPDATE OF relationship_epoch ON relationship_authority
    FOR EACH ROW EXECUTE FUNCTION erase_companion_authority();
CREATE TRIGGER companion_policy_erases_prior_inputs AFTER UPDATE OF revision ON companion_policies
    FOR EACH ROW EXECUTE FUNCTION erase_companion_authority();
CREATE TRIGGER context_policy_erases_companion AFTER UPDATE OF version ON context_policy
    FOR EACH ROW EXECUTE FUNCTION erase_companion_authority();

CREATE FUNCTION erase_companion_source() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
DECLARE
    field TEXT;
    changed BOOLEAN := FALSE;
BEGIN
    IF TG_OP='UPDATE' THEN
        IF NEW IS NOT DISTINCT FROM OLD THEN RETURN NEW; END IF;
        IF TG_NARGS>2 THEN
            FOREACH field IN ARRAY string_to_array(TG_ARGV[2],',') LOOP
                IF (to_jsonb(NEW)->field) IS DISTINCT FROM (to_jsonb(OLD)->field) THEN changed:=TRUE; EXIT; END IF;
            END LOOP;
            IF NOT changed THEN RETURN NEW; END IF;
        END IF;
    END IF;
    PERFORM erase_companion_operations(ARRAY(SELECT operation_id FROM companion_sources
        WHERE kind=TG_ARGV[0] AND source_id=(to_jsonb(OLD)->>TG_ARGV[1])::uuid));
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;
CREATE TRIGGER appraisal_erases_companion AFTER UPDATE OR DELETE ON appraisal_records
    FOR EACH ROW EXECUTE FUNCTION erase_companion_source('appraisal','appraisal_id');
CREATE TRIGGER affect_erases_companion AFTER DELETE ON affect_projections
    FOR EACH ROW EXECUTE FUNCTION erase_companion_source('affect','projection_id');
CREATE TRIGGER message_erases_companion AFTER UPDATE OF content,user_id,session_id,role OR DELETE ON chat_messages
    FOR EACH ROW EXECUTE FUNCTION erase_companion_source('message','id');
CREATE TRIGGER memory_erases_companion AFTER UPDATE OF revision,status,visibility,sensitive,expires_at OR DELETE ON memory_items
    FOR EACH ROW EXECUTE FUNCTION erase_companion_source('memory','id');
CREATE TRIGGER graph_erases_companion AFTER UPDATE OR DELETE ON memory_edges
    FOR EACH ROW EXECUTE FUNCTION erase_companion_source('graph','id');
CREATE TRIGGER cache_erases_companion AFTER UPDATE OR DELETE ON llm_cache
    FOR EACH ROW EXECUTE FUNCTION erase_companion_source('cache','id','id,user_id,context_id,revision,scope,query_hash,query_text,response_text,content_digest,status,expires_at');
CREATE TRIGGER execution_erases_companion AFTER UPDATE OR DELETE ON execution_effects
    FOR EACH ROW EXECUTE FUNCTION erase_companion_source('execution','effect_id','effect_id,user_id,request_id,model_call_id,tool_name,status,intent_digest,completion,context_id,source_event_id,source_privacy_epoch,source_received_at');
CREATE TRIGGER response_erases_companion AFTER UPDATE OR DELETE ON delivery_responses
    FOR EACH ROW EXECUTE FUNCTION erase_companion_source('response','response_id','response_id,user_id,request_id,context_id,conversation_id,assistant_message_id,status,final_content,source_event_id,source_privacy_epoch,source_received_at,channel_id');

CREATE FUNCTION erase_companion_graph_node() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
BEGIN
    IF TG_OP='UPDATE' AND (NEW.id,NEW.user_id,NEW.tenant_id,NEW.label,NEW.node_type,NEW.properties)
        IS NOT DISTINCT FROM (OLD.id,OLD.user_id,OLD.tenant_id,OLD.label,OLD.node_type,OLD.properties) THEN RETURN NEW; END IF;
    PERFORM erase_companion_operations(ARRAY(SELECT s.operation_id FROM companion_sources s JOIN memory_edges e ON e.id=s.source_id
        WHERE s.kind='graph' AND (e.src_node_id=OLD.id OR e.dst_node_id=OLD.id)));
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;
-- BEFORE DELETE still sees edges that ON DELETE CASCADE will remove.
CREATE TRIGGER graph_node_erases_companion BEFORE UPDATE OR DELETE ON memory_nodes
    FOR EACH ROW EXECUTE FUNCTION erase_companion_graph_node();
