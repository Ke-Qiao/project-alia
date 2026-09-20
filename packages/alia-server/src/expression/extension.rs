use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::time::timeout;

use super::abi::{
    ExpressionPluginRequest, ExpressionPluginResponse, ExternalExpressionPlugin,
    external_draft_to_appraisal,
};
use super::affect::AppraisalEvent;
use super::appraisal::{AppraisalContext, AppraisalEnginePlugin};
use super::types::{
    AffectSnapshot, ExpressionInput, ExpressionIntent, ExpressionPlan,
    ExpressionSafetyModification, ExpressiveStance,
};

/// Legacy reducer shape retained so registration can report a clear migration error.
pub struct AffectReducerContext<'a> {
    pub channel_id: &'a str,
    pub user_id: &'a str,
    pub route: Option<&'a str>,
}

/// Context passed to expression planning and safety plugins.
pub struct ExpressionPlanningContext<'a> {
    pub input: &'a ExpressionInput,
    pub affect: &'a AffectSnapshot,
    pub stance: &'a ExpressiveStance,
}

/// Legacy interface: runtime/registry registration rejects mutable reducers.
///
/// Reducers should only adjust the provided snapshot. They must not execute
/// hardware actions or call LLMs; expensive interpretation belongs before this
/// point as appraisal events.
pub trait AffectReducerPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn reduce(
        &self,
        ctx: &AffectReducerContext<'_>,
        snapshot: &mut AffectSnapshot,
    ) -> anyhow::Result<()>;
}

/// Extension point for modifying a planned expression before safety checks.
///
/// Planner plugins should keep changes semantic and device-independent: style,
/// social function, intensity, timeline intent, and fallback preferences.
pub trait ExpressionPlannerPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn plan(
        &self,
        ctx: &ExpressionPlanningContext<'_>,
        plan: &mut ExpressionPlan,
    ) -> anyhow::Result<()>;
}

/// Extension point for final policy enforcement before a plan reaches channels.
///
/// Safety plugins may clamp, remove, or downgrade timeline actions. They should
/// return trace entries explaining every mutation so UI/debug views can show why
/// a plan changed.
pub trait ExpressionSafetyPolicyPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn apply(
        &self,
        ctx: &ExpressionPlanningContext<'_>,
        plan: &mut ExpressionPlan,
    ) -> anyhow::Result<Vec<ExpressionSafetyModification>>;
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ExpressionExtensionSummary {
    pub external_plugins: Vec<String>,
    pub appraisal_engines: Vec<String>,
    pub reducers: Vec<String>,
    pub planners: Vec<String>,
    pub safety_policies: Vec<String>,
}

#[derive(Default)]
pub struct ExpressionExtensionRegistry {
    external_plugins: RwLock<BTreeMap<String, Arc<dyn ExternalExpressionPlugin>>>,
    appraisal_engines: RwLock<BTreeMap<String, Arc<dyn AppraisalEnginePlugin>>>,
    planners: RwLock<BTreeMap<String, Arc<dyn ExpressionPlannerPlugin>>>,
    safety_policies: RwLock<BTreeMap<String, Arc<dyn ExpressionSafetyPolicyPlugin>>>,
}

impl fmt::Debug for ExpressionExtensionRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExpressionExtensionRegistry")
            .field("summary", &self.summary())
            .finish()
    }
}

impl ExpressionExtensionRegistry {
    pub fn register_external_plugin(
        &self,
        plugin: Arc<dyn ExternalExpressionPlugin>,
    ) -> anyhow::Result<()> {
        let manifest = plugin.manifest();
        let name = normalized_name(&manifest.name)?;
        let mut plugins = self
            .external_plugins
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if plugins.contains_key(&name) {
            anyhow::bail!("external expression plugin '{name}' is already registered");
        }
        plugins.insert(name, plugin);
        Ok(())
    }

    pub fn unregister_external_plugin(&self, name: &str) -> bool {
        let mut plugins = self
            .external_plugins
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plugins.remove(name).is_some()
    }

    pub fn register_appraisal_engine(
        &self,
        plugin: Arc<dyn AppraisalEnginePlugin>,
    ) -> anyhow::Result<()> {
        let name = normalized_name(plugin.name())?;
        let mut engines = self
            .appraisal_engines
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if engines.contains_key(&name) {
            anyhow::bail!("appraisal engine '{name}' is already registered");
        }
        engines.insert(name, plugin);
        Ok(())
    }

    pub fn unregister_appraisal_engine(&self, name: &str) -> bool {
        let mut engines = self
            .appraisal_engines
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        engines.remove(name).is_some()
    }

    /// Legacy snapshot mutation is closed. Draft interpreters must carry an
    /// implementation pin and pass the canonical Appraisal input/write gates.
    pub fn register_reducer(&self, _plugin: Arc<dyn AffectReducerPlugin>) -> anyhow::Result<()> {
        anyhow::bail!(
            "mutable affect reducers must migrate to versioned canonical appraisal drafts"
        )
    }

    pub fn unregister_reducer(&self, _name: &str) -> bool {
        false
    }

    pub fn register_planner(&self, plugin: Arc<dyn ExpressionPlannerPlugin>) -> anyhow::Result<()> {
        let name = normalized_name(plugin.name())?;
        let mut planners = self
            .planners
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if planners.contains_key(&name) {
            anyhow::bail!("expression planner '{name}' is already registered");
        }
        planners.insert(name, plugin);
        Ok(())
    }

    pub fn unregister_planner(&self, name: &str) -> bool {
        let mut planners = self
            .planners
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        planners.remove(name).is_some()
    }

    pub fn register_safety_policy(
        &self,
        plugin: Arc<dyn ExpressionSafetyPolicyPlugin>,
    ) -> anyhow::Result<()> {
        let name = normalized_name(plugin.name())?;
        let mut safety_policies = self
            .safety_policies
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if safety_policies.contains_key(&name) {
            anyhow::bail!("expression safety policy '{name}' is already registered");
        }
        safety_policies.insert(name, plugin);
        Ok(())
    }

    pub fn unregister_safety_policy(&self, name: &str) -> bool {
        let mut safety_policies = self
            .safety_policies
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        safety_policies.remove(name).is_some()
    }

    pub fn summary(&self) -> ExpressionExtensionSummary {
        ExpressionExtensionSummary {
            external_plugins: keys(&self.external_plugins),
            appraisal_engines: keys(&self.appraisal_engines),
            reducers: Vec::new(),
            planners: keys(&self.planners),
            safety_policies: keys(&self.safety_policies),
        }
    }

    pub(crate) async fn appraise_verified_user_input(
        &self,
        ctx: &AppraisalContext<'_>,
        approved: &BTreeMap<String, String>,
        external_timeout: Duration,
    ) -> anyhow::Result<Vec<AppraisalEvent>> {
        let mut events = Vec::new();
        for engine in values(&self.appraisal_engines) {
            // This builtin is reconstructed from the persistent policy by the
            // runtime; never use the registration's possibly stale lexicons.
            if engine.name() == "text_classifier_appraisal" {
                continue;
            }
            let pin = engine.provenance_pin().ok_or_else(|| {
                anyhow::anyhow!(
                    "appraisal engine {} has no implementation pin",
                    engine.name()
                )
            })?;
            anyhow::ensure!(
                approved.get(engine.name()) == Some(&pin),
                "appraisal engine is not approved by persistent policy"
            );
            let drafts = engine.appraise_user_input(ctx).await?;
            anyhow::ensure!(
                engine.provenance_pin().as_ref() == Some(&pin),
                "appraisal engine changed during interpretation"
            );
            anyhow::ensure!(
                events.len() + drafts.len() <= 64,
                "appraisal engine exceeded event budget"
            );
            events.extend(drafts);
        }
        for plugin in values(&self.external_plugins) {
            anyhow::ensure!(
                !plugin.manifest().capabilities.affect_reducer,
                "external snapshot replacement is not a canonical reducer"
            );
            if !plugin.manifest().capabilities.appraisal_engine {
                continue;
            }
            let pin = plugin.provenance_pin().ok_or_else(|| {
                anyhow::anyhow!(
                    "external appraisal engine {} has no implementation pin",
                    plugin.name()
                )
            })?;
            anyhow::ensure!(
                approved.get(plugin.name()) == Some(&pin),
                "external appraisal engine is not approved by persistent policy"
            );
            anyhow::ensure!(
                plugin.manifest().cancel_on_timeout && !plugin.manifest().trace_stderr,
                "private appraisal requires supervised cancellation and no raw stderr trace"
            );
            let result = call_external_plugin(
                &plugin,
                ExpressionPluginRequest::AppraiseUserInput {
                    context: ctx.into(),
                },
                external_timeout,
            )
            .await?;
            anyhow::ensure!(
                plugin.provenance_pin().as_ref() == Some(&pin),
                "external appraisal engine changed during interpretation"
            );
            match result {
                ExpressionPluginResponse::AppraisalEvents { events: drafts } => {
                    anyhow::ensure!(
                        events.len() + drafts.len() <= 64,
                        "external appraisal exceeded event budget"
                    );
                    events.extend(
                        drafts
                            .into_iter()
                            .map(|draft| external_draft_to_appraisal(draft, ctx.now, ctx.policy)),
                    );
                }
                ExpressionPluginResponse::Noop => (),
                _ => anyhow::bail!("external appraisal returned an invalid response kind"),
            }
        }
        Ok(events)
    }

    pub(crate) async fn apply_intent_planners(
        &self,
        input: &ExpressionInput,
        affect: &AffectSnapshot,
        intent: &mut ExpressionIntent,
        external_timeout: Duration,
    ) {
        for plugin in values(&self.external_plugins) {
            if !plugin.manifest().capabilities.intent_planner {
                continue;
            }
            let response = call_external_plugin(
                &plugin,
                ExpressionPluginRequest::PlanIntent {
                    input: input.clone(),
                    affect: affect.clone(),
                    intent: intent.clone(),
                },
                external_timeout,
            )
            .await;
            match response {
                Ok(ExpressionPluginResponse::Intent {
                    intent: planned_intent,
                }) => *intent = planned_intent,
                Ok(ExpressionPluginResponse::Noop) => {}
                Ok(other) => tracing::warn!(
                    plugin = plugin.name(),
                    response = ?other,
                    "external intent planner returned unexpected response"
                ),
                Err(error) => tracing::warn!(
                    plugin = plugin.name(),
                    error = ?error,
                    "external intent planner failed; keeping current expression intent"
                ),
            }
        }
    }

    pub(crate) async fn apply_planners(
        &self,
        ctx: &ExpressionPlanningContext<'_>,
        plan: &mut ExpressionPlan,
        external_timeout: Duration,
    ) {
        for planner in values(&self.planners) {
            if let Err(error) = planner.plan(ctx, plan) {
                tracing::warn!(
                    plugin = planner.name(),
                    error = ?error,
                    "expression planner failed; keeping current expression plan"
                );
            }
        }
        for plugin in values(&self.external_plugins) {
            if !plugin.manifest().capabilities.plan_transformer {
                continue;
            }
            let response = call_external_plugin(
                &plugin,
                ExpressionPluginRequest::TransformPlan {
                    input: ctx.input.clone(),
                    affect: ctx.affect.clone(),
                    stance: ctx.stance.clone(),
                    plan: plan.clone(),
                },
                external_timeout,
            )
            .await;
            match response {
                Ok(ExpressionPluginResponse::Plan { plan: transformed }) => *plan = transformed,
                Ok(ExpressionPluginResponse::Noop) => {}
                Ok(other) => tracing::warn!(
                    plugin = plugin.name(),
                    response = ?other,
                    "external plan transformer returned unexpected response"
                ),
                Err(error) => tracing::warn!(
                    plugin = plugin.name(),
                    error = ?error,
                    "external plan transformer failed; keeping current expression plan"
                ),
            }
        }
    }

    pub(crate) async fn apply_safety_policies(
        &self,
        ctx: &ExpressionPlanningContext<'_>,
        plan: &mut ExpressionPlan,
        external_timeout: Duration,
    ) {
        for policy in values(&self.safety_policies) {
            match policy.apply(ctx, plan) {
                Ok(modifications) => plan.safety_modifications.extend(modifications),
                Err(error) => tracing::warn!(
                    plugin = policy.name(),
                    error = ?error,
                    "expression safety policy failed; keeping current expression plan"
                ),
            }
        }
        for plugin in values(&self.external_plugins) {
            if !plugin.manifest().capabilities.safety_policy {
                continue;
            }
            let response = call_external_plugin(
                &plugin,
                ExpressionPluginRequest::ApplySafetyPolicy {
                    input: ctx.input.clone(),
                    affect: ctx.affect.clone(),
                    stance: ctx.stance.clone(),
                    plan: plan.clone(),
                },
                external_timeout,
            )
            .await;
            match response {
                Ok(ExpressionPluginResponse::SafetyPlan {
                    plan: safe_plan,
                    modifications,
                }) => {
                    *plan = safe_plan;
                    plan.safety_modifications.extend(modifications);
                }
                Ok(ExpressionPluginResponse::Noop) => {}
                Ok(other) => tracing::warn!(
                    plugin = plugin.name(),
                    response = ?other,
                    "external safety policy returned unexpected response"
                ),
                Err(error) => tracing::warn!(
                    plugin = plugin.name(),
                    error = ?error,
                    "external safety policy failed; keeping current expression plan"
                ),
            }
        }
    }
}

fn normalized_name(name: &str) -> anyhow::Result<String> {
    let normalized = name.trim();
    if normalized.is_empty() {
        anyhow::bail!("expression extension name cannot be empty");
    }
    Ok(normalized.to_string())
}

fn keys<T: ?Sized>(lock: &RwLock<BTreeMap<String, Arc<T>>>) -> Vec<String> {
    lock.read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .keys()
        .cloned()
        .collect()
}

fn values<T: ?Sized>(lock: &RwLock<BTreeMap<String, Arc<T>>>) -> Vec<Arc<T>> {
    lock.read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .cloned()
        .collect()
}

async fn call_external_plugin(
    plugin: &Arc<dyn ExternalExpressionPlugin>,
    request: ExpressionPluginRequest,
    external_timeout: Duration,
) -> anyhow::Result<ExpressionPluginResponse> {
    let timeout_duration = plugin
        .manifest()
        .request_timeout_ms
        .map(|millis| Duration::from_millis(millis.max(1)))
        .unwrap_or(external_timeout);
    match timeout(timeout_duration, plugin.call(request)).await {
        Ok(response) => response,
        Err(_) => anyhow::bail!(
            "external expression plugin '{}' timed out after {}ms",
            plugin.name(),
            timeout_duration.as_millis()
        ),
    }
}

pub fn safety_modification(
    policy: impl Into<String>,
    path: impl Into<String>,
    reason: impl Into<String>,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
) -> ExpressionSafetyModification {
    ExpressionSafetyModification {
        policy: policy.into(),
        path: path.into(),
        reason: reason.into(),
        before,
        after,
    }
}

pub fn validate_expression_plan(plan: &ExpressionPlan) -> anyhow::Result<()> {
    anyhow::ensure!(
        (0.0..=1.0).contains(&plan.intensity),
        "expression intensity must be between 0 and 1"
    );
    for action in &plan.timeline {
        anyhow::ensure!(
            (0.0..=1.0).contains(&action.intensity),
            "timeline action '{}' intensity must be between 0 and 1",
            action.action
        );
    }
    Ok(())
}
