-- Delivery is a record of an attempted presentation, never a replay queue.
CREATE TABLE delivery_responses (
    response_id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    output_key TEXT NOT NULL CHECK (length(output_key) BETWEEN 1 AND 256),
    conversation_id UUID,
    assistant_message_id UUID UNIQUE,
    source_event_id UUID,
    source_received_at TIMESTAMPTZ NOT NULL,
    source_privacy_epoch BIGINT CHECK (source_privacy_epoch >= 0),
    channel_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    content JSONB NOT NULL,
    final_content JSONB,
    intent_digest TEXT NOT NULL CHECK (length(intent_digest) = 64),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','committed','interrupted','discarded')),
    cancel_requested BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(user_id, request_id, output_key),
    CHECK ((status = 'committed') = (final_content IS NOT NULL AND assistant_message_id IS NOT NULL))
);

CREATE TABLE deliveries (
    delivery_id UUID PRIMARY KEY,
    response_id UUID NOT NULL REFERENCES delivery_responses(response_id),
    output_key TEXT NOT NULL CHECK (length(output_key) BETWEEN 1 AND 256),
    content JSONB NOT NULL,
    playback_id UUID,
    identity JSONB,
    intent_digest TEXT NOT NULL CHECK (length(intent_digest) = 64),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','sending','observing','submitted','completed','cancelled','failed','unknown')),
    owner_id UUID NOT NULL,
    dispatch_id UUID,
    lease_expires_at TIMESTAMPTZ NOT NULL,
    observation_deadline_at TIMESTAMPTZ,
    cancel_requested BOOLEAN NOT NULL DEFAULT FALSE,
    source_sample_rate INTEGER CHECK (source_sample_rate > 0),
    source_channels INTEGER CHECK (source_channels > 0),
    offered_frames BIGINT NOT NULL DEFAULT 0 CHECK (offered_frames >= 0),
    accepted_frames BIGINT NOT NULL DEFAULT 0 CHECK (accepted_frames BETWEEN 0 AND offered_frames),
    submitted_frames BIGINT CHECK (submitted_frames BETWEEN 0 AND accepted_frames),
    presented_frames BIGINT CHECK (presented_frames BETWEEN 0 AND submitted_frames),
    end_accepted BOOLEAN NOT NULL DEFAULT FALSE,
    receipt_sequence BIGINT NOT NULL DEFAULT 0 CHECK (receipt_sequence >= 0),
    progress JSONB,
    outcome JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(response_id, output_key),
    UNIQUE(playback_id),
    CHECK ((identity IS NULL) = (playback_id IS NULL)),
    CHECK (presented_frames IS NULL OR submitted_frames IS NOT NULL),
    CHECK ((source_sample_rate IS NULL) = (source_channels IS NULL)),
    CHECK (status <> 'submitted' OR (end_accepted AND submitted_frames IS NOT NULL AND submitted_frames = offered_frames)),
    CHECK (status <> 'completed' OR (end_accepted AND presented_frames IS NOT NULL AND presented_frames = offered_frames)),
    CHECK (status = 'pending' OR dispatch_id IS NOT NULL OR status IN ('cancelled','failed','unknown'))
);
CREATE INDEX deliveries_expired_owners ON deliveries(lease_expires_at)
    WHERE status IN ('pending','sending','observing');
CREATE INDEX delivery_responses_owner ON delivery_responses(user_id, created_at DESC);

CREATE TABLE delivery_events (
    event_id BIGSERIAL PRIMARY KEY,
    response_id UUID NOT NULL REFERENCES delivery_responses(response_id),
    delivery_id UUID REFERENCES deliveries(delivery_id),
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX delivery_events_response ON delivery_events(response_id,event_id);
