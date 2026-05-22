-- Migration: Normalize skill/preferred-tool architecture.
--
-- Skills are cognitive context only; tools are executable units. Skill/tool
-- preferences live in the explicit junction table below.

CREATE TABLE IF NOT EXISTS skill_preferred_tools (
    skill_id UUID NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    tool_id UUID NOT NULL REFERENCES tools(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (skill_id, tool_id)
);

CREATE INDEX IF NOT EXISTS idx_skill_preferred_tools_tool_id ON skill_preferred_tools(tool_id);

ALTER TABLE skills DROP COLUMN IF EXISTS parameters;
ALTER TABLE skills DROP COLUMN IF EXISTS execution_config;
ALTER TABLE skills DROP COLUMN IF EXISTS tools;
