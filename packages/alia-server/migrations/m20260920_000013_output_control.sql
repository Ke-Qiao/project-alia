-- The original prepared Response owns a bounded, non-transferable control lifetime.
-- These tables are evidence, never an outbox or permission to replay media.
CREATE TABLE output_control_scopes (
    response_id UUID PRIMARY KEY REFERENCES delivery_responses(response_id),
    owner_id UUID NOT NULL REFERENCES users(id),
    device_boot_id UUID NOT NULL,
    session_id UUID NOT NULL,
    generation_epoch NUMERIC(20,0) NOT NULL CHECK(generation_epoch >= 0),
    registration JSONB NOT NULL,
    registration_digest TEXT NOT NULL CHECK(length(registration_digest)=64),
    deadline_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','retired')),
    retirement_reason TEXT,
    last_command_sequence BIGINT NOT NULL DEFAULT 0 CHECK(last_command_sequence BETWEEN 0 AND 32),
    latest_receipt JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(device_boot_id,session_id,generation_epoch),
    CHECK((status='retired')=(retirement_reason IS NOT NULL))
);
CREATE INDEX output_control_scope_recovery ON output_control_scopes(deadline_at)
    WHERE status='active';

CREATE TABLE output_control_commands (
    command_id UUID PRIMARY KEY,
    response_id UUID NOT NULL REFERENCES output_control_scopes(response_id),
    sequence BIGINT NOT NULL CHECK(sequence BETWEEN 1 AND 32),
    hold_id UUID NOT NULL,
    command JSONB NOT NULL,
    intent_digest TEXT NOT NULL CHECK(length(intent_digest)=64),
    deadline_at TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL DEFAULT 'requested' CHECK(status IN ('requested','unknown','receipted')),
    unknown_reason TEXT,
    first_receipt JSONB,
    latest_receipt JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(response_id,sequence),
    CHECK((status='receipted')=(first_receipt IS NOT NULL)),
    CHECK((first_receipt IS NULL)=(latest_receipt IS NULL))
);
-- At most 32 commands, first/latest receipts each, plus one scope snapshot.
-- Repeated Observe does not append rows or delivery_events.
