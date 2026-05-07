-- Migration: Create skills table for structured skill definitions
-- Skills map user intents to deterministic actions without full LLM reasoning

CREATE TABLE skills (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(100) UNIQUE NOT NULL,
    display_name VARCHAR(100) NOT NULL,
    description TEXT,
    category VARCHAR(50) NOT NULL,              -- 'smart_home' | 'utility' | 'information' | 'memory' | 'automation'
    intent_patterns JSONB NOT NULL,             -- Intent matching patterns/keywords
    parameters JSONB DEFAULT '{}',              -- Parameter definitions (JSON Schema)
    execution_config JSONB,                     -- Internal execution routing (plugin, action)
    prompt_template TEXT,                       -- Response template for simple confirmations
    cacheable BOOLEAN DEFAULT FALSE,
    enabled BOOLEAN DEFAULT TRUE,
    priority INTEGER DEFAULT 0,                 -- Higher priority wins on conflict
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- Index for category filtering
CREATE INDEX idx_skills_category ON skills(category);
-- Index for enabled skills lookup
CREATE INDEX idx_skills_enabled ON skills(enabled) WHERE enabled = TRUE;
-- Priority ordering
CREATE INDEX idx_skills_priority ON skills(priority DESC);
