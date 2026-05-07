mod builtin;

use async_trait::async_trait;

pub use builtin::BuiltinSkills;
use crate::providers::adapter::{SkillInfo, SkillMatch, SkillProvider, SkillResult};
use crate::module::ModuleLifecycle;

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

    async fn execute(
        &self,
        _skill: &SkillMatch,
        _params: serde_json::Value,
    ) -> anyhow::Result<SkillResult> {
        Ok(SkillResult {
            success: false,
            output: serde_json::Value::Null,
            message: Some("Skills module not configured".into()),
        })
    }

    async fn list_skills(&self) -> anyhow::Result<Vec<SkillInfo>> {
        Ok(Vec::new())
    }
}
