//! Persona resources and prompt rendering; relationship state belongs to Companion.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    pub name: String,
    pub system_prompt_template: String,
    pub personality_traits: serde_json::Value,
    pub voice_config: serde_json::Value,
    pub greeting: Option<String>,
    pub behavior_guidelines: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedPersonaPrompt {
    pub persona: Persona,
    pub system_prompt: String,
}

pub const DEFAULT_ASSISTANT_NAME: &str = "the assistant";

pub fn sanitize_assistant_name(input: &str) -> String {
    let normalized = input
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    let compact = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = compact.trim();
    if trimmed.is_empty() {
        return DEFAULT_ASSISTANT_NAME.to_string();
    }

    trimmed.chars().take(80).collect()
}

mod service;

pub use service::PersonaService;
