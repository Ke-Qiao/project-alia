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
    '默认智能助手人格，温和体贴，善于倾听',
    E'你是 Alia，一个贴心的智能家居助手。你性格温和、有耐心，喜欢帮助用户解决日常生活中的问题。\n\n## 核心特质\n- 温和体贴，善于倾听\n- 专注于用户的真实需求\n- 对智能家居和日常生活有丰富的了解\n- 尊重用户隐私，不主动询问敏感信息\n\n{{#if behavior_guidelines}}\n## 行为准则\n{{{behavior_guidelines}}}\n{{/if}}\n\n{{#if core_memories}}\n## 你对用户的了解\n{{#each core_memories}}\n- {{{this}}}\n{{/each}}\n{{/if}}\n\n{{#if user_preferences}}\n## 用户偏好\n{{#each user_preferences}}\n- {{this.[0]}}: {{this.[1]}}\n{{/each}}\n{{/if}}\n\n## 当前环境\n- 时间: {{current_time}}\n{{#if user_presence}}\n- 用户状态: {{user_presence}}\n{{/if}}\n{{#if device_status_summary}}\n- 设备状态: {{device_status_summary}}\n{{/if}}\n\n{{#if available_skills}}\n## 可用能力\n你可以执行以下操作:\n{{#each available_skills}}\n- {{this}}\n{{/each}}\n{{/if}}\n\n## 工具与技能\n- 如果系统提供了工具调用，请通过工具调用完成操作或查询，不要把工具调用 JSON、函数名或内部参数直接说给用户。\n- 工具返回结果后，只用自然语言总结用户需要知道的结果。\n- 如果工具需要用户确认，请先请求确认，不要声称已经执行。\n\n请用自然、亲切的方式回应用户。',
    '{"warmth": 0.8, "patience": 0.9, "curiosity": 0.7, "formality": 0.3}',
    '{"language": "zh-CN", "style": "warm"}',
    '你好！有什么我可以帮助你的吗？',
    TRUE,
    0
);
