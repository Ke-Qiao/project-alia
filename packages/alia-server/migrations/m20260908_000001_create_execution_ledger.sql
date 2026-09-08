-- Execution owns these records. A response cancellation cannot erase an effect.
CREATE TABLE execution_effects (
    effect_id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    invocation_key TEXT NOT NULL CHECK (length(invocation_key) BETWEEN 1 AND 128),
    model_call_id TEXT NOT NULL CHECK (length(model_call_id) <= 256),
    source_received_at TIMESTAMPTZ NOT NULL,
    source_event_id UUID,
    source TEXT NOT NULL CHECK (length(source) BETWEEN 1 AND 64),
    conversation_id UUID,
    channel_id TEXT CHECK (length(channel_id) <= 256),
    tool_name TEXT NOT NULL CHECK (length(tool_name) BETWEEN 1 AND 128),
    arguments JSONB NOT NULL CHECK (octet_length(arguments::text) <= 32768),
    tool_digest TEXT NOT NULL CHECK (length(tool_digest) = 64),
    policy_digest TEXT NOT NULL CHECK (length(policy_digest) = 64),
    catalog_digest TEXT NOT NULL CHECK (length(catalog_digest) = 64),
    intent_digest TEXT NOT NULL CHECK (length(intent_digest) = 64),
    requires_confirmation BOOLEAN NOT NULL,
    required_role TEXT CHECK (required_role IN ('user', 'admin')),
    status TEXT NOT NULL CHECK (status IN (
        'proposed', 'approved', 'dispatched', 'succeeded', 'failed', 'unknown', 'cancelled'
    )),
    approved_by UUID REFERENCES users(id),
    approval_kind TEXT CHECK (approval_kind IN ('automatic', 'explicit')),
    approved_at TIMESTAMPTZ,
    approval_expires_at TIMESTAMPTZ,
    claim_id UUID UNIQUE,
    worker_id UUID,
    deadline_at TIMESTAMPTZ,
    completion JSONB CHECK (octet_length(completion::text) <= 131072),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (user_id, request_id, invocation_key),
    CHECK (user_id <> '00000000-0000-0000-0000-000000000000'),
    CHECK (request_id <> '00000000-0000-0000-0000-000000000000'),
    CHECK (status NOT IN ('approved', 'dispatched', 'succeeded', 'unknown')
        OR (approved_by IS NOT NULL AND approval_kind IS NOT NULL
            AND approved_at IS NOT NULL AND approval_expires_at IS NOT NULL)),
    CHECK (status NOT IN ('dispatched', 'succeeded', 'unknown')
        OR (claim_id IS NOT NULL AND worker_id IS NOT NULL AND deadline_at IS NOT NULL)),
    CHECK (status NOT IN ('succeeded', 'failed', 'unknown') OR completion IS NOT NULL)
);

CREATE INDEX execution_effects_owner_history ON execution_effects(user_id, created_at DESC, effect_id DESC);
CREATE INDEX execution_effects_expired_claims ON execution_effects(deadline_at)
    WHERE status = 'dispatched';
CREATE INDEX execution_effects_proposed_owner ON execution_effects(user_id)
    WHERE status = 'proposed';

-- Only approved, never-dispatched effects are enqueued. Claim consumes this
-- intent in the same transaction that commits Dispatched. It is never requeued.
CREATE TABLE execution_outbox (
    effect_id UUID PRIMARY KEY REFERENCES execution_effects(effect_id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE execution_events (
    event_id UUID PRIMARY KEY,
    effect_id UUID NOT NULL REFERENCES execution_effects(effect_id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    actor_id UUID,
    evidence JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX execution_events_history ON execution_events(effect_id, created_at, event_id);
