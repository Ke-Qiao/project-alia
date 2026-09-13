-- Software body submission is its own evidence type, not audio or position.
CREATE TABLE body_deliveries (
    delivery_id UUID PRIMARY KEY,
    response_id UUID NOT NULL REFERENCES delivery_responses(response_id),
    output_key TEXT NOT NULL CHECK (length(output_key) BETWEEN 1 AND 256),
    channel_id TEXT NOT NULL,
    source_command_id TEXT NOT NULL,
    dispatch_id UUID NOT NULL UNIQUE,
    identity JSONB NOT NULL UNIQUE,
    command JSONB NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('sending','observing','submitted','cancelled','failed','rejected','unknown')),
    progress JSONB,
    unknown_reason TEXT,
    lease_expires_at TIMESTAMPTZ NOT NULL,
    observation_deadline_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(response_id, output_key),
    UNIQUE(channel_id, source_command_id),
    CHECK (status NOT IN ('submitted','cancelled','failed','rejected') OR progress IS NOT NULL)
);
CREATE INDEX body_deliveries_expiry ON body_deliveries(lease_expires_at)
    WHERE status IN ('sending','observing');
CREATE TABLE body_delivery_late_checkpoints (
    delivery_id UUID NOT NULL REFERENCES body_deliveries(delivery_id),
    receipt_sequence BIGINT NOT NULL CHECK (receipt_sequence > 0),
    progress JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(delivery_id, receipt_sequence)
);
