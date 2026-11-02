use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures::{StreamExt, stream};
use sea_orm::DbErr;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::repository::ExecutionRepository;
use super::types::*;
use crate::config::ToolsSection;
use crate::modules::platform::lifecycle::ModuleHealth;
use crate::providers::adapter::{
    PreparedTool, ToolCall, ToolContext, ToolDefinition, ToolRegistry, ToolResult,
};

const SUBMISSION_DEADLINE: Duration = Duration::from_secs(5);
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(10);
const CANCELLATION_DEADLINE: Duration = Duration::from_secs(2);
const CANCELLATION_CONCURRENCY: usize = 8;

pub struct ExecutionService {
    pub(crate) repository: ExecutionRepository,
    pub(crate) raw: Arc<dyn ToolRegistry>,
    pub(crate) policy: RwLock<ExecutionPolicy>,
    pub(crate) catalog_gate: RwLock<()>,
    pub(crate) build_digest: String,
    pub(crate) stop: CancellationToken,
    pub(crate) wake: Notify,
    cancellations: StdMutex<HashMap<Uuid, (Uuid, CancellationToken)>>,
    task: Mutex<Option<JoinHandle<anyhow::Result<()>>>>,
    pub(crate) fault: StdMutex<Option<String>>,
    pub(crate) scan_fault: StdMutex<Option<String>>,
}

#[derive(Clone)]
pub(crate) struct ExecutionPolicy {
    pub timeout: Duration,
    pub allow_plugins: bool,
    pub confirmation_tools: Vec<String>,
}

impl ExecutionPolicy {
    pub(crate) fn digest(&self) -> String {
        let mut names = self.confirmation_tools.clone();
        names.sort();
        names.dedup();
        digest(&json!({
            "timeout_ms": self.timeout.as_millis(),
            "allow_plugins": self.allow_plugins,
            "confirmation_tools": names,
        }))
    }

    fn requires_confirmation(&self, tool: &ToolDefinition) -> bool {
        tool.require_confirmation
            || self
                .confirmation_tools
                .iter()
                .any(|name| name == tool.name.as_str())
    }

    pub(crate) fn allows(&self, tool: &ToolDefinition) -> bool {
        self.allow_plugins || !crate::registry::is_plugin_source(&tool.source)
    }
}

impl ExecutionService {
    pub async fn new(
        repository: ExecutionRepository,
        raw: Arc<dyn ToolRegistry>,
        config: &ToolsSection,
        shutdown: &CancellationToken,
    ) -> anyhow::Result<Arc<Self>> {
        let build_digest = super::identity::build_digest().await?;
        Ok(Arc::new(Self {
            repository,
            raw,
            policy: RwLock::new(ExecutionPolicy {
                timeout: Duration::from_secs(config.tool_call_timeout_sec.clamp(1, 300)),
                allow_plugins: config.allow_plugin_tools,
                confirmation_tools: config.require_confirmation.clone(),
            }),
            catalog_gate: RwLock::new(()),
            build_digest,
            stop: shutdown.child_token(),
            wake: Notify::new(),
            cancellations: StdMutex::new(HashMap::new()),
            task: Mutex::new(None),
            fault: StdMutex::new(None),
            scan_fault: StdMutex::new(None),
        }))
    }

    pub async fn start(self: &Arc<Self>) -> anyhow::Result<()> {
        let mut task = self.task.lock().await;
        anyhow::ensure!(!self.stop.is_cancelled(), "execution service is stopping");
        if task.is_none() {
            // An unavailable ledger is a startup failure, not a disabled feature.
            tokio::time::timeout(Duration::from_secs(2), self.repository.recover_expired(256))
                .await??;
            let service = self.clone();
            *task = Some(tokio::spawn(
                async move { super::worker::run(service).await },
            ));
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + SHUTDOWN_DEADLINE;
        self.stop.cancel();
        // Cancel every known session immediately, including while an admission
        // is completing. A second snapshot after the barrier catches newcomers.
        self.cancel_session_tokens();
        let mut failures = Vec::new();
        let admission_pending = tokio::time::timeout_at(deadline, async {
            let _admission = self.catalog_gate.write().await;
            self.cancel_session_tokens()
        })
        .await;
        let admission_pending = match admission_pending {
            Ok(cancelled) => {
                let mut cancellations = stream::iter(cancelled)
                    .map(|(owner, id)| async move {
                        let result = tokio::time::timeout(
                            CANCELLATION_DEADLINE,
                            self.repository.cancel(owner, id),
                        )
                        .await;
                        match result {
                            Ok(Ok(_)) | Ok(Err(ExecutionError::InvalidTransition(_))) => {
                                self.forget_cancellation(id).await;
                                None
                            }
                            result => Some(format!("cancellation {id}: {result:?}")),
                        }
                    })
                    .buffer_unordered(CANCELLATION_CONCURRENCY);
                // One total budget bounds the whole batch, not 256 sequential
                // per-row deadlines. Unconfirmed entries remain retryable.
                let drained = tokio::time::timeout_at(deadline, async {
                    while let Some(error) = cancellations.next().await {
                        if let Some(error) = error {
                            failures.push(error);
                        }
                    }
                })
                .await;
                if drained.is_err() {
                    failures.push("cancellation batch deadline elapsed".into());
                }
                false
            }
            Err(_) => {
                failures.push("admission barrier deadline elapsed".into());
                true
            }
        };
        let worker_pending = match tokio::time::timeout_at(deadline, async {
            let mut task = self.task.lock().await;
            if let Some(worker) = task.as_mut() {
                // Keep the only handle stored while awaiting. Timeout or a
                // dropped waiter leaves the worker supervised and joinable.
                let result = worker.await;
                task.take();
                result
                    .map_err(anyhow::Error::from)
                    .and_then(|result| result)?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        {
            Ok(Ok(())) => false,
            Ok(Err(error)) => {
                failures.push(format!("worker: {error:#}"));
                false
            }
            Err(_) => true,
        };
        let pending = self
            .cancellations
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        if admission_pending || worker_pending || !pending.is_empty() || !failures.is_empty() {
            let fault = format!(
                "execution shutdown incomplete: admission_pending={admission_pending}, \
                 worker_pending={worker_pending}, pending_effects={pending:?}; {}",
                failures.join("; ")
            );
            *self.fault.lock().unwrap() = Some(fault.clone());
            anyhow::bail!(fault);
        }
        if let Some(fault) = self.fault.lock().unwrap().as_ref() {
            anyhow::bail!("execution shutdown reported failure: {fault}");
        }
        if let Some(fault) = self.scan_fault.lock().unwrap().as_ref() {
            anyhow::bail!("execution shutdown with unresolved ledger scan: {fault}");
        }
        Ok(())
    }

    fn cancel_session_tokens(&self) -> Vec<(Uuid, Uuid)> {
        self.cancellations
            .lock()
            .unwrap()
            .iter()
            .map(|(id, (owner, token))| {
                token.cancel();
                (*owner, *id)
            })
            .collect()
    }

    pub(crate) fn worker_pending(&self) -> bool {
        self.task.try_lock().map_or(true, |task| task.is_some())
    }

    /// Dependencies must outlive admission, receipt persistence and cancellation,
    /// even when a previous shutdown attempt has already reported an error.
    pub(crate) fn shutdown_pending(&self) -> bool {
        self.worker_pending()
            || self.catalog_gate.try_write().is_err()
            || !self.cancellations.lock().unwrap().is_empty()
    }

    pub fn health(&self) -> ModuleHealth {
        match self.fault.lock().unwrap().as_ref() {
            Some(fault) => ModuleHealth::Unhealthy(fault.clone()),
            None => match self.scan_fault.lock().unwrap().as_ref() {
                Some(fault) => ModuleHealth::Unhealthy(fault.clone()),
                None => ModuleHealth::Healthy,
            },
        }
    }

    pub async fn submit(&self, call: ToolCall, ctx: &ToolContext) -> ExecutionResult<EffectRecord> {
        tokio::time::timeout(SUBMISSION_DEADLINE, self.submit_before_deadline(call, ctx))
            .await
            .map_err(|_| {
                ExecutionError::Database(DbErr::Custom(
                    "execution admission deadline elapsed; outcome must be reconciled".into(),
                ))
            })?
    }

    async fn submit_before_deadline(
        &self,
        call: ToolCall,
        ctx: &ToolContext,
    ) -> ExecutionResult<EffectRecord> {
        if self.stop.is_cancelled()
            || ctx
                .cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(ExecutionError::InvalidTransition(
                "request cancelled before proposal".into(),
            ));
        }
        let _catalog = self.catalog_gate.read().await;
        if self.stop.is_cancelled() {
            return Err(ExecutionError::InvalidTransition(
                "execution service is stopping".into(),
            ));
        }
        let prepared = self
            .raw
            .resolve_executor(&call.tool_name)
            .await
            .map_err(|error| ExecutionError::InvalidInput(error.to_string()))?
            .ok_or_else(|| ExecutionError::InvalidInput("tool unavailable".into()))?;
        prepared
            .executor
            .authorize(ctx)
            .map_err(|e| ExecutionError::Unauthorized(e.to_string()))?;
        prepared
            .executor
            .validate(&call.arguments)
            .map_err(|e| ExecutionError::InvalidInput(e.to_string()))?;
        let definition = prepared.executor.definition();
        let policy = self.policy.read().await.clone();
        if !policy.allows(&definition) {
            return Err(ExecutionError::Unauthorized(
                "plugin tools disabled by current policy".into(),
            ));
        }
        let effect = self
            .repository
            .propose(NewEffect {
                context_id: ctx.context_id,
                key: EffectKey {
                    user_id: ctx.user_id,
                    request_id: ctx.request_id,
                    invocation_key: ctx.invocation_key.clone(),
                },
                model_call_id: call.call_id,
                source_received_at: ctx.source_received_at,
                source_event_id: ctx.source_event_id,
                source_privacy_epoch: ctx.source_privacy_epoch,
                source: ctx.source.clone(),
                conversation_id: ctx.conversation_id,
                channel_id: ctx.channel_id.clone(),
                tool_name: call.tool_name,
                arguments: call.arguments,
                tool_digest: implementation_digest(&prepared, &self.build_digest)?,
                policy_digest: policy.digest(),
                requires_confirmation: policy.requires_confirmation(&definition),
                required_role: definition.required_role,
            })
            .await?;
        // Register before any approval/cancellation await. Dropping this
        // admission (including an uncertain COMMIT acknowledgement) cancels the
        // originating session and retains ownership until durable reconciliation.
        let mut admission_guard = None;
        if !effect.requires_confirmation
            && matches!(
                effect.status,
                EffectStatus::Proposed | EffectStatus::Approved
            )
        {
            if let Some(token) = &ctx.cancel {
                let token = self
                    .cancellations
                    .lock()
                    .unwrap()
                    .entry(effect.effect_id)
                    .or_insert_with(|| (ctx.user_id, token.clone()))
                    .1
                    .clone();
                admission_guard = Some(token.drop_guard());
            }
        }
        if self.stop.is_cancelled()
            || ctx
                .cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        {
            if let Some(guard) = admission_guard.take() {
                drop(guard);
                let cancelled = self
                    .repository
                    .cancel(ctx.user_id, effect.effect_id)
                    .await?;
                self.forget_cancellation(effect.effect_id).await;
                return Ok(cancelled);
            }
            return Err(ExecutionError::InvalidTransition(
                "execution service or request is stopping".into(),
            ));
        }
        let effect = if effect.status == EffectStatus::Proposed && !effect.requires_confirmation {
            let approved = self
                .repository
                .approve(
                    ctx.user_id,
                    effect.effect_id,
                    Approval {
                        actor_id: ctx.user_id,
                        kind: ApprovalKind::Automatic,
                        expected_intent_digest: effect.intent_digest,
                    },
                )
                .await;
            match approved {
                Ok(approved) => approved,
                // An error cannot prove approval did not commit. The guard
                // cancels the session; its mapping must survive this return.
                Err(error) => return Err(error),
            }
        } else {
            effect
        };
        if let Some(guard) = admission_guard {
            guard.disarm();
        }
        self.wake.notify_one();
        Ok(effect)
    }

    /// Returns execution state separately from tool success. A bounded wait may
    /// end before the worker; the durable operation remains queryable by effect_id.
    pub async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let call_id = call.call_id.clone();
        let mut effect = match self.submit(call, ctx).await {
            Ok(effect) => effect,
            Err(error) => return Ok(error_result(call_id, &error)),
        };
        let until = tokio::time::Instant::now() + Duration::from_secs(10);
        while !effect.status.is_terminal() && effect.status != EffectStatus::Proposed {
            let cancellation = async {
                match &ctx.cancel {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            };
            let wait = async {
                tokio::select! {
                    biased;
                    _ = cancellation => {
                        // The database serializes cancellation against claim. If
                        // already dispatched, only the caller's wait ends.
                        match self.repository.cancel(ctx.user_id, effect.effect_id).await {
                            Ok(cancelled) => effect = cancelled,
                            Err(ExecutionError::InvalidTransition(_)) => {
                                effect = self.get(ctx.user_id, effect.effect_id).await?;
                            }
                            Err(error) => return Err(error.into()),
                        }
                        self.forget_cancellation(effect.effect_id).await;
                        Ok::<_, anyhow::Error>(true)
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        effect = self.get(ctx.user_id, effect.effect_id).await?;
                        Ok(false)
                    }
                }
            };
            // Bound the database awaits inside polling/cancellation too. If
            // an acknowledgement is unavailable, return the last observed
            // fact and leave cancellation ownership available to the worker.
            match tokio::time::timeout_at(until, wait).await {
                Ok(Ok(false)) => {}
                Ok(Ok(true)) | Err(_) => break,
                Ok(Err(error)) => return Err(error),
            }
        }
        let mut result = effect_result(&effect);
        result.call_id = call_id;
        Ok(result)
    }

    pub async fn get(&self, owner: Uuid, id: Uuid) -> ExecutionResult<EffectRecord> {
        self.repository
            .get(owner, id)
            .await?
            .ok_or(ExecutionError::NotFound)
    }

    pub async fn list(
        &self,
        owner: Uuid,
        before: Option<Uuid>,
        limit: u32,
        request: Option<Uuid>,
    ) -> ExecutionResult<Vec<EffectRecord>> {
        self.repository
            .list(owner, before, limit.min(100), request)
            .await
    }

    /// Called only by authenticated confirmation ingress, never by model tools.
    pub async fn confirm(
        &self,
        owner: Uuid,
        id: Uuid,
        intent_digest: String,
    ) -> ExecutionResult<EffectRecord> {
        let effect = self
            .repository
            .approve(
                owner,
                id,
                Approval {
                    actor_id: owner,
                    kind: ApprovalKind::Explicit,
                    expected_intent_digest: intent_digest,
                },
            )
            .await?;
        self.wake.notify_one();
        Ok(effect)
    }

    pub async fn cancel(&self, owner: Uuid, id: Uuid) -> ExecutionResult<EffectRecord> {
        let result = self.repository.cancel(owner, id).await?;
        self.forget_cancellation(id).await;
        Ok(result)
    }

    pub(crate) async fn cancelled_before_claim(
        &self,
        effect: &EffectRecord,
    ) -> ExecutionResult<bool> {
        let cancelled = self
            .cancellations
            .lock()
            .unwrap()
            .get(&effect.effect_id)
            .is_some_and(|(_, token)| token.is_cancelled());
        if cancelled {
            match self
                .repository
                .cancel(effect.key.user_id, effect.effect_id)
                .await
            {
                Ok(_) | Err(ExecutionError::InvalidTransition(_)) => {}
                Err(error) => return Err(error),
            }
            self.forget_cancellation(effect.effect_id).await;
        }
        Ok(cancelled)
    }

    pub(crate) async fn forget_cancellation(&self, id: Uuid) {
        self.cancellations.lock().unwrap().remove(&id);
    }

    pub(crate) async fn apply_settings(&self, updates: &BTreeMap<String, Value>) -> Vec<String> {
        let _catalog = self.catalog_gate.write().await;
        let mut policy = self.policy.write().await;
        let mut applied = Vec::new();
        let timeout_key = "core.policies.tools.tool_call_timeout_sec";
        if let Some(timeout) = updates.get(timeout_key).and_then(Value::as_u64) {
            policy.timeout = Duration::from_secs(timeout.clamp(1, 300));
            applied.push(timeout_key.into());
        }
        let plugin_key = "core.policies.tools.allow_plugin_tools";
        if let Some(allow) = updates.get(plugin_key).and_then(Value::as_bool) {
            policy.allow_plugins = allow;
            applied.push(plugin_key.into());
        }
        applied
    }
}

pub(crate) fn implementation_digest(
    prepared: &PreparedTool,
    build_digest: &str,
) -> ExecutionResult<String> {
    let tool = prepared.executor.definition();
    if prepared.registration_revision.trim().is_empty() {
        return Err(ExecutionError::InvalidInput(
            "tool registration revision is required".into(),
        ));
    }
    let revision = prepared.executor.implementation_revision();
    Ok(digest(&json!({
        "function": tool.function, "source": tool.source,
        "identity": prepared.executor.identity, "principal": prepared.executor.principal(),
        "require_confirmation": tool.require_confirmation, "required_role": tool.required_role,
        "build_digest": build_digest, "implementation_revision": revision,
        "registration_revision": prepared.registration_revision,
    })))
}

fn digest(value: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), canonical(v)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
            value => value.clone(),
        }
    }
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(value)).expect("JSON value serializes"))
    )
}

pub(crate) fn effect_result(effect: &EffectRecord) -> ToolResult {
    let (receipt, message) = match &effect.completion {
        Some(Completion::Succeeded { receipt }) => (Some(receipt.clone()), "Execution succeeded."),
        Some(Completion::Failed { evidence }) => {
            (Some(evidence.clone()), "Execution definitively failed.")
        }
        Some(Completion::Unknown { .. }) => (
            None,
            "Execution outcome is unknown. Do not repeat the action; inspect its effect record.",
        ),
        None if effect.status == EffectStatus::Proposed => (
            None,
            "Authenticated confirmation of this effect is required before execution.",
        ),
        None if effect.status == EffectStatus::Cancelled => {
            (None, "Execution was cancelled before dispatch.")
        }
        _ => (
            None,
            "Execution is pending. Do not claim completion or submit the action again.",
        ),
    };
    ToolResult {
        presentation: None,
        call_id: effect.model_call_id.clone(),
        success: effect.status == EffectStatus::Succeeded,
        output: json!({
            "effect_id": effect.effect_id, "request_id": effect.key.request_id,
            "execution_status": effect.status, "intent_digest": effect.intent_digest,
            "requires_confirmation": effect.status == EffectStatus::Proposed,
            "receipt": receipt, "message": message,
        }),
        display: Some(message.into()),
    }
}

pub(crate) fn error_result(call_id: String, error: &ExecutionError) -> ToolResult {
    let code = match error {
        ExecutionError::IdentityUnlinked => "identity_unlinked",
        ExecutionError::Unauthorized(_) => "unauthorized",
        ExecutionError::IdempotencyConflict => "idempotency_conflict",
        ExecutionError::CapacityExceeded => "capacity_exceeded",
        ExecutionError::VersionChanged => "version_changed",
        ExecutionError::InvalidTransition(_) => "invalid_transition",
        ExecutionError::InvalidInput(_) => "invalid_input",
        _ => "execution_unavailable",
    };
    ToolResult {
        presentation: None,
        call_id,
        success: false,
        output: json!({"error": code, "message": error.to_string()}),
        display: Some(error.to_string()),
    }
}
