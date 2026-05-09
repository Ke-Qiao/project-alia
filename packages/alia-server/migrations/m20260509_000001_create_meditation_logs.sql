CREATE TABLE IF NOT EXISTS meditation_logs (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL,
    phase VARCHAR(20) NOT NULL,
    trigger_type VARCHAR(20) NOT NULL,
    summary TEXT NOT NULL,
    nodes_created INTEGER NOT NULL DEFAULT 0,
    nodes_updated INTEGER NOT NULL DEFAULT 0,
    nodes_merged INTEGER NOT NULL DEFAULT 0,
    edges_created INTEGER NOT NULL DEFAULT 0,
    model_used VARCHAR(50),
    duration_ms INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_meditation_logs_user_id ON meditation_logs(user_id);
CREATE INDEX idx_meditation_logs_phase ON meditation_logs(phase);
CREATE INDEX idx_meditation_logs_created_at ON meditation_logs(created_at DESC);
