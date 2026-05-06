use std::collections::HashMap;
use std::sync::Arc;

use crate::providers::adapter::{
    CacheBackend, EmbeddingAdapter, LlmAdapter, MeditationStrategy, MemoryBackend,
    PersonaProvider, RouterStrategy, RuleEvaluator, SkillProvider, SttAdapter, TtsAdapter,
};
use crate::module::lifecycle::{ModuleHealth, ModuleLifecycle};

/// Runtime module registry — read-only after construction, shared via Arc
pub struct ModuleRegistry {
    // AI adapters
    pub llm: Arc<dyn LlmAdapter>,
    pub stt: Arc<dyn SttAdapter>,
    pub tts: Arc<dyn TtsAdapter>,
    pub embedding: Arc<dyn EmbeddingAdapter>,
    // Intelligence core
    pub memory: Arc<dyn MemoryBackend>,
    pub cache: Arc<dyn CacheBackend>,
    pub skills: Arc<dyn SkillProvider>,
    pub persona: Arc<dyn PersonaProvider>,
    pub meditation: Arc<dyn MeditationStrategy>,
    pub router: Arc<dyn RouterStrategy>,
    // Infrastructure
    pub rule_engine: Arc<dyn RuleEvaluator>,
    // Custom extensions
    pub extensions: Vec<Arc<dyn ModuleLifecycle>>,
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
            self.meditation.as_ref(),
            self.rule_engine.as_ref(),
            self.router.as_ref(),
            self.persona.as_ref(),
            self.skills.as_ref(),
            self.cache.as_ref(),
            self.memory.as_ref(),
            self.tts.as_ref(),
            self.stt.as_ref(),
            self.embedding.as_ref(),
            self.llm.as_ref(),
        ];
        for m in modules {
            if let Err(e) = m.stop().await {
                tracing::warn!(module = m.name(), error = ?e, "module stop failed");
            }
        }
    }

    /// Aggregate health check for GET /health endpoint
    pub async fn health(&self) -> HashMap<&str, ModuleHealth> {
        let mut results = HashMap::new();
        results.insert(self.llm.name(), self.llm.health_check().await);
        results.insert(self.stt.name(), self.stt.health_check().await);
        results.insert(self.tts.name(), self.tts.health_check().await);
        results.insert(self.embedding.name(), self.embedding.health_check().await);
        results.insert(self.memory.name(), self.memory.health_check().await);
        results.insert(self.cache.name(), self.cache.health_check().await);
        results.insert(self.skills.name(), self.skills.health_check().await);
        results.insert(self.persona.name(), self.persona.health_check().await);
        results.insert(self.meditation.name(), self.meditation.health_check().await);
        results.insert(self.router.name(), self.router.health_check().await);
        results.insert(
            self.rule_engine.name(),
            self.rule_engine.health_check().await,
        );
        results
    }
}
