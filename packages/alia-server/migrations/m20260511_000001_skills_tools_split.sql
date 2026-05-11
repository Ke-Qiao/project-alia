-- Migration: Update skills table and create tools table for the skills/tools split
--
-- Skills = extend what the LLM knows how to do well
-- Tools  = extend what the LLM can do
--
-- Changes:
--   1. Skills: add `source` column to track who registered the skill (builtin, file, plugin)
--   2. Skills: add `tools` column (text array of tool names) replacing execution_config semantics
--   3. Create `tools` table for DB-backed tool definitions (plugin-provided or configured)

-- ── Skills table updates ───────────────────────────────────────────────

-- Track where the skill came from (builtin seed, markdown file, api, or a plugin)
ALTER TABLE skills ADD COLUMN IF NOT EXISTS source VARCHAR(100) DEFAULT 'builtin';

-- Explicit tool references as JSONB array of strings (e.g. ["device_control", "sensor_read"])
-- While execution_config is kept for backward compat, this column is the new standard.
ALTER TABLE skills ADD COLUMN IF NOT EXISTS tools JSONB DEFAULT '[]';

-- Backfill: migrate execution_config → tools column for existing rows
-- New-format (JSON array of strings): copy directly
UPDATE skills
SET tools = execution_config
WHERE execution_config IS NOT NULL
  AND jsonb_typeof(execution_config) = 'array'
  AND (tools IS NULL OR tools = '[]'::jsonb);

-- Legacy-format ({"function":{"name":"..."}}) → extract into single-element array
UPDATE skills
SET tools = jsonb_build_array(execution_config -> 'function' ->> 'name')
WHERE execution_config IS NOT NULL
  AND jsonb_typeof(execution_config) = 'object'
  AND execution_config -> 'function' ->> 'name' IS NOT NULL
  AND (tools IS NULL OR tools = '[]'::jsonb);

-- Index for source filtering (e.g. list all plugin-provided skills)
CREATE INDEX IF NOT EXISTS idx_skills_source ON skills(source);

-- ── Tools table ────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS tools (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(100) UNIQUE NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    parameters JSONB DEFAULT '{}',          -- JSON Schema for function arguments
    source VARCHAR(100) NOT NULL,           -- 'builtin' | plugin name
    require_confirmation BOOLEAN DEFAULT FALSE,
    required_role VARCHAR(50),              -- Optional role requirement
    enabled BOOLEAN DEFAULT TRUE,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- Index for source filtering (e.g. list all tools from a specific plugin)
CREATE INDEX IF NOT EXISTS idx_tools_source ON tools(source);
-- Index for enabled tools lookup
CREATE INDEX IF NOT EXISTS idx_tools_enabled ON tools(enabled) WHERE enabled = TRUE;
