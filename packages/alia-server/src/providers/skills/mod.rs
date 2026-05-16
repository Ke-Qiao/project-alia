//! Skills module: extend what the LLM **knows how to do well**.
//!
//! Skills provide domain knowledge and routing context. Tool bindings are
//! injected into the LLM separately; skills are never executable functions.
//!
//! This module provides:
//! - `NoopSkills`: A no-op implementation for disabled skills
//! - `BuiltinSkills`: Full DB-backed implementation with markdown file support

pub mod builtin;
pub mod markdown;

use async_trait::async_trait;

use crate::module::ModuleLifecycle;
use crate::providers::adapter::{
    PluginSkillDefinition, SkillInfo, SkillMatch, SkillProvider, SkillResolution,
};
pub use builtin::BuiltinSkills;

pub struct NoopSkills;

#[async_trait]
impl ModuleLifecycle for NoopSkills {
    fn name(&self) -> &'static str {
        "noop_skills"
    }
}

#[async_trait]
impl SkillProvider for NoopSkills {
    async fn match_skill(&self, _intent: &str) -> anyhow::Result<Option<SkillMatch>> {
        Ok(None)
    }

    async fn resolve(&self, skill: &SkillMatch) -> anyhow::Result<SkillResolution> {
        Ok(SkillResolution {
            skill_name: skill.name.clone(),
            system_prompt: None,
            tools: Vec::new(),
            cacheable: false,
        })
    }

    async fn list_skills(&self) -> anyhow::Result<Vec<SkillInfo>> {
        Ok(Vec::new())
    }

    async fn register_plugin_skill(
        &self,
        _plugin_name: &str,
        _skill: PluginSkillDefinition,
    ) -> anyhow::Result<()> {
        anyhow::bail!("skills are disabled")
    }

    async fn unregister_plugin_skills(&self, _plugin_name: &str) -> anyhow::Result<u64> {
        Ok(0)
    }
}
