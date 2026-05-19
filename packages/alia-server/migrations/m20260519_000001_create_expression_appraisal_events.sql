-- Persist expression/appraisal events so affect and relationship state can
-- recover across server restarts while runtime reducers stay fast in memory.

CREATE TABLE IF NOT EXISTS expression_appraisal_events (
    id UUID PRIMARY KEY,
    channel_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    route TEXT,
    kind TEXT NOT NULL,
    source TEXT NOT NULL,
    target TEXT NOT NULL,
    scope TEXT NOT NULL,
    confidence REAL NOT NULL,
    ttl_sec BIGINT NOT NULL,
    decay JSONB NOT NULL DEFAULT '{}',
    deltas JSONB NOT NULL DEFAULT '{}',
    evidence JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_expression_appraisal_events_channel_user
    ON expression_appraisal_events(channel_id, user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_expression_appraisal_events_user_scope
    ON expression_appraisal_events(user_id, scope, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_expression_appraisal_events_created_at
    ON expression_appraisal_events(created_at DESC);
