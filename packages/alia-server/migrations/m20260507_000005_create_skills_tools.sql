-- Migration: Create skills and tools tables
--
-- Skills extend what the LLM **knows how to do well**.
-- Each skill carries intent patterns and a prompt template for domain context.
--
-- Tools extend what the LLM **can do**.
-- Each tool is an executable function the LLM can call via function calling.

-- ── Skills ─────────────────────────────────────────────────────────────

CREATE TABLE skills (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(100) UNIQUE NOT NULL,
    display_name VARCHAR(100) NOT NULL,
    description TEXT,
    category VARCHAR(50) NOT NULL,              -- 'smart_home' | 'utility' | 'information' | 'memory' | 'automation'
    intent_patterns JSONB NOT NULL,             -- Intent matching patterns/keywords
    prompt_template TEXT,                       -- System prompt / domain context for LLM
    cacheable BOOLEAN DEFAULT FALSE,
    enabled BOOLEAN DEFAULT TRUE,
    priority INTEGER DEFAULT 0,                 -- Higher priority wins on conflict
    source VARCHAR(100) DEFAULT 'builtin',      -- Origin: 'builtin' | 'file' | 'api' | plugin name
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_skills_category ON skills(category);
CREATE INDEX idx_skills_enabled ON skills(enabled) WHERE enabled = TRUE;
CREATE INDEX idx_skills_priority ON skills(priority DESC);
CREATE INDEX idx_skills_source ON skills(source);

-- ── Tools ──────────────────────────────────────────────────────────────

CREATE TABLE tools (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(100) UNIQUE NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    parameters JSONB DEFAULT '{}',              -- JSON Schema for function arguments
    source VARCHAR(100) NOT NULL,               -- 'builtin' | plugin name
    require_confirmation BOOLEAN DEFAULT FALSE,
    required_role VARCHAR(50),                  -- Optional role requirement
    enabled BOOLEAN DEFAULT TRUE,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_tools_source ON tools(source);
CREATE INDEX idx_tools_enabled ON tools(enabled) WHERE enabled = TRUE;

-- ── Skill Preferred Tool Bindings ─────────────────────────────────────

CREATE TABLE skill_preferred_tools (
    skill_id UUID NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    tool_id UUID NOT NULL REFERENCES tools(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (skill_id, tool_id)
);

CREATE INDEX idx_skill_preferred_tools_tool_id ON skill_preferred_tools(tool_id);
