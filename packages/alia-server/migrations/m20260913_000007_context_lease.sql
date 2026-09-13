CREATE TABLE context_policy (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    version BIGINT NOT NULL CHECK (version > 0),
    policy JSONB NOT NULL CHECK (jsonb_typeof(policy) = 'object' AND octet_length(policy::text) <= 32768),
    policy_digest TEXT NOT NULL CHECK (policy_digest ~ '^[0-9a-f]{64}$'),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE context_leases (
    context_id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    context_key TEXT NOT NULL CHECK (length(context_key) BETWEEN 1 AND 128),
    purpose TEXT NOT NULL CHECK (purpose IN ('generation','source_notice','memory_capture','memory_maintenance')),
    channel_id TEXT NOT NULL,
    conversation_id UUID,
    source_event_id UUID,
    source_received_at TIMESTAMPTZ NOT NULL,
    source_privacy_epoch BIGINT NOT NULL CHECK (source_privacy_epoch >= 0),
    policy_version BIGINT NOT NULL CHECK (policy_version > 0),
    dependencies JSONB NOT NULL CHECK (jsonb_typeof(dependencies) = 'array' AND jsonb_array_length(dependencies) <= 256 AND octet_length(dependencies::text) <= 65536),
    dependencies_digest TEXT NOT NULL CHECK (dependencies_digest ~ '^[0-9a-f]{64}$'),
    materialized_digest TEXT NOT NULL CHECK (materialized_digest ~ '^[0-9a-f]{64}$'),
    valid_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (owner_id, request_id, context_key),
    CHECK (purpose <> 'generation' OR (conversation_id IS NOT NULL AND source_event_id IS NOT NULL)),
    CHECK (valid_until IS NULL OR valid_until > created_at)
);
CREATE INDEX context_leases_owner_epoch ON context_leases(owner_id,source_privacy_epoch);
CREATE FUNCTION reject_context_lease_update() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'context leases are immutable' USING ERRCODE = '23514'; END;
$$;
CREATE TRIGGER context_leases_immutable BEFORE UPDATE ON context_leases
    FOR EACH ROW EXECUTE FUNCTION reject_context_lease_update();
ALTER TABLE delivery_responses ADD COLUMN context_id UUID REFERENCES context_leases(context_id);
CREATE INDEX delivery_responses_context ON delivery_responses(context_id);
ALTER TABLE execution_effects ADD COLUMN context_id UUID REFERENCES context_leases(context_id);
