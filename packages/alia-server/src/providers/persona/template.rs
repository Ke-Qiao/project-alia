//! Template-based Persona Provider
//!
//! Implements PersonaProvider using Handlebars for system prompt template rendering.
//! Loads personas from database, supports per-user overrides and active persona selection.

use std::sync::Arc;

use async_trait::async_trait;
use handlebars::Handlebars;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::entity::{persona, user_active_persona, user_persona_override};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{Persona, PersonaProvider};

/// Default system prompt template with Handlebars variables
const DEFAULT_SYSTEM_PROMPT: &str = r#"You are Alia, a warm and capable personal assistant for everyday life, smart-home control, and voice-first interaction.

## Core Traits
- Be calm, concise, and genuinely helpful.
- Listen for the user's real intent, especially in short voice commands.
- Use available context carefully, without pretending to know things the system did not provide.
- Respect privacy. Do not ask for sensitive information unless it is necessary for the task.
- Prefer practical next steps over long explanations, unless the user asks for detail.

{{#if behavior_guidelines}}
## Additional Behavior Guidelines
{{{behavior_guidelines}}}
{{/if}}

Respond naturally in the language the user is using when it is clear. Keep voice responses short unless the user asks for a longer answer.
"#;

/// Template-based Persona Provider backed by PostgreSQL
pub struct TemplatePersona {
    handlebars: Arc<RwLock<Handlebars<'static>>>,
    db: Option<DatabaseConnection>,
}

impl TemplatePersona {
    pub fn new() -> Self {
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(false);

        handlebars
            .register_template_string("default", DEFAULT_SYSTEM_PROMPT)
            .expect("Default template should be valid");

        Self {
            handlebars: Arc::new(RwLock::new(handlebars)),
            db: None,
        }
    }

    /// Find the user's active persona from the database, falling back to the
    /// first system persona if no explicit selection exists.
    async fn find_active_persona(
        &self,
        db: &DatabaseConnection,
        user_id: Uuid,
    ) -> anyhow::Result<persona::Model> {
        // Check user_active_persona first
        if let Some(active) = user_active_persona::Entity::find_by_id(user_id)
            .one(db)
            .await?
        {
            if let Some(p) = persona::Entity::find_by_id(active.persona_id)
                .filter(persona::Column::Enabled.eq(true))
                .one(db)
                .await?
            {
                return Ok(p);
            }
        }

        // Fallback: first enabled system persona by sort order
        if let Some(p) = persona::Entity::find()
            .filter(persona::Column::Enabled.eq(true))
            .filter(persona::Column::IsSystem.eq(true))
            .order_by_asc(persona::Column::SortOrder)
            .one(db)
            .await?
        {
            return Ok(p);
        }

        anyhow::bail!("no persona available")
    }

    /// Merge per-user overrides onto a base persona model to produce a `Persona`.
    fn merge_persona(
        base: &persona::Model,
        user_override: Option<&user_persona_override::Model>,
    ) -> Persona {
        Persona {
            name: base.display_name.clone(),
            system_prompt_template: base.system_prompt_template.clone(),
            personality_traits: user_override
                .and_then(|o| o.personality_traits.clone())
                .unwrap_or_else(|| base.personality_traits.clone()),
            voice_config: user_override
                .and_then(|o| o.voice_config.clone())
                .unwrap_or_else(|| base.voice_config.clone()),
            greeting: user_override
                .and_then(|o| o.greeting.clone())
                .or_else(|| base.greeting.clone()),
            behavior_guidelines: user_override
                .and_then(|o| o.behavior_guidelines.clone())
                .or_else(|| base.behavior_guidelines.clone()),
        }
    }

    fn default_persona() -> Persona {
        Persona {
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
            greeting: Some("Hello! How can I help you?".to_string()),
            behavior_guidelines: None,
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

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        self.db = Some(ctx.db.clone());

        // Pre-register templates from all enabled personas
        if let Ok(personas) = persona::Entity::find()
            .filter(persona::Column::Enabled.eq(true))
            .all(&ctx.db)
            .await
        {
            let mut hb = self.handlebars.write().await;
            for p in &personas {
                if let Err(e) = hb.register_template_string(&p.name, &p.system_prompt_template) {
                    tracing::warn!(persona = %p.name, error = ?e, "failed to register persona template, will use default");
                }
            }
            tracing::info!(
                count = personas.len(),
                "TemplatePersona loaded personas from database"
            );
        } else {
            tracing::warn!(
                "TemplatePersona: failed to load personas from database, using defaults"
            );
        }

        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}

#[async_trait]
impl PersonaProvider for TemplatePersona {
    async fn get_effective_persona(&self, user_id: Uuid) -> anyhow::Result<Persona> {
        let Some(db) = &self.db else {
            return Ok(Self::default_persona());
        };

        let persona_model = match self.find_active_persona(db, user_id).await {
            Ok(p) => p,
            Err(_) => return Ok(Self::default_persona()),
        };

        // Load user override for this persona
        let user_override = user_persona_override::Entity::find()
            .filter(user_persona_override::Column::UserId.eq(user_id))
            .filter(user_persona_override::Column::PersonaId.eq(persona_model.id))
            .one(db)
            .await?;

        Ok(Self::merge_persona(&persona_model, user_override.as_ref()))
    }

    async fn render_system_prompt(&self, user_id: Uuid) -> anyhow::Result<String> {
        let persona = self.get_effective_persona(user_id).await?;

        let data = serde_json::json!({
            "persona_name": persona.name,
            "behavior_guidelines": persona.behavior_guidelines,
            "personality_traits": persona.personality_traits,
        });

        let handlebars = self.handlebars.read().await;

        let rendered = match handlebars.render_template(&persona.system_prompt_template, &data) {
            Ok(rendered) => rendered,
            Err(error) => {
                tracing::warn!(
                    persona = %persona.name,
                    error = ?error,
                    "failed to render persona template from database, falling back to default"
                );
                handlebars.render("default", &data)?
            }
        };
        Ok(rendered)
    }
}
