use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::expression::ExpressionRuntime;
use crate::modules::platform::lifecycle::{ModuleHealth, ModuleLifecycle};
use crate::{
    modules::{
        interaction::{
            cache::CacheService, conversation::ConversationService, persona::PersonaService,
            routing::RouterService, skills::SkillRegistry,
        },
        platform::insights::InsightService,
    },
    providers::adapter::{
        EmbeddingAdapter, LlmAdapter, RuleEvaluator, SchedulerBackend, SttAdapter, ToolRegistry,
        TtsAdapter,
    },
};

/// Runtime module registry — read-only after construction, shared via Arc
pub struct ModuleRegistry {
    pub(crate) cleanup: Arc<crate::modules::platform::lifecycle::RuntimeCleanup>,
    // AI adapters
    pub llm: Arc<dyn LlmAdapter>,
    pub minor_llm: Arc<dyn LlmAdapter>,
    pub major_llm: Arc<dyn LlmAdapter>,
    pub memory_capture_llm: Option<Arc<dyn LlmAdapter>>,
    pub sentence_expression_llm: Option<Arc<dyn LlmAdapter>>,
    pub stt: Arc<dyn SttAdapter>,
    pub tts: Arc<dyn TtsAdapter>,
    pub embedding: Arc<dyn EmbeddingAdapter>,
    // Intelligence core
    pub conversation: Arc<ConversationService>,
    pub responses: Arc<crate::modules::interaction::response::ResponseCommitter>,
    pub memory: Arc<crate::modules::memory::MemoryService>,
    pub context: Arc<crate::modules::memory::context::ContextService>,
    pub companion: Arc<crate::modules::companion::CompanionService>,
    pub cache: Arc<CacheService>,
    pub skills: Arc<SkillRegistry>,
    pub persona: Arc<PersonaService>,
    pub insights: Arc<InsightService>,
    pub router: Arc<RouterService>,
    // Infrastructure
    pub rule_engine: Arc<dyn RuleEvaluator>,
    pub scheduler: Arc<dyn SchedulerBackend>,
    pub tools: Arc<dyn ToolRegistry>,
    pub expression: Arc<ExpressionRuntime>,
    pub delivery: Arc<crate::modules::delivery::DeliveryService>,
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
    /// Request dependency-ordered cleanup; a deadline retains the cleanup owner.
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.cleanup
            .start()
            .wait_until(self.cleanup.deadline())
            .await
    }

    /// Aggregate health check for GET /health endpoint
    pub async fn health(&self) -> HashMap<String, ModuleHealth> {
        let mut results = HashMap::new();
        results.insert("llm.minor".to_string(), self.minor_llm.health_check().await);
        results.insert("llm.major".to_string(), self.major_llm.health_check().await);
        if let Some(llm) = &self.memory_capture_llm {
            results.insert("llm.memory_capture".to_string(), llm.health_check().await);
        }
        if let Some(llm) = &self.sentence_expression_llm {
            results.insert(
                "llm.sentence_expression".to_string(),
                llm.health_check().await,
            );
        }
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
        let modules: Vec<Option<&dyn ModuleLifecycle>> = vec![
            Some(self.minor_llm.as_ref()),
            Some(self.major_llm.as_ref()),
            self.memory_capture_llm
                .as_deref()
                .map(|module| module as &dyn ModuleLifecycle),
            self.sentence_expression_llm
                .as_deref()
                .map(|module| module as &dyn ModuleLifecycle),
            Some(self.stt.as_ref()),
            Some(self.tts.as_ref()),
            Some(self.embedding.as_ref()),
            Some(self.cache.as_ref()),
            Some(self.skills.as_ref()),
            Some(self.router.as_ref()),
            Some(self.insights.as_ref()),
            Some(self.rule_engine.as_ref()),
            Some(self.tools.as_ref()),
        ];

        let mut reports = Vec::new();
        for module in modules.into_iter().flatten() {
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
