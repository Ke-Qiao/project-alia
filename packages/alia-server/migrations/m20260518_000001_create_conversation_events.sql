-- Migration: Create conversation_events table for traceable agent history

CREATE TABLE IF NOT EXISTS conversation_events (
    id UUID PRIMARY KEY,
    session_id UUID NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
    turn_id UUID NOT NULL,
    sequence INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    actor TEXT NOT NULL,
    summary TEXT,
    payload JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (session_id, turn_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_conversation_events_session_time
    ON conversation_events(session_id, created_at ASC, sequence ASC);

CREATE INDEX IF NOT EXISTS idx_conversation_events_turn
    ON conversation_events(turn_id, sequence ASC);
