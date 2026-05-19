use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::expression::ExpressionRuntime;
use crate::module::lifecycle::{ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    CacheBackend, ConversationBackend, EmbeddingAdapter, InsightStrategy, LlmAdapter,
    MemoryBackend, PersonaProvider, RouterStrategy, RuleEvaluator, SchedulerBackend, SkillProvider,
    SttAdapter, ToolRegistry, TtsAdapter,
};

/// Runtime module registry — read-only after construction, shared via Arc
pub struct ModuleRegistry {
    // AI adapters
    pub llm: Arc<dyn LlmAdapter>,
    pub minor_llm: Arc<dyn LlmAdapter>,
    pub major_llm: Arc<dyn LlmAdapter>,
    pub stt: Arc<dyn SttAdapter>,
    pub tts: Arc<dyn TtsAdapter>,
    pub embedding: Arc<dyn EmbeddingAdapter>,
    // Intelligence core
    pub conversation: Arc<dyn ConversationBackend>,
    pub memory: Arc<dyn MemoryBackend>,
    pub cache: Arc<dyn CacheBackend>,
    pub skills: Arc<dyn SkillProvider>,
    pub persona: Arc<dyn PersonaProvider>,
    pub insights: Arc<dyn InsightStrategy>,
    pub router: Arc<dyn RouterStrategy>,
    // Infrastructure
    pub rule_engine: Arc<dyn RuleEvaluator>,
    pub scheduler: Arc<dyn SchedulerBackend>,
    pub tools: Arc<dyn ToolRegistry>,
    pub expression: Arc<ExpressionRuntime>,
    // Custom extensions
    pub extensions: Vec<Arc<dyn ModuleLifecycle>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleSettingsApplyReport {
    pub module: String,
    pub applied: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ModuleRegistry {
    /// Graceful shutdown: stop all modules in reverse order
    pub async fn shutdown(&self) {
        // Stop extensions in reverse
        for ext in self.extensions.iter().rev() {
            if let Err(e) = ext.stop().await {
                tracing::warn!(module = ext.name(), error = ?e, "module stop failed");
            }
        }

        // Stop core modules in reverse dependency order
        let modules: Vec<&dyn ModuleLifecycle> = vec![
            self.scheduler.as_ref(),
            self.tools.as_ref(),
            self.insights.as_ref(),
            self.rule_engine.as_ref(),
            self.router.as_ref(),
            self.persona.as_ref(),
            self.skills.as_ref(),
            self.cache.as_ref(),
            self.memory.as_ref(),
            self.conversation.as_ref(),
            self.tts.as_ref(),
            self.stt.as_ref(),
            self.embedding.as_ref(),
            self.major_llm.as_ref(),
            self.minor_llm.as_ref(),
        ];
        for m in modules {
            if let Err(e) = m.stop().await {
                tracing::warn!(module = m.name(), error = ?e, "module stop failed");
            }
        }
    }

    /// Aggregate health check for GET /health endpoint
    pub async fn health(&self) -> HashMap<String, ModuleHealth> {
        let mut results = HashMap::new();
        results.insert("llm.minor".to_string(), self.minor_llm.health_check().await);
        results.insert("llm.major".to_string(), self.major_llm.health_check().await);
        results.insert("stt".to_string(), self.stt.health_check().await);
        results.insert("tts".to_string(), self.tts.health_check().await);
        results.insert("embedding".to_string(), self.embedding.health_check().await);
        results.insert(
            "conversation".to_string(),
            self.conversation.health_check().await,
        );
        results.insert("memory".to_string(), self.memory.health_check().await);
        results.insert("cache".to_string(), self.cache.health_check().await);
        results.insert("skills".to_string(), self.skills.health_check().await);
        results.insert("persona".to_string(), self.persona.health_check().await);
        results.insert("insights".to_string(), self.insights.health_check().await);
        results.insert("router".to_string(), self.router.health_check().await);
        results.insert(
            "rule_engine".to_string(),
            self.rule_engine.health_check().await,
        );
        results.insert("scheduler".to_string(), self.scheduler.health_check().await);
        results.insert("tools".to_string(), self.tools.health_check().await);
        results
    }

    /// Apply runtime settings to modules that support hot reload.
    pub async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> Vec<ModuleSettingsApplyReport> {
        let modules: Vec<&dyn ModuleLifecycle> = vec![
            self.minor_llm.as_ref(),
            self.major_llm.as_ref(),
            self.stt.as_ref(),
            self.tts.as_ref(),
            self.embedding.as_ref(),
            self.cache.as_ref(),
            self.skills.as_ref(),
            self.router.as_ref(),
            self.insights.as_ref(),
            self.rule_engine.as_ref(),
            self.tools.as_ref(),
        ];

        let mut reports = Vec::new();
        for module in modules {
            match module.apply_settings(updates).await {
                Ok(applied) if !applied.is_empty() => reports.push(ModuleSettingsApplyReport {
                    module: module.name().to_string(),
                    applied,
                    error: None,
                }),
                Ok(_) => {}
                Err(error) => reports.push(ModuleSettingsApplyReport {
                    module: module.name().to_string(),
                    applied: Vec::new(),
                    error: Some(error.to_string()),
                }),
            }
        }

        for extension in &self.extensions {
            match extension.apply_settings(updates).await {
                Ok(applied) if !applied.is_empty() => reports.push(ModuleSettingsApplyReport {
                    module: extension.name().to_string(),
                    applied,
                    error: None,
                }),
                Ok(_) => {}
                Err(error) => reports.push(ModuleSettingsApplyReport {
                    module: extension.name().to_string(),
                    applied: Vec::new(),
                    error: Some(error.to_string()),
                }),
            }
        }

        reports
    }
}
