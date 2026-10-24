-- Operations outlive responses, process restarts and their initiating tool receipt.
CREATE TABLE embodiment_operations (
    operation_id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id),
    start_effect_id UUID NOT NULL UNIQUE REFERENCES execution_effects(effect_id),
    cancel_effect_id UUID REFERENCES execution_effects(effect_id),
    identity JSONB NOT NULL,
    goal JSONB NOT NULL CHECK (octet_length(goal::text) <= 32768),
    resources JSONB NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('submitting','accepted','running','succeeded','failed','cancelling','cancelled','unknown')),
    last_confirmed_status TEXT CHECK (last_confirmed_status IN ('accepted','running','succeeded','failed','cancelling','cancelled')),
    last_sequence BIGINT NOT NULL DEFAULT 0 CHECK (last_sequence >= 0),
    last_feedback_at_ms BIGINT,
    feedback JSONB NOT NULL DEFAULT '{}' CHECK (octet_length(feedback::text) <= 16384),
    stopped JSONB CHECK (octet_length(stopped::text) <= 16384),
    uncertainty TEXT CHECK (length(uncertainty) <= 1024),
    created_at_ms BIGINT NOT NULL DEFAULT (extract(epoch FROM clock_timestamp()) * 1000)::bigint,
    updated_at_ms BIGINT NOT NULL DEFAULT (extract(epoch FROM clock_timestamp()) * 1000)::bigint,
    next_observe_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX embodiment_operations_owner ON embodiment_operations(owner_id, created_at_ms DESC, operation_id);
CREATE INDEX embodiment_operations_reconcile ON embodiment_operations(next_observe_at)
    WHERE status NOT IN ('succeeded','failed','cancelled') OR stopped IS NULL;

-- Never leased or timed out: Unknown continues to own these resources.
CREATE TABLE embodiment_operation_resources (
    device_id UUID NOT NULL,
    resource TEXT NOT NULL CHECK (length(resource) BETWEEN 1 AND 128),
    operation_id UUID NOT NULL REFERENCES embodiment_operations(operation_id),
    PRIMARY KEY (device_id, resource)
);
CREATE INDEX embodiment_operation_resource_owner ON embodiment_operation_resources(operation_id);

CREATE TABLE embodiment_operation_cancellations (
    effect_id UUID PRIMARY KEY REFERENCES execution_effects(effect_id),
    operation_id UUID NOT NULL REFERENCES embodiment_operations(operation_id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE embodiment_operation_events (
    event_id BIGSERIAL PRIMARY KEY,
    operation_id UUID NOT NULL REFERENCES embodiment_operations(operation_id),
    status TEXT NOT NULL,
    evidence JSONB NOT NULL CHECK (octet_length(evidence::text) <= 32768),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX embodiment_operation_event_owner ON embodiment_operation_events(operation_id, event_id DESC);
