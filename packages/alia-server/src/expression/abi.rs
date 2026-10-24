use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::config::AffectPolicySection;
use alia_plugin_runtime::{Deployment, package::PackageManifest};

use super::affect::{
    AffectDeltas, AppraisalDecay, AppraisalEvent, AppraisalScope, AppraisalSource, AppraisalTarget,
    half_life_for_scope, ttl_for_scope,
};
use super::appraisal::AppraisalContext;
use super::types::{
    AffectSnapshot, ExpressionConstraints, ExpressionInput, ExpressionIntent, ExpressionPlan,
    ExpressionSafetyModification, ExpressiveStance,
};

pub const EXPRESSION_PLUGIN_ABI_VERSION: u32 = 3;

#[async_trait]
pub trait ExternalExpressionPlugin: Send + Sync {
    fn manifest(&self) -> &ExpressionPluginManifest;
    fn provenance_pin(&self) -> Option<String> {
        None
    }

    fn private_input_isolated(&self) -> bool {
        false
    }

    /// The deployment retains its cleanup owner if cancellation drops this future.
    async fn call(
        &self,
        request: ExpressionPluginRequest,
        cancel: CancellationToken,
    ) -> anyhow::Result<ExpressionPluginResponse>;

    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn name(&self) -> &str {
        &self.manifest().name
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpressionPluginManifest {
    pub name: String,
    pub abi_version: u32,
    #[serde(default)]
    pub request_timeout_ms: Option<u64>,
    #[serde(default)]
    pub capabilities: ExpressionPluginCapabilities,
}

/// Stored in PackageManifest.expression; package identity supplies the name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmExpressionConfig {
    pub abi_version: u32,
    #[serde(default)]
    pub capabilities: ExpressionPluginCapabilities,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpressionPluginCapabilities {
    #[serde(default)]
    pub appraisal_engine: bool,
    #[serde(default)]
    pub intent_planner: bool,
    #[serde(default)]
    pub plan_transformer: bool,
    #[serde(default)]
    pub safety_policy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalExpressionInput {
    pub response_text: String,
    pub dialogue_act: String,
    pub content_tone: String,
    pub constraints: ExpressionConstraints,
    pub voice_output: bool,
    pub hardware_control: bool,
}

impl From<&ExpressionInput> for ExternalExpressionInput {
    fn from(input: &ExpressionInput) -> Self {
        Self {
            response_text: input.response_text.clone(),
            dialogue_act: input.dialogue_act.clone(),
            content_tone: input.content_tone.clone(),
            constraints: input.constraints.clone(),
            voice_output: input.voice_output,
            hardware_control: input.hardware_control,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExpressionPluginRequest {
    AppraiseUserInput {
        context: ExternalAppraisalContext,
    },
    PlanIntent {
        input: ExternalExpressionInput,
        affect: AffectSnapshot,
        intent: ExpressionIntent,
    },
    TransformPlan {
        input: ExternalExpressionInput,
        affect: AffectSnapshot,
        stance: ExpressiveStance,
        plan: ExpressionPlan,
    },
    ApplySafetyPolicy {
        input: ExternalExpressionInput,
        affect: AffectSnapshot,
        stance: ExpressiveStance,
        plan: ExpressionPlan,
    },
    HealthCheck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExpressionPluginResponse {
    AppraisalEvents {
        events: Vec<ExternalAppraisalEventDraft>,
    },
    Intent {
        intent: ExpressionIntent,
    },
    Plan {
        plan: ExpressionPlan,
    },
    SafetyPlan {
        plan: ExpressionPlan,
        #[serde(default)]
        modifications: Vec<ExpressionSafetyModification>,
    },
    Health {
        ok: bool,
        #[serde(default)]
        message: Option<String>,
    },
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAppraisalContext {
    pub user_text: String,
    pub now: chrono::DateTime<chrono::Utc>,
}

impl From<&AppraisalContext<'_>> for ExternalAppraisalContext {
    fn from(ctx: &AppraisalContext<'_>) -> Self {
        Self {
            user_text: ctx.user_text.to_string(),
            now: ctx.now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalAppraisalEventDraft {
    pub kind: String,
    pub source: AppraisalSource,
    pub target: AppraisalTarget,
    pub confidence: f32,
    pub scope: AppraisalScope,
    pub deltas: AffectDeltas,
    pub evidence_summary: String,
    #[serde(default)]
    pub ttl_sec: Option<u64>,
    #[serde(default)]
    pub decay: Option<AppraisalDecay>,
}

pub(crate) fn external_draft_to_appraisal(
    draft: ExternalAppraisalEventDraft,
    now: chrono::DateTime<chrono::Utc>,
    policy: &AffectPolicySection,
) -> AppraisalEvent {
    let mut event = AppraisalEvent::new(
        draft.kind,
        draft.source,
        draft.target,
        draft.confidence,
        draft.scope,
        now,
        policy,
        draft.deltas,
        draft.evidence_summary,
    );
    if let Some(ttl_sec) = draft.ttl_sec {
        event.ttl_sec = ttl_sec.clamp(1, ttl_for_scope(event.scope, policy));
    }
    if let Some(decay) = draft.decay {
        event.decay = AppraisalDecay {
            kind: decay.kind,
            half_life_sec: decay
                .half_life_sec
                .clamp(1, half_life_for_scope(event.scope, policy)),
        };
    }
    event
}

pub fn ensure_supported_expression_plugin_abi(version: u32) -> anyhow::Result<()> {
    anyhow::ensure!(
        version == EXPRESSION_PLUGIN_ABI_VERSION,
        "unsupported expression plugin ABI version {version}; host supports {}",
        EXPRESSION_PLUGIN_ABI_VERSION
    );
    Ok(())
}

pub struct WasmExpressionPlugin {
    manifest: ExpressionPluginManifest,
    deployment: Arc<Deployment>,
}

impl WasmExpressionPlugin {
    pub fn from_package(
        package: &PackageManifest,
        deployment: Arc<Deployment>,
    ) -> anyhow::Result<Arc<Self>> {
        let config: WasmExpressionConfig =
            serde_json::from_value(package.expression.clone().ok_or_else(|| {
                anyhow::anyhow!("package does not declare an expression extension")
            })?)?;
        ensure_supported_expression_plugin_abi(config.abi_version)?;
        deployment.require_export("alia_expression")?;
        Ok(Arc::new(Self {
            manifest: ExpressionPluginManifest {
                name: package.name.clone(),
                abi_version: config.abi_version,
                request_timeout_ms: Some(package.limits.timeout_ms),
                capabilities: config.capabilities,
            },
            deployment,
        }))
    }
}

#[async_trait]
impl ExternalExpressionPlugin for WasmExpressionPlugin {
    fn manifest(&self) -> &ExpressionPluginManifest {
        &self.manifest
    }
    fn provenance_pin(&self) -> Option<String> {
        self.deployment
            .is_live()
            .then(|| self.deployment.implementation_pin().to_owned())
    }
    fn private_input_isolated(&self) -> bool {
        self.deployment.is_live()
    }
    async fn call(
        &self,
        request: ExpressionPluginRequest,
        cancel: CancellationToken,
    ) -> anyhow::Result<ExpressionPluginResponse> {
        let input = serde_json::to_vec(&request)?;
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(self.manifest.request_timeout_ms.unwrap_or(1000));
        let output = self
            .deployment
            .invoke("alia_expression", &input, deadline, cancel)
            .await?;
        Ok(serde_json::from_slice(&output)?)
    }
    async fn shutdown(&self) -> anyhow::Result<()> {
        self.deployment.shutdown().await
    }
}

#[cfg(test)]
mod wasm_tests {

    #[tokio::test]
    async fn real_wasm_appraisal_requires_exact_persistent_pin_and_is_revocable() {
        use crate::expression::abi::WasmExpressionPlugin;
        use crate::expression::{
            AppraisalContext, ExpressionExtensionRegistry, ExternalExpressionPlugin,
        };
        use alia_plugin_runtime::{Deployment, Limits, package::PackageManifest};
        use std::{collections::BTreeMap, sync::Arc, time::Duration};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../examples/wasm-plugins/target/wasm32-unknown-unknown/release/alia_wasm_demo.wasm",
        );
        let module = std::fs::read(path).expect("build examples/wasm-plugins with Rust 1.96 first");
        let manifest = PackageManifest {
            name: "rust-appraisal".into(),
            version: "1".into(),
            abi_version: 1,
            limits: Limits {
                timeout_ms: 10000,
                ..Limits::default()
            },
            tools: vec![],
            expression: Some(
                serde_json::json!({"abi_version":3,"capabilities":{"appraisal_engine":true}}),
            ),
        };
        let deployment = Deployment::new(
            module,
            manifest.limits.clone(),
            &serde_json::to_value(&manifest).unwrap(),
        )
        .unwrap();
        deployment.enable(deployment.implementation_pin()).unwrap();
        let plugin = WasmExpressionPlugin::from_package(&manifest, deployment.clone()).unwrap();
        assert!(plugin.private_input_isolated());
        let pin = plugin.provenance_pin().unwrap();
        let registry = ExpressionExtensionRegistry::default();
        registry
            .register_external_plugin(plugin as Arc<dyn ExternalExpressionPlugin>)
            .unwrap();
        let policy = Default::default();
        let ctx = AppraisalContext {
            channel_id: "private-channel",
            user_id: "private-user",
            user_text: "thanks",
            now: chrono::Utc::now(),
            policy: &policy,
        };
        assert!(
            registry
                .appraise_verified_user_input(&ctx, &BTreeMap::new(), Duration::from_secs(10))
                .await
                .is_err()
        );
        let approved = BTreeMap::from([("rust-appraisal".to_string(), pin)]);
        let events = registry
            .appraise_verified_user_input(&ctx, &approved, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "external_positive_feedback");
        deployment.revoke().await.unwrap();
        assert!(
            registry
                .appraise_verified_user_input(&ctx, &approved, Duration::from_secs(10))
                .await
                .is_err()
        );
        registry.shutdown().await.unwrap();
    }
}
