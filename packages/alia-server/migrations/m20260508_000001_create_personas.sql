-- Migration: Create personas and user persona overrides tables
-- Personas define AI personality, system prompt templates, and voice configuration.
-- Users can override specific fields per persona via user_persona_overrides.

CREATE TABLE personas (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(100) UNIQUE NOT NULL,
    display_name VARCHAR(100) NOT NULL,
    description TEXT,
    -- Handlebars template for system prompt
    system_prompt_template TEXT NOT NULL,
    -- Personality traits (e.g. {"warmth": 0.8, "patience": 0.9})
    personality_traits JSONB NOT NULL DEFAULT '{}',
    -- Voice synthesis config (language, style, speaker_id, etc.)
    voice_config JSONB NOT NULL DEFAULT '{}',
    -- Greeting message shown on conversation start
    greeting TEXT,
    -- Additional behavior guidelines injected into prompt
    behavior_guidelines TEXT,
    -- Whether this is a system-provided persona (vs user-created)
    is_system BOOLEAN NOT NULL DEFAULT FALSE,
    -- Whether this persona is available for selection
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    -- Sort order for listing
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Per-user overrides for a persona (partial fields)
CREATE TABLE user_persona_overrides (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL,
    persona_id UUID NOT NULL REFERENCES personas(id) ON DELETE CASCADE,
    -- Partial overrides (null = use persona default)
    greeting TEXT,
    behavior_guidelines TEXT,
    personality_traits JSONB,
    voice_config JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, persona_id)
);

-- Track which persona each user has selected as active
CREATE TABLE user_active_persona (
    user_id UUID PRIMARY KEY,
    persona_id UUID NOT NULL REFERENCES personas(id) ON DELETE CASCADE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes
CREATE INDEX idx_personas_enabled ON personas(enabled) WHERE enabled = TRUE;
CREATE INDEX idx_personas_system ON personas(is_system);
CREATE INDEX idx_user_persona_overrides_user ON user_persona_overrides(user_id);
CREATE INDEX idx_user_persona_overrides_persona ON user_persona_overrides(persona_id);

-- Seed default Alia persona
INSERT INTO personas (name, display_name, description, system_prompt_template, personality_traits, voice_config, greeting, is_system, sort_order)
VALUES (
    'alia',
    'Alia',
    'Default personal assistant persona for voice-first daily help and smart-home control',
    E'You are Alia, a warm and capable personal assistant for everyday life, smart-home control, and voice-first interaction.\n\n## Core Traits\n- Be calm, concise, and genuinely helpful.\n- Listen for the real intent, especially in short voice commands.\n- Use available context carefully, without pretending to know things the system did not provide.\n- Respect privacy. Do not ask for sensitive information unless it is necessary for the task.\n- Prefer practical next steps over long explanations, unless the user asks for detail.\n\n{{#if behavior_guidelines}}\n## Additional Behavior Guidelines\n{{{behavior_guidelines}}}\n{{/if}}\n\nRespond naturally in the language the user is using when it is clear. Keep voice responses short unless the user asks for a longer answer.',
    '{"warmth": 0.8, "patience": 0.9, "curiosity": 0.7, "formality": 0.3}',
    '{"language": "zh-CN", "style": "warm"}',
    'Hello! How can I help you?',
    TRUE,
    0
);
