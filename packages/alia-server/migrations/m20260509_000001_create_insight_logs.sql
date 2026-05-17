CREATE TABLE IF NOT EXISTS insight_logs (
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

CREATE INDEX idx_insight_logs_user_id ON insight_logs(user_id);
CREATE INDEX idx_insight_logs_phase ON insight_logs(phase);
CREATE INDEX idx_insight_logs_created_at ON insight_logs(created_at DESC);
