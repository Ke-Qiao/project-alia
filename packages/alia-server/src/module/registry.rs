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
    pub transcript_correction_llm: Option<Arc<dyn LlmAdapter>>,
    pub memory_capture_llm: Option<Arc<dyn LlmAdapter>>,
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
        let modules: Vec<Option<&dyn ModuleLifecycle>> = vec![
            Some(self.scheduler.as_ref()),
            Some(self.tools.as_ref()),
            Some(self.insights.as_ref()),
            Some(self.rule_engine.as_ref()),
            Some(self.router.as_ref()),
            Some(self.persona.as_ref()),
            Some(self.skills.as_ref()),
            Some(self.cache.as_ref()),
            Some(self.memory.as_ref()),
            Some(self.conversation.as_ref()),
            Some(self.tts.as_ref()),
            Some(self.stt.as_ref()),
            Some(self.embedding.as_ref()),
            self.transcript_correction_llm
                .as_deref()
                .map(|module| module as &dyn ModuleLifecycle),
            self.memory_capture_llm
                .as_deref()
                .map(|module| module as &dyn ModuleLifecycle),
            Some(self.major_llm.as_ref()),
            Some(self.minor_llm.as_ref()),
        ];
        for m in modules.into_iter().flatten() {
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
        if let Some(llm) = &self.transcript_correction_llm {
            results.insert(
                "llm.transcript_correction".to_string(),
                llm.health_check().await,
            );
        }
        if let Some(llm) = &self.memory_capture_llm {
            results.insert("llm.memory_capture".to_string(), llm.health_check().await);
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
            self.transcript_correction_llm
                .as_deref()
                .map(|module| module as &dyn ModuleLifecycle),
            self.memory_capture_llm
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
