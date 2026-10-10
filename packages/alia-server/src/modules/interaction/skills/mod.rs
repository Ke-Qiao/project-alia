//! Skill context and preferred tool metadata.
//!
//! Skills provide domain knowledge and preferred tool ranking; tool visibility
//! remains governed by the global tool router and policy layer.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMatch {
    pub name: String,
    pub confidence: f32,
}

/// Resolved skill context for LLM-driven execution.
///
/// When a skill is triggered, Interaction sends this to the LLM:
/// - `system_prompt` provides domain-specific instructions
/// - `preferred_tools` lists tools to boost during global tool selection
/// - The LLM decides which tools to call (if any)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillResolution {
    pub skill_name: String,
    /// Domain-specific system prompt (from the skill's prompt_template)
    pub system_prompt: Option<String>,
    /// Tool names to prefer during this skill execution.
    /// Empty means this skill contributes prompt context only.
    pub preferred_tools: Vec<String>,
    /// Whether the LLM response can be cached
    pub cacheable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub category: String,
}

pub mod markdown;
pub mod registry;

pub use registry::SkillRegistry;
