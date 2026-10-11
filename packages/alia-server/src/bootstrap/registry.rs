use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

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
    pub(crate) health_gate: Mutex<()>,
    pub(crate) health_cancel: CancellationToken,
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
        let mut modules: Vec<(&str, &dyn ModuleLifecycle)> = vec![
            ("llm.minor", self.minor_llm.as_ref()),
            ("llm.major", self.major_llm.as_ref()),
        ];
        if let Some(llm) = &self.memory_capture_llm {
            modules.push(("llm.memory_capture", llm.as_ref()));
        }
        if let Some(llm) = &self.sentence_expression_llm {
            modules.push(("llm.sentence_expression", llm.as_ref()));
        }
        modules.extend([
            ("stt", self.stt.as_ref() as &dyn ModuleLifecycle),
            ("tts", self.tts.as_ref()),
            ("embedding", self.embedding.as_ref()),
            ("conversation", self.conversation.as_ref()),
            ("memory", self.memory.as_ref()),
            ("cache", self.cache.as_ref()),
            ("skills", self.skills.as_ref()),
            ("persona", self.persona.as_ref()),
            ("insights", self.insights.as_ref()),
            ("router", self.router.as_ref()),
            ("rule_engine", self.rule_engine.as_ref()),
            ("scheduler", self.scheduler.as_ref()),
            ("tools", self.tools.as_ref()),
        ]);
        aggregate_health(&modules, &self.health_gate, &self.health_cancel).await
    }

    /// A configured instance may serve multiple roles but has one lifecycle.
    pub(crate) fn unique_llms(&self) -> Vec<&Arc<dyn LlmAdapter>> {
        let mut unique: Vec<&Arc<dyn LlmAdapter>> = Vec::new();
        for llm in [
            Some(&self.minor_llm),
            Some(&self.major_llm),
            self.memory_capture_llm.as_ref(),
            self.sentence_expression_llm.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !unique.iter().any(|known| Arc::ptr_eq(known, llm)) {
                unique.push(llm);
            }
        }
        unique
    }

    /// Apply runtime settings to modules that support hot reload.
    pub async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> Vec<ModuleSettingsApplyReport> {
        let mut modules: Vec<&dyn ModuleLifecycle> = self
            .unique_llms()
            .into_iter()
            .map(|llm| llm.as_ref() as &dyn ModuleLifecycle)
            .collect();
        modules.extend([
            self.stt.as_ref() as &dyn ModuleLifecycle,
            self.tts.as_ref(),
            self.embedding.as_ref(),
            self.cache.as_ref(),
            self.skills.as_ref(),
            self.router.as_ref(),
            self.insights.as_ref(),
            self.rule_engine.as_ref(),
            self.tools.as_ref(),
        ]);

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

const HEALTH_BUDGET: Duration = Duration::from_secs(6);
const HEALTH_CONCURRENCY: usize = 8;

async fn aggregate_health(
    modules: &[(&str, &dyn ModuleLifecycle)],
    gate: &Mutex<()>,
    cancel: &CancellationToken,
) -> HashMap<String, ModuleHealth> {
    let deadline = tokio::time::Instant::now() + HEALTH_BUDGET;
    let all_degraded = |reason: &str| {
        modules
            .iter()
            .map(|(name, _)| (name.to_string(), ModuleHealth::Degraded(reason.to_string())))
            .collect()
    };
    if cancel.is_cancelled() {
        return all_degraded("server is stopping");
    }
    let Ok(_admission) = gate.try_lock() else {
        return all_degraded("health check already in progress");
    };

    let mut instances: Vec<(&dyn ModuleLifecycle, Vec<&str>)> = Vec::new();
    for (name, module) in modules {
        if let Some((_, names)) = instances.iter_mut().find(|(known, _)| {
            std::ptr::addr_eq(
                *known as *const dyn ModuleLifecycle,
                *module as *const dyn ModuleLifecycle,
            )
        }) {
            names.push(name);
        } else {
            instances.push((*module, vec![name]));
        }
    }
    // Borrowed futures stay owned by this request; timeout/drop never detaches probes.
    let mut pending = Vec::with_capacity(instances.len());
    for (module, names) in instances {
        pending.push(async move {
            let health = std::panic::AssertUnwindSafe(module.health_check())
                .catch_unwind()
                .await
                .unwrap_or_else(|_| ModuleHealth::Unhealthy("health check panicked".into()));
            (names, health)
        });
    }
    let checks = futures::stream::iter(pending).buffer_unordered(HEALTH_CONCURRENCY);
    tokio::pin!(checks);
    let mut results = HashMap::new();
    let remaining_reason = loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break "server is stopping",
            _ = tokio::time::sleep_until(deadline) => break "health check timed out after 6s",
            result = checks.next() => match result {
                Some((names, health)) => {
                    for name in names { results.insert(name.to_string(), health.clone()); }
                }
                None => return results,
            }
        }
    };
    for (name, _) in modules {
        results
            .entry(name.to_string())
            .or_insert_with(|| ModuleHealth::Degraded(remaining_reason.to_string()));
    }
    results
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
