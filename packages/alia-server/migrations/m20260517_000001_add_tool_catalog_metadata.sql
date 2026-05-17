ALTER TABLE tools
    ADD COLUMN IF NOT EXISTS routing_keywords JSONB NOT NULL DEFAULT '[]',
    ADD COLUMN IF NOT EXISTS routing_examples JSONB NOT NULL DEFAULT '[]',
    ADD COLUMN IF NOT EXISTS routing_text TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS routing_embedding JSONB;

CREATE INDEX IF NOT EXISTS idx_tools_routing_keywords
    ON tools USING GIN (routing_keywords);

