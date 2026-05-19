use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::{AffectPolicySection, ServerConfig};

use super::affect::{
    AffectDeltas, AppraisalDecay, AppraisalEvent, AppraisalScope, AppraisalSource, AppraisalTarget,
    half_life_for_scope, ttl_for_scope,
};
use super::appraisal::AppraisalContext;
use super::extension::AffectReducerContext;
use super::types::{
    AffectSnapshot, ExpressionInput, ExpressionIntent, ExpressionPlan,
    ExpressionSafetyModification, ExpressiveStance,
};

pub const EXPRESSION_PLUGIN_ABI_VERSION: u32 = 1;

#[async_trait]
pub trait ExternalExpressionPlugin: Send + Sync {
    fn manifest(&self) -> &ExpressionPluginManifest;

    async fn call(
        &self,
        request: ExpressionPluginRequest,
    ) -> anyhow::Result<ExpressionPluginResponse>;

    fn name(&self) -> &str {
        &self.manifest().name
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionPluginManifest {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_expression_plugin_abi_version")]
    pub abi_version: u32,
    #[serde(default)]
    pub request_timeout_ms: Option<u64>,
    #[serde(default)]
    pub trace_stderr: bool,
    #[serde(default = "default_expression_plugin_max_stderr_bytes")]
    pub max_stderr_bytes: usize,
    #[serde(default = "default_true")]
    pub cancel_on_timeout: bool,
    #[serde(default)]
    pub sandbox: ExpressionPluginSandbox,
    /// Command used by the dynamic dispatcher. The request is written as JSON to
    /// stdin and a single `ExpressionPluginResponse` JSON object is read from
    /// stdout.
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub capabilities: ExpressionPluginCapabilities,
}

fn default_true() -> bool {
    true
}

fn default_expression_plugin_abi_version() -> u32 {
    EXPRESSION_PLUGIN_ABI_VERSION
}

fn default_expression_plugin_max_stderr_bytes() -> usize {
    4096
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionPluginSandbox {
    /// Clear inherited process environment before launching the plugin.
    #[serde(default = "default_true")]
    pub clear_env: bool,
    /// Environment allowlist passed to the child after optional clearing.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl Default for ExpressionPluginSandbox {
    fn default() -> Self {
        Self {
            clear_env: true,
            env: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExpressionPluginCapabilities {
    #[serde(default)]
    pub appraisal_engine: bool,
    #[serde(default)]
    pub affect_reducer: bool,
    #[serde(default)]
    pub intent_planner: bool,
    #[serde(default)]
    pub plan_transformer: bool,
    #[serde(default)]
    pub safety_policy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExpressionPluginRequest {
    AppraiseUserInput {
        context: ExternalAppraisalContext,
    },
    ReduceAffect {
        context: ExternalAffectReducerContext,
        snapshot: AffectSnapshot,
    },
    PlanIntent {
        input: ExpressionInput,
        affect: AffectSnapshot,
        intent: ExpressionIntent,
    },
    TransformPlan {
        input: ExpressionInput,
        affect: AffectSnapshot,
        stance: ExpressiveStance,
        plan: ExpressionPlan,
    },
    ApplySafetyPolicy {
        input: ExpressionInput,
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
    AffectSnapshot {
        snapshot: AffectSnapshot,
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
pub struct ExternalAppraisalContext {
    pub channel_id: String,
    pub user_id: String,
    pub user_text: String,
    pub now: chrono::DateTime<chrono::Utc>,
}

impl From<&AppraisalContext<'_>> for ExternalAppraisalContext {
    fn from(ctx: &AppraisalContext<'_>) -> Self {
        Self {
            channel_id: ctx.channel_id.to_string(),
            user_id: ctx.user_id.to_string(),
            user_text: ctx.user_text.to_string(),
            now: ctx.now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalAffectReducerContext {
    pub channel_id: String,
    pub user_id: String,
    pub route: Option<String>,
}

impl From<&AffectReducerContext<'_>> for ExternalAffectReducerContext {
    fn from(ctx: &AffectReducerContext<'_>) -> Self {
        Self {
            channel_id: ctx.channel_id.to_string(),
            user_id: ctx.user_id.to_string(),
            route: ctx.route.map(str::to_string),
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

pub fn load_external_expression_plugins(
    config: &ServerConfig,
) -> anyhow::Result<Vec<Arc<dyn ExternalExpressionPlugin>>> {
    let Some(dir) = &config.resources.external_expression_plugins_dir else {
        return Ok(Vec::new());
    };
    let dir = config.resolve_path(dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut plugins = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        let content = std::fs::read_to_string(&path)?;
        let mut manifest: ExpressionPluginManifest = toml::from_str(&content)?;
        if !manifest.enabled {
            tracing::info!(
                plugin_manifest = %path.display(),
                plugin = manifest.name,
                "external expression plugin manifest disabled"
            );
            continue;
        }
        ensure_supported_expression_plugin_abi(manifest.abi_version)
            .map_err(|error| anyhow::anyhow!("{error} in {}", path.display()))?;
        if manifest.working_dir.is_none() {
            manifest.working_dir = path.parent().map(Path::to_path_buf);
        } else if let Some(working_dir) = &manifest.working_dir
            && working_dir.is_relative()
        {
            manifest.working_dir = path.parent().map(|parent| parent.join(working_dir));
        }
        plugins.push(Arc::new(JsonLineExternalExpressionPlugin { manifest })
            as Arc<dyn ExternalExpressionPlugin>);
    }
    Ok(plugins)
}

pub fn ensure_supported_expression_plugin_abi(version: u32) -> anyhow::Result<()> {
    anyhow::ensure!(
        version == EXPRESSION_PLUGIN_ABI_VERSION,
        "unsupported expression plugin ABI version {version}; host supports {}",
        EXPRESSION_PLUGIN_ABI_VERSION
    );
    Ok(())
}

#[derive(Debug)]
pub struct JsonLineExternalExpressionPlugin {
    manifest: ExpressionPluginManifest,
}

#[async_trait]
impl ExternalExpressionPlugin for JsonLineExternalExpressionPlugin {
    fn manifest(&self) -> &ExpressionPluginManifest {
        &self.manifest
    }

    async fn call(
        &self,
        request: ExpressionPluginRequest,
    ) -> anyhow::Result<ExpressionPluginResponse> {
        let (program, args) = self
            .manifest
            .command
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("external expression plugin command is empty"))?;
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(self.manifest.cancel_on_timeout);
        if self.manifest.sandbox.clear_env {
            command.env_clear();
        }
        for (key, value) in &self.manifest.sandbox.env {
            command.env(key, value);
        }
        if let Some(working_dir) = &self.manifest.working_dir {
            command.current_dir(working_dir);
        }
        let mut child = command.spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("external expression plugin stdin unavailable"))?;
        stdin.write_all(&serde_json::to_vec(&request)?).await?;
        stdin.shutdown().await?;
        drop(stdin);

        let output = child.wait_with_output().await?;
        let stderr = stderr_preview(&output.stderr, self.manifest.max_stderr_bytes);
        if self.manifest.trace_stderr && !stderr.is_empty() {
            tracing::warn!(
                plugin = self.name(),
                stderr = %stderr,
                "external expression plugin wrote stderr"
            );
        }
        if !output.status.success() {
            anyhow::bail!(
                "external expression plugin '{}' exited with {}: {}",
                self.name(),
                output.status,
                stderr
            );
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    }
}

fn stderr_preview(stderr: &[u8], max_bytes: usize) -> String {
    if stderr.is_empty() || max_bytes == 0 {
        return String::new();
    }
    let mut text = String::from_utf8_lossy(stderr).to_string();
    if text.len() > max_bytes {
        text.truncate(max_bytes);
        text.push_str("...");
    }
    text
}
