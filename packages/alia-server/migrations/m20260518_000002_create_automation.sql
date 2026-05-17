CREATE TABLE IF NOT EXISTS automation_rules (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    event_pattern JSONB NOT NULL DEFAULT '{}',
    conditions JSONB NOT NULL DEFAULT '[]',
    action JSONB NOT NULL,
    cooldown_sec INTEGER NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL DEFAULT 'user',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_automation_rules_user_id
    ON automation_rules (user_id);

CREATE INDEX IF NOT EXISTS idx_automation_rules_enabled
    ON automation_rules (enabled)
    WHERE enabled = TRUE;

CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    schedule JSONB NOT NULL,
    conditions JSONB NOT NULL DEFAULT '[]',
    action JSONB NOT NULL,
    cooldown_sec INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL DEFAULT 'user',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_user_id
    ON scheduled_tasks (user_id);

CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_enabled
    ON scheduled_tasks (enabled)
    WHERE enabled = TRUE;

CREATE TABLE IF NOT EXISTS event_subscriptions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    event_pattern JSONB NOT NULL DEFAULT '{}',
    conditions JSONB NOT NULL DEFAULT '[]',
    action JSONB NOT NULL,
    cooldown_sec INTEGER NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL DEFAULT 'user',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_event_subscriptions_user_id
    ON event_subscriptions (user_id);

CREATE INDEX IF NOT EXISTS idx_event_subscriptions_enabled
    ON event_subscriptions (enabled)
    WHERE enabled = TRUE;
