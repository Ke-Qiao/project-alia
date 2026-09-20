ALTER TABLE context_leases DROP CONSTRAINT context_leases_purpose_check;
ALTER TABLE context_leases ADD CONSTRAINT context_leases_purpose_check
    CHECK (purpose IN ('generation','source_notice','memory_capture','memory_maintenance','appraisal_rebuild'));

-- One explicit page is an immutable intent. It contains identifiers and
-- receipts, never another copy of the user's raw input or interpreter draft.
CREATE TABLE appraisal_rebuild_operations (
    operation_id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id),
    policy_version BIGINT NOT NULL CHECK (policy_version>0),
    privacy_epoch BIGINT NOT NULL CHECK (privacy_epoch>=0),
    relationship_epoch BIGINT NOT NULL CHECK (relationship_epoch>=0),
    after_source UUID,
    page_limit SMALLINT NOT NULL CHECK (page_limit BETWEEN 1 AND 32),
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX appraisal_rebuild_owner ON appraisal_rebuild_operations(owner_id,operation_id);
CREATE TABLE appraisal_rebuild_items (
    operation_id UUID NOT NULL REFERENCES appraisal_rebuild_operations(operation_id),
    source_event_id UUID NOT NULL,
    context_id UUID REFERENCES context_leases(context_id),
    valid_until TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending','claimed','committed','skipped')),
    claim_token UUID,
    claim_until TIMESTAMPTZ,
    receipt JSONB CHECK (receipt IS NULL OR octet_length(receipt::text)<=4096),
    PRIMARY KEY(operation_id,source_event_id),
    CHECK ((status='claimed')=(claim_token IS NOT NULL AND claim_until IS NOT NULL)),
    CHECK ((status='committed')=(receipt IS NOT NULL)),
    CHECK (status='skipped' OR context_id IS NOT NULL)
);
CREATE FUNCTION preserve_appraisal_rebuild_intent() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.operation_id,NEW.owner_id,NEW.policy_version,NEW.privacy_epoch,
        NEW.relationship_epoch,NEW.after_source,NEW.page_limit,NEW.created_at)
        IS DISTINCT FROM
       (OLD.operation_id,OLD.owner_id,OLD.policy_version,OLD.privacy_epoch,
        OLD.relationship_epoch,OLD.after_source,OLD.page_limit,OLD.created_at)
       OR (OLD.revoked AND NOT NEW.revoked) THEN
        RAISE EXCEPTION 'appraisal rebuild intent is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER appraisal_rebuild_intent_immutable BEFORE UPDATE ON appraisal_rebuild_operations
    FOR EACH ROW EXECUTE FUNCTION preserve_appraisal_rebuild_intent();
CREATE FUNCTION revoke_appraisal_rebuilds() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='memory_privacy_state' THEN
        IF NEW.privacy_epoch IS DISTINCT FROM OLD.privacy_epoch THEN
            UPDATE appraisal_rebuild_operations SET revoked=TRUE WHERE owner_id=NEW.user_id;
        END IF;
    ELSIF NEW.relationship_epoch IS DISTINCT FROM OLD.relationship_epoch THEN
        UPDATE appraisal_rebuild_operations SET revoked=TRUE WHERE owner_id=NEW.owner_id;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER privacy_revokes_appraisal_rebuilds AFTER UPDATE OF privacy_epoch ON memory_privacy_state
    FOR EACH ROW EXECUTE FUNCTION revoke_appraisal_rebuilds();
CREATE TRIGGER relationship_revokes_appraisal_rebuilds AFTER UPDATE OF relationship_epoch ON relationship_authority
    FOR EACH ROW EXECUTE FUNCTION revoke_appraisal_rebuilds();
