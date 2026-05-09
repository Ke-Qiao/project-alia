//! Persona providers for AI personality and prompt management.
//!
//! Available implementations:
//! - `TemplatePersona`: Handlebars template-based system prompt rendering
//! - `NoopPersona`: No-op placeholder with default Alia persona

pub mod template;

pub use template::TemplatePersona;

use async_trait::async_trait;
use uuid::Uuid;

use crate::module::ModuleLifecycle;
use crate::providers::adapter::{Persona, PersonaProvider, PromptVars};

pub struct NoopPersona;

#[async_trait]
impl ModuleLifecycle for NoopPersona {
    fn name(&self) -> &'static str {
        "noop_persona"
    }
}

#[async_trait]
impl PersonaProvider for NoopPersona {
    async fn get_effective_persona(&self, _user_id: Uuid) -> anyhow::Result<Persona> {
        Ok(Persona {
            name: "Alia".into(),
            system_prompt_template: "You are Alia, a helpful assistant.".into(),
            personality_traits: serde_json::json!({}),
            voice_config: serde_json::json!({}),
            greeting: Some("Hello!".into()),
            behavior_guidelines: None,
        })
    }

    async fn render_system_prompt(
        &self,
        _user_id: Uuid,
        _vars: PromptVars,
    ) -> anyhow::Result<String> {
        Ok("You are Alia, a helpful assistant.".into())
    }
}
