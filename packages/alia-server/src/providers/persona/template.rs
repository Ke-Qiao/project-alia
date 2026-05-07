//! Template-based Persona Provider
//!
//! Implements PersonaProvider using Handlebars for system prompt template rendering.
//! Supports variable substitution for memories, time, device status, etc.

use std::sync::Arc;

use async_trait::async_trait;
use handlebars::Handlebars;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{Persona, PersonaProvider, PromptVars};

/// Default system prompt template with Handlebars variables
const DEFAULT_SYSTEM_PROMPT: &str = r#"你是 Alia，一个贴心的智能家居助手。你性格温和、有耐心，喜欢帮助用户解决日常生活中的问题。

## 核心特质
- 温和体贴，善于倾听
- 专注于用户的真实需求
- 对智能家居和日常生活有丰富的了解
- 尊重用户隐私，不主动询问敏感信息

{{#if behavior_guidelines}}
## 行为准则
{{{behavior_guidelines}}}
{{/if}}

{{#if core_memories}}
## 你对用户的了解
{{#each core_memories}}
- {{{this}}}
{{/each}}
{{/if}}

{{#if user_preferences}}
## 用户偏好
{{#each user_preferences}}
- {{this.[0]}}: {{this.[1]}}
{{/each}}
{{/if}}

## 当前环境
- 时间: {{current_time}}
{{#if user_presence}}
- 用户状态: {{user_presence}}
{{/if}}
{{#if device_status_summary}}
- 设备状态: {{device_status_summary}}
{{/if}}

{{#if available_skills}}
## 可用能力
你可以执行以下操作:
{{#each available_skills}}
- {{this}}
{{/each}}
{{/if}}

请用自然、亲切的方式回应用户。
"#;

/// Template-based Persona Provider
pub struct TemplatePersona {
    handlebars: Arc<RwLock<Handlebars<'static>>>,
    default_persona: Persona,
}

impl TemplatePersona {
    pub fn new() -> Self {
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(false); // Allow missing variables
        
        // Register default template
        handlebars
            .register_template_string("system_prompt", DEFAULT_SYSTEM_PROMPT)
            .expect("Default template should be valid");

        Self {
            handlebars: Arc::new(RwLock::new(handlebars)),
            default_persona: Persona {
                name: "Alia".to_string(),
                system_prompt_template: DEFAULT_SYSTEM_PROMPT.to_string(),
                personality_traits: serde_json::json!({
                    "warmth": 0.8,
                    "patience": 0.9,
                    "curiosity": 0.7,
                    "formality": 0.3
                }),
                voice_config: serde_json::json!({
                    "language": "zh-CN",
                    "style": "warm"
                }),
                greeting: Some("你好！有什么我可以帮助你的吗？".to_string()),
                behavior_guidelines: None,
            },
        }
    }
}

impl Default for TemplatePersona {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for TemplatePersona {
    fn name(&self) -> &'static str {
        "template"
    }

    async fn init(&mut self, _ctx: &ModuleContext) -> anyhow::Result<()> {
        // In the future, load user-customized personas from database
        tracing::info!("TemplatePersona initialized with default Alia persona");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}

#[async_trait]
impl PersonaProvider for TemplatePersona {
    async fn get_effective_persona(&self, _user_id: Uuid) -> anyhow::Result<Persona> {
        // TODO: Load user-specific persona overrides from database
        // For now, return the default persona
        Ok(self.default_persona.clone())
    }

    async fn render_system_prompt(
        &self,
        user_id: Uuid,
        vars: PromptVars,
    ) -> anyhow::Result<String> {
        let persona = self.get_effective_persona(user_id).await?;
        
        // Build template data
        let data = serde_json::json!({
            "persona_name": persona.name,
            "core_memories": vars.core_memories,
            "user_preferences": vars.user_preferences,
            "current_time": vars.current_time,
            "user_presence": vars.user_presence,
            "device_status_summary": vars.device_status_summary,
            "available_skills": vars.available_skills,
            "behavior_guidelines": persona.behavior_guidelines,
        });

        let handlebars = self.handlebars.read().await;
        
        // Try to render user's custom template, fallback to default
        let rendered = handlebars.render("system_prompt", &data)?;
        
        Ok(rendered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_render_system_prompt() {
        let persona = TemplatePersona::new();
        let vars = PromptVars {
            core_memories: vec!["用户喜欢喝咖啡".to_string()],
            user_preferences: vec![("温度".to_string(), "26°C".to_string())],
            current_time: "2025-05-07 14:30".to_string(),
            user_presence: "在家".to_string(),
            device_status_summary: "空调运行中".to_string(),
            available_skills: vec!["控制空调".to_string(), "播放音乐".to_string()],
        };
        
        let result = persona
            .render_system_prompt(Uuid::new_v4(), vars)
            .await
            .unwrap();
        
        assert!(result.contains("Alia"));
        assert!(result.contains("用户喜欢喝咖啡"));
        assert!(result.contains("26°C"));
    }
}
