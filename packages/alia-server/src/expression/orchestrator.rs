use crate::modules::delivery::transport::DeliveryTarget;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alia_core::delivery::VoiceScope;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use super::types::{
    ExpressionActionReceipt, ExpressionActionStatus, ExpressionCapabilitySnapshot,
    ExpressionExecutionResult, ExpressionPlan, ExpressionPriority, ExpressionTimelineAction,
    SkippedExpressionAction,
};
use crate::channels::adapter::{ChannelAction, ChannelAdapter};

/// The adapter must bind this capability to typed, original command evidence.
pub const EXPRESSION_COMMAND_PROTOCOL: &str = "expression-command-v1";
const OPERATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
struct ActiveExpression {
    plan_id: Uuid,
    plan: serde_json::Value,
    scope: Option<VoiceScope>,
    response_id: Uuid,
    priority: ExpressionPriority,
    interruptible: bool,
    command_id: Option<String>,
    phase: ExpressionExecutionPhase,
    expected_actions: Vec<String>,
    command_params: serde_json::Value,
    results: BTreeMap<ExpressionExecutionPhase, ExpressionExecutionResult>,
    closed: bool,
}

impl ActiveExpression {
    fn same_owner(&self, other: &Self) -> bool {
        self.plan_id == other.plan_id
            && self.scope == other.scope
            && self.response_id == other.response_id
            && self.command_id == other.command_id
    }
}

#[derive(Debug)]
struct ChannelState {
    operation: AsyncMutex<()>,
    active: Mutex<Option<ActiveExpression>>,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            operation: AsyncMutex::new(()),
            active: Mutex::new(None),
        }
    }
}

/// Owned by one ExpressionRuntime. No process-global state or detached workers.
#[derive(Debug, Default)]
pub struct ExpressionOrchestrator {
    channels: Mutex<BTreeMap<(String, String), Arc<ChannelState>>>,
}

impl ExpressionOrchestrator {
    fn channel(&self, channel: &dyn ChannelAdapter, target: &DeliveryTarget) -> Arc<ChannelState> {
        self.channels
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry((channel.channel_id().to_owned(), target.user_id.clone()))
            .or_default()
            .clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExpressionExecutionPhase {
    Full,
    PreSpeech,
    SpeechStart,
    PostSpeech,
}

impl ExpressionExecutionPhase {
    fn action_matches(self, action: &ExpressionTimelineAction) -> bool {
        match self {
            Self::Full => true,
            Self::PreSpeech => action.at == "pre_speech",
            Self::SpeechStart => {
                action.at == "speech_start" || action.at.starts_with("speech_phrase:")
            }
            Self::PostSpeech => action.at == "post_speech",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::PreSpeech => "pre_speech",
            Self::SpeechStart => "speech_start",
            Self::PostSpeech => "post_speech",
        }
    }
    fn closes_plan(self) -> bool {
        matches!(self, Self::Full | Self::PostSpeech)
    }
}

pub async fn negotiate_expression_capabilities(
    channel: &dyn ChannelAdapter,
) -> ExpressionCapabilitySnapshot {
    if !channel.capabilities().hardware_control {
        return ExpressionCapabilitySnapshot::unsupported("hardware_control_disabled");
    }
    match tokio::time::timeout(OPERATION_TIMEOUT, channel.execute_action(ChannelAction {
        name: "expression_capabilities".into(), params: serde_json::json!({}),
    })).await {
        Ok(Ok(value)) => serde_json::from_value(value).unwrap_or_else(|error| {
            tracing::warn!(channel_id=channel.channel_id(), error=%error, "invalid expression capability snapshot");
            ExpressionCapabilitySnapshot::unsupported("invalid_capability_snapshot")
        }),
        _ => ExpressionCapabilitySnapshot::unsupported("capability_negotiation_failed"),
    }
}

pub async fn cancel_expression_execution(
    orchestrator: &ExpressionOrchestrator,
    channel: &dyn ChannelAdapter,
    target: &DeliveryTarget,
    reason: &str,
) -> ExpressionExecutionResult {
    let state = orchestrator.channel(channel, target);
    // Capture before waiting. A delayed cancellation never adopts a new plan.
    let original = state
        .active
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let Some(original) = original.filter(|active| !active.closed && active.scope == target.voice)
    else {
        return ExpressionExecutionResult::skipped("no expression owned by this scope");
    };
    let Ok(_operation) = tokio::time::timeout(OPERATION_TIMEOUT, state.operation.lock()).await
    else {
        return unknown(
            original.command_id.clone(),
            original.scope,
            "cancel_expression",
            "expression operation wait timed out",
        );
    };
    let current = state
        .active
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if !current
        .as_ref()
        .is_some_and(|active| !active.closed && active.same_owner(&original))
    {
        return ExpressionExecutionResult::skipped("original expression owner has already changed");
    }
    let result = cancel_owned(channel, &original, &target.user_id, reason).await;
    if original.command_id.is_none() || result.writer_retired() {
        if let Some(active) = state
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            && active.same_owner(&original)
        {
            active.closed = true;
            active.command_id = None;
            active.results.clear();
        }
    }
    result
}

async fn cancel_owned(
    channel: &dyn ChannelAdapter,
    active: &ActiveExpression,
    user: &str,
    reason: &str,
) -> ExpressionExecutionResult {
    let Some(command_id) = active.command_id.as_deref() else {
        return ExpressionExecutionResult::skipped(
            "no outstanding body command; local plan retired",
        );
    };
    let capabilities = negotiate_expression_capabilities(channel).await;
    if capabilities.command_protocol.as_deref() != Some(EXPRESSION_COMMAND_PROTOCOL) {
        return unknown(
            Some(command_id.to_owned()),
            active.scope,
            "cancel_expression",
            "adapter does not support command-bound cancellation",
        );
    }
    let output = tokio::time::timeout(OPERATION_TIMEOUT, channel.execute_action(ChannelAction {
        name: "cancel_expression".into(),
        params: serde_json::json!({"command_id": command_id, "voice_scope": active.scope, "target_user_id": user, "plan_id": active.plan_id, "reason": reason, "body_intent": active.command_params}),
    })).await;
    match output {
        Ok(Ok(output)) => validate_result(
            output,
            command_id,
            active.scope,
            &["cancel_expression".into()],
            active.plan_id,
            user,
        ),
        Ok(Err(error)) => unknown(
            Some(command_id.into()),
            active.scope,
            "cancel_expression",
            error.to_string(),
        ),
        Err(_) => unknown(
            Some(command_id.into()),
            active.scope,
            "cancel_expression",
            "cancellation acknowledgement timed out",
        ),
    }
}

/// Body dispatch is attached to the same canonical Response as speech/text
/// output. Missing source authority cannot be replaced by a local identity.
pub async fn execute_expression_plan_phase_with_response(
    orchestrator: &ExpressionOrchestrator,
    channel: &dyn ChannelAdapter,
    target: &DeliveryTarget,
    plan: &ExpressionPlan,
    phase: ExpressionExecutionPhase,
    response_id: Option<Uuid>,
) -> ExpressionExecutionResult {
    let Some(response_id) = response_id.filter(|id| !id.is_nil()) else {
        return ExpressionExecutionResult::skipped("expression requires a canonical Response");
    };
    if plan.timeline.is_empty() || !channel.capabilities().hardware_control {
        return ExpressionExecutionResult::skipped("channel has no executable expression timeline");
    }
    if plan.id.is_nil() || target.voice.is_some_and(|scope| !scope.is_valid()) {
        return ExpressionExecutionResult::skipped("invalid plan identity or original voice scope");
    }
    let serialized_plan = match serde_json::to_value(plan) {
        Ok(value) => value,
        Err(error) => {
            return ExpressionExecutionResult::skipped(format!("invalid expression plan: {error}"));
        }
    };
    let state = orchestrator.channel(channel, target);
    let Ok(_operation) = tokio::time::timeout(OPERATION_TIMEOUT, state.operation.lock()).await
    else {
        return ExpressionExecutionResult::skipped("expression operation is busy");
    };
    let capabilities = negotiate_expression_capabilities(channel).await;
    if capabilities.command_protocol.as_deref() != Some(EXPRESSION_COMMAND_PROTOCOL) {
        return ExpressionExecutionResult::skipped(
            "adapter has no command-bound expression protocol",
        );
    }
    refresh_observation(&state, channel, target).await;
    let original = state
        .active
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if let Some(active) = &original {
        if active.plan_id == plan.id
            && (active.scope != target.voice
                || active.plan != serialized_plan
                || active.response_id != response_id)
        {
            return ExpressionExecutionResult::skipped(
                "expression plan identity was reused with different intent or scope",
            );
        }
        if active.plan_id == plan.id && active.scope == target.voice {
            if let Some(result) = active.results.get(&phase) {
                return result.clone();
            }
            if active.closed {
                return ExpressionExecutionResult::skipped("expression plan has already closed");
            }
            if active.command_id.is_some() {
                return ExpressionExecutionResult::skipped(
                    "prior body command has no terminal acknowledgement",
                );
            }
        } else if !active.closed {
            if active.scope != target.voice {
                return ExpressionExecutionResult::skipped(
                    "another original scope owns the expression plan",
                );
            }
            if !active.interruptible
                || priority_rank(plan.priority) <= priority_rank(active.priority)
            {
                return ExpressionExecutionResult::skipped("expression orchestrator is busy");
            }
            let cancellation = cancel_owned(
                channel,
                active,
                &target.user_id,
                "preempted_by_higher_priority_expression",
            )
            .await;
            if active.command_id.is_some() && !cancellation.writer_retired() {
                return cancellation;
            }
        }
    }
    if capabilities.current_state.busy {
        return ExpressionExecutionResult::skipped("expression adapter is busy");
    }
    let mut actions = plan
        .timeline
        .iter()
        .filter(|action| phase.action_matches(action))
        .cloned()
        .collect::<Vec<_>>();
    actions.sort_by_key(|action| timeline_order(&action.at));
    let mut executable = Vec::new();
    let mut skipped_actions = Vec::new();
    for action in coalesce_actions(actions) {
        if capabilities.supports(&action) {
            executable.push(action);
        } else {
            skipped_actions.push(skipped(
                &action,
                "action unsupported by negotiated capabilities",
            ));
        }
    }
    if executable.is_empty() {
        if phase.closes_plan() {
            if let Some(active) = state
                .active
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_mut()
                && active.plan_id == plan.id
                && active.scope == target.voice
                && active.command_id.is_none()
            {
                active.closed = true;
            }
        }
        let mut result =
            ExpressionExecutionResult::skipped("no supported actions for expression phase");
        result.skipped_actions.extend(skipped_actions);
        return result;
    }
    let mut phase_plan = plan.clone();
    phase_plan.timeline = executable;
    let mut params = match serde_json::to_value(&phase_plan) {
        Ok(value) => value,
        Err(error) => {
            return ExpressionExecutionResult::skipped(format!("invalid expression plan: {error}"));
        }
    };
    // The plan UUID and phase are immutable. The adapter must bind repeated IDs
    // to identical command payloads; this local layer does not retry commands.
    let command_id = format!("{}:{}:{}", response_id, plan.id, phase.label());
    params["command_id"] = serde_json::json!(command_id);
    params["voice_scope"] = serde_json::json!(target.voice);
    params["target_user_id"] = serde_json::json!(target.user_id);
    params["phase"] = serde_json::json!(phase.label());
    params["response_id"] = serde_json::json!(response_id);
    {
        let mut guard = state
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !guard
            .as_ref()
            .is_some_and(|active| active.plan_id == plan.id && active.scope == target.voice)
        {
            *guard = Some(ActiveExpression {
                plan_id: plan.id,
                plan: serialized_plan,
                scope: target.voice,
                response_id,
                priority: plan.priority,
                interruptible: plan.interruptible,
                command_id: None,
                phase,
                expected_actions: Vec::new(),
                command_params: serde_json::Value::Null,
                results: BTreeMap::new(),
                closed: false,
            });
        }
        guard.as_mut().unwrap().command_id = Some(command_id.clone());
        guard.as_mut().unwrap().phase = phase;
        guard.as_mut().unwrap().command_params = params.clone();
        guard.as_mut().unwrap().expected_actions = phase_plan
            .timeline
            .iter()
            .map(|action| action.action.clone())
            .collect();
    }
    let expected = phase_plan
        .timeline
        .iter()
        .map(|action| action.action.clone())
        .collect::<Vec<_>>();
    let output = tokio::time::timeout(
        OPERATION_TIMEOUT,
        channel.execute_action(ChannelAction {
            name: "execute_expression".into(),
            params,
        }),
    )
    .await;
    let mut result = match output {
        Ok(Ok(output)) => validate_result(
            output,
            &command_id,
            target.voice,
            &expected,
            plan.id,
            &target.user_id,
        ),
        Ok(Err(error)) => unknown(
            Some(command_id.clone()),
            target.voice,
            "execute_expression",
            error.to_string(),
        ),
        Err(_) => unknown(
            Some(command_id.clone()),
            target.voice,
            "execute_expression",
            "command acknowledgement timed out",
        ),
    };
    result.skipped_actions.extend(skipped_actions);
    result.safety_modifications = safety_modifications(plan);
    {
        let mut guard = state
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(active) = guard.as_mut()
            && active.plan_id == plan.id
            && active.scope == target.voice
            && active.command_id.as_deref() == Some(command_id.as_str())
        {
            let terminal = result.writer_retired();
            if terminal {
                active.command_id = None;
                active.closed = phase.closes_plan();
            }
            active.results.insert(phase, result.clone());
        }
    }
    result
}

async fn refresh_observation(
    state: &ChannelState,
    channel: &dyn ChannelAdapter,
    target: &DeliveryTarget,
) {
    let original = state
        .active
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let Some(original) = original.filter(|active| active.command_id.is_some() && !active.closed)
    else {
        return;
    };
    let command_id = original.command_id.as_ref().unwrap();
    let output = tokio::time::timeout(OPERATION_TIMEOUT, channel.execute_action(ChannelAction {
        name: "observe_expression".into(), params: serde_json::json!({
            "command_id":command_id,"voice_scope":original.scope,"target_user_id":target.user_id,"plan_id":original.plan_id,"body_intent":original.command_params
        }),
    })).await;
    let Ok(Ok(output)) = output else {
        return;
    };
    let result = validate_result(
        output,
        command_id,
        original.scope,
        &original.expected_actions,
        original.plan_id,
        &target.user_id,
    );
    let mut guard = state
        .active
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(active) = guard.as_mut()
        && active.same_owner(&original)
    {
        if result.writer_retired() {
            active.command_id = None;
            active.closed = original.phase.closes_plan();
        }
        active.results.insert(original.phase, result);
    }
}

fn priority_rank(priority: ExpressionPriority) -> u8 {
    match priority {
        ExpressionPriority::Low => 0,
        ExpressionPriority::Normal => 1,
        ExpressionPriority::High => 2,
    }
}

/// Treat channel JSON as observations, not permission to synthesize completion.
fn validate_result(
    output: serde_json::Value,
    command_id: &str,
    scope: Option<VoiceScope>,
    expected: &[String],
    plan_id: Uuid,
    user_id: &str,
) -> ExpressionExecutionResult {
    let invalid = |reason: &str| {
        unknown(
            Some(command_id.to_owned()),
            scope,
            "execute_expression",
            reason,
        )
    };
    let result = match serde_json::from_value::<ExpressionExecutionResult>(output) {
        Ok(result) => result,
        Err(_) => return invalid("invalid expression acknowledgement JSON"),
    };
    if result.command_id.as_deref() != Some(command_id) || result.voice_scope != scope {
        return invalid(
            "expression acknowledgement identity does not match original command and scope",
        );
    }
    if result.action_receipts.len() != expected.len()
        || result
            .action_receipts
            .iter()
            .zip(expected)
            .any(|(receipt, action)| &receipt.action != action)
    {
        return invalid("missing or mismatched typed action receipts");
    }
    if let Some(body) = &result.body {
        if body.validate().is_err()
            || body.command.source_command_id != command_id
            || body.command.scope.voice() != scope
            || body.command.plan_id != plan_id
            || Uuid::parse_str(user_id).ok() != Some(body.command.owner_id)
        {
            return invalid("invalid original body command evidence");
        }
        let cancellation = expected == ["cancel_expression"];
        if !cancellation && body.command.actions.len() != expected.len() {
            return invalid("body command action count differs from original plan");
        }
        let derived = ExpressionExecutionResult::from_body(body.clone(), expected, cancellation);
        if result.status != derived.status
            || result
                .action_receipts
                .iter()
                .zip(&derived.action_receipts)
                .any(|(a, b)| a.status != b.status)
        {
            return invalid(
                "body submission was misrepresented as execution or position completion",
            );
        }
        return result;
    }
    // Admission can fail before the durable command receives an identity. That
    // result remains Unknown and can never retire the original writer.
    if result.status == "unknown"
        && result
            .action_receipts
            .iter()
            .all(|receipt| receipt.status == ExpressionActionStatus::Unknown)
    {
        result
    } else {
        invalid("expression acknowledgement has no original body command evidence")
    }
}

fn unknown(
    command_id: Option<String>,
    voice_scope: Option<VoiceScope>,
    action: &str,
    reason: impl Into<String>,
) -> ExpressionExecutionResult {
    let reason = reason.into();
    ExpressionExecutionResult {
        status: "unknown".into(),
        command_id,
        voice_scope,
        body: None,
        action_receipts: vec![ExpressionActionReceipt {
            action: action.into(),
            status: ExpressionActionStatus::Unknown,
            reason: Some(reason),
        }],
        skipped_actions: Vec::new(),
        safety_modifications: Vec::new(),
    }
}

fn coalesce_actions(actions: Vec<ExpressionTimelineAction>) -> Vec<ExpressionTimelineAction> {
    let mut merged: BTreeMap<(String, String, String), ExpressionTimelineAction> = BTreeMap::new();
    for action in actions {
        let key = (
            action.at.clone(),
            action.channel.clone(),
            action.action.clone(),
        );
        merged
            .entry(key)
            .and_modify(|existing| {
                if action.intensity > existing.intensity {
                    existing.intensity = action.intensity;
                }
                if action.duration_ms > existing.duration_ms {
                    existing.duration_ms = action.duration_ms;
                }
                merge_json_objects(&mut existing.params, &action.params);
            })
            .or_insert(action);
    }
    merged.into_values().collect()
}

fn merge_json_objects(base: &mut serde_json::Value, update: &serde_json::Value) {
    let (Some(base), Some(update)) = (base.as_object_mut(), update.as_object()) else {
        return;
    };
    for (key, value) in update {
        base.insert(key.clone(), value.clone());
    }
}

fn safety_modifications(plan: &ExpressionPlan) -> Vec<serde_json::Value> {
    plan.safety_modifications
        .iter()
        .filter_map(|modification| serde_json::to_value(modification).ok())
        .collect()
}

fn skipped(
    action: &ExpressionTimelineAction,
    reason: impl Into<String>,
) -> SkippedExpressionAction {
    SkippedExpressionAction {
        action: action.action.clone(),
        reason: reason.into(),
    }
}

fn timeline_order(at: &str) -> u8 {
    match at {
        "pre_speech" => 0,
        "speech_start" => 1,
        "post_speech" => 3,
        _ if at.starts_with("speech_phrase:") => 2,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::adapter::ChannelCapabilities;
    use crate::expression::{
        ExpressionRuntime, ExpressionSafetyClass, ExpressiveStance, FallbackPolicy,
    };
    use crate::modules::platform::lifecycle::ModuleLifecycle;
    use alia_core::body::{
        BodyAction, BodyBackend, BodyCommand, BodyCommandIdentity, BodyControlLease, BodyEnd,
        BodyMotion, BodyPriority, BodyProgress, BodyScope, BodySubmissionBoundary,
    };
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Semaphore;

    type Mutate = fn(Value) -> Value;
    struct MockChannel {
        protocol: bool,
        calls: Mutex<Vec<ChannelAction>>,
        commands: Mutex<BTreeMap<String, BodyCommand>>,
        observations: Mutex<BTreeMap<String, Value>>,
        mutate: Mutex<Mutate>,
        cancel_mutate: Mutex<Mutate>,
        block_next: AtomicBool,
        entered: Semaphore,
        release: Semaphore,
        fail_cancel: AtomicBool,
    }
    impl MockChannel {
        fn new() -> Self {
            Self {
                protocol: true,
                calls: Mutex::new(Vec::new()),
                commands: Mutex::new(BTreeMap::new()),
                observations: Mutex::new(BTreeMap::new()),
                mutate: Mutex::new(|v| v),
                cancel_mutate: Mutex::new(|v| v),
                block_next: AtomicBool::new(false),
                entered: Semaphore::new(0),
                release: Semaphore::new(0),
                fail_cancel: AtomicBool::new(false),
            }
        }
        fn count(&self, name: &str) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.name == name)
                .count()
        }
    }
    #[async_trait]
    impl ModuleLifecycle for MockChannel {
        fn name(&self) -> &'static str {
            "expression_test"
        }
    }
    #[async_trait]
    impl crate::modules::delivery::transport::DeliveryTransport for MockChannel {
        fn channel_id(&self) -> &str {
            "same-channel"
        }
    }

    #[async_trait::async_trait]
    impl ChannelAdapter for MockChannel {
        fn channel_type(&self) -> &'static str {
            "test"
        }
        fn capabilities(&self) -> ChannelCapabilities {
            ChannelCapabilities {
                hardware_control: true,
                ..Default::default()
            }
        }
        async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<Value> {
            if action.name == "expression_capabilities" {
                return Ok(
                    json!({ "adapter": "test", "command_protocol": self.protocol.then_some(EXPRESSION_COMMAND_PROTOCOL),
                    "capabilities": {"small_nod": {"supported":true}, "small_bow": {"supported":false}}, "current_state":{"busy":false} }),
                );
            }
            self.calls.lock().unwrap().push(action.clone());
            let source_id = action.params["command_id"].as_str().unwrap().to_owned();
            if action.name == "observe_expression" {
                return self
                    .observations
                    .lock()
                    .unwrap()
                    .get(&source_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("no command observation yet"));
            }
            if action.name == "cancel_expression" && self.fail_cancel.load(Ordering::SeqCst) {
                anyhow::bail!("cancel transport failed");
            }
            let command = if action.name == "cancel_expression" {
                self.commands.lock().unwrap()[&source_id].clone()
            } else {
                let plan: ExpressionPlan = serde_json::from_value(action.params.clone()).unwrap();
                let scope: VoiceScope =
                    serde_json::from_value(action.params["voice_scope"].clone()).unwrap();
                let mut commands = self.commands.lock().unwrap();
                let command = BodyCommand {
                    identity: BodyCommandIdentity {
                        lease: BodyControlLease {
                            device_boot_id: scope.device_boot_id,
                            server_instance_id: Uuid::from_u128(1),
                            generation: 1,
                        },
                        sequence: commands.len() as u64 + 1,
                    },
                    plan_id: plan.id,
                    source_command_id: source_id.clone(),
                    owner_id: Uuid::parse_str(action.params["target_user_id"].as_str().unwrap())
                        .unwrap(),
                    scope: BodyScope::Voice { scope },
                    priority: match plan.priority {
                        ExpressionPriority::Low => BodyPriority::Low,
                        ExpressionPriority::Normal => BodyPriority::Normal,
                        ExpressionPriority::High => BodyPriority::High,
                    },
                    interruptible: plan.interruptible,
                    actions: plan
                        .timeline
                        .iter()
                        .map(|action| BodyAction {
                            duration_ms: action.duration_ms,
                            intensity: action.intensity,
                            motion: match action.action.as_str() {
                                "small_nod" => BodyMotion::SmallNod,
                                "small_bow" => BodyMotion::SmallBow,
                                _ => panic!("unsupported mock action"),
                            },
                        })
                        .collect(),
                };
                commands.insert(source_id.clone(), command.clone());
                command
            };
            if action.name == "execute_expression" && self.block_next.swap(false, Ordering::SeqCst)
            {
                self.entered.add_permits(1);
                self.release.acquire().await.unwrap().forget();
            }
            let cancellation = action.name == "cancel_expression";
            let names = if cancellation {
                vec!["cancel_expression".to_owned()]
            } else {
                action.params["timeline"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item["action"].as_str().unwrap().to_owned())
                    .collect()
            };
            let count = command.actions.len() as u32;
            let progress = BodyProgress {
                identity: command.identity,
                receipt_sequence: 1,
                accepted_actions: count,
                started_actions: if cancellation { 0 } else { count },
                submitted_actions: if cancellation { 0 } else { count },
                submitted_frames: if cancellation { 0 } else { count.into() },
                partial_frame: None,
                last_partial_channels: Vec::new(),
                last_uncertain_channels: Vec::new(),
                backend: Some(BodyBackend {
                    instance_id: Uuid::from_u128(2),
                    boundary: BodySubmissionBoundary::Simulated,
                    channels: 3,
                }),
                writer_quiescent: true,
                position_confirmed_actions: None,
                end: Some(if cancellation {
                    BodyEnd::Cancelled {
                        reason: "test cancel".into(),
                    }
                } else {
                    BodyEnd::Submitted
                }),
            };
            progress.validate(None, &command).unwrap();
            let result = serde_json::to_value(ExpressionExecutionResult::from_body(
                super::super::types::BodyExecutionEvidence {
                    command,
                    progress: Some(progress),
                    unknown_reason: None,
                },
                &names,
                cancellation,
            ))?;
            let mutate = if cancellation {
                *self.cancel_mutate.lock().unwrap()
            } else {
                *self.mutate.lock().unwrap()
            };
            let result = mutate(result);
            if !cancellation {
                self.observations
                    .lock()
                    .unwrap()
                    .insert(source_id, result.clone());
            }
            Ok(result)
        }
    }
    fn target() -> DeliveryTarget {
        DeliveryTarget {
            user_id: Uuid::from_u128(3).to_string(),
            voice: Some(VoiceScope {
                device_boot_id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                generation_epoch: 1,
            }),
        }
    }
    fn accepted(mut value: Value) -> Value {
        value["status"] = json!("accepted");
        let progress = &mut value["body"]["progress"];
        progress["started_actions"] = json!(0);
        progress["submitted_actions"] = json!(0);
        progress["submitted_frames"] = json!(0);
        progress["writer_quiescent"] = json!(false);
        progress["end"] = Value::Null;
        for receipt in value["action_receipts"].as_array_mut().unwrap() {
            receipt["status"] = json!("accepted");
        }
        value
    }
    fn active_plan(priority: ExpressionPriority) -> ExpressionPlan {
        let mut plan = expression_plan();
        plan.priority = priority;
        plan.timeline = vec![timeline("speech_start", "small_nod")];
        plan
    }
    async fn phase(
        o: &ExpressionOrchestrator,
        c: &MockChannel,
        t: &DeliveryTarget,
        p: &ExpressionPlan,
    ) -> ExpressionExecutionResult {
        execute_expression_plan_phase_with_response(
            o,
            c,
            t,
            p,
            ExpressionExecutionPhase::SpeechStart,
            Some(p.id),
        )
        .await
    }

    #[test]
    fn missing_body_evidence_and_local_skip_cannot_retire_a_writer() {
        let result: ExpressionExecutionResult =
            serde_json::from_value(json!({"status":"accepted"})).unwrap();
        assert!(!result.writer_retired());
        let skipped = ExpressionExecutionResult::skipped("unsupported");
        assert!(!skipped.writer_retired());
        assert_eq!(
            skipped.action_receipts[0].status,
            ExpressionActionStatus::Skipped
        );
    }

    #[tokio::test]
    async fn invalid_or_legacy_receipts_never_retire_the_original_writer() {
        let cases: Vec<(&str, Mutate)> = vec![
            ("empty object", |_| json!({})),
            ("non-object", |_| json!(false)),
            ("old completion flag", |mut v| {
                v["completion_acknowledged"] = json!(true);
                v
            }),
            ("no command id", |mut v| {
                v.as_object_mut().unwrap().remove("command_id");
                v
            }),
            ("random command id", |mut v| {
                v["command_id"] = json!("device-random-id");
                v
            }),
            ("wrong scope", |mut v| {
                v["voice_scope"]["generation_epoch"] = json!(99);
                v
            }),
            ("no action receipts", |mut v| {
                v.as_object_mut().unwrap().remove("action_receipts");
                v
            }),
            ("no body evidence", |mut v| {
                v.as_object_mut().unwrap().remove("body");
                v
            }),
            ("accepted contradicts submitted facts", |mut v| {
                v["status"] = json!("accepted");
                v
            }),
            ("wrong receipt status type", |mut v| {
                v["action_receipts"][0]["status"] = json!(true);
                v
            }),
            ("wrong receipt action", |mut v| {
                v["action_receipts"][0]["action"] = json!("other");
                v
            }),
            ("old executed actions", |mut v| {
                v["executed_actions"] = json!(["other"]);
                v
            }),
            ("accepted with no typed receipts", |v| {
                let mut v = accepted(v);
                v["action_receipts"] = json!([]);
                v
            }),
            ("forged physical completion", |mut v| {
                v["body"]["progress"]["position_confirmed_actions"] = json!(1);
                v
            }),
            ("old completed status", |mut v| {
                v["status"] = json!("completed");
                v["action_receipts"][0]["status"] = json!("completed");
                v
            }),
        ];
        for (name, mutate) in cases {
            let orchestrator = ExpressionOrchestrator::default();
            let channel = MockChannel::new();
            *channel.mutate.lock().unwrap() = mutate;
            let result = phase(
                &orchestrator,
                &channel,
                &target(),
                &active_plan(ExpressionPriority::Normal),
            )
            .await;
            assert_eq!(result.status, "unknown", "{name}");
            assert!(!result.writer_retired(), "{name}");
        }
    }

    #[tokio::test]
    async fn missing_canonical_response_never_reaches_the_adapter() {
        let orchestrator = ExpressionOrchestrator::default();
        let channel = MockChannel::new();
        for response_id in [None, Some(Uuid::nil())] {
            let result = execute_expression_plan_phase_with_response(
                &orchestrator,
                &channel,
                &target(),
                &active_plan(ExpressionPriority::Normal),
                ExpressionExecutionPhase::Full,
                response_id,
            )
            .await;
            assert_eq!(result.status, "skipped");
            assert!(!result.writer_retired());
        }
        assert!(channel.calls.lock().unwrap().is_empty());
        assert!(orchestrator.channels.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_body_outcome_retires_only_with_original_quiescence_evidence() {
        for quiescent in [false, true] {
            let orchestrator = ExpressionOrchestrator::default();
            let channel = MockChannel::new();
            *channel.mutate.lock().unwrap() = if quiescent {
                |mut value| {
                    value["status"] = json!("unknown");
                    value["body"]["progress"]["end"] =
                        json!({"kind":"unknown", "reason":"feedback unavailable"});
                    value
                }
            } else {
                |mut value| {
                    value["status"] = json!("unknown");
                    value["body"]["progress"]["end"] =
                        json!({"kind":"unknown", "reason":"writer acknowledgement lost"});
                    value["body"]["progress"]["writer_quiescent"] = json!(false);
                    value
                }
            };
            let result = phase(
                &orchestrator,
                &channel,
                &target(),
                &active_plan(ExpressionPriority::Normal),
            )
            .await;
            assert_eq!(result.status, "unknown");
            assert_eq!(result.writer_retired(), quiescent);
            assert!(
                result
                    .body
                    .as_ref()
                    .unwrap()
                    .progress
                    .as_ref()
                    .unwrap()
                    .position_confirmed_actions
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn accepted_is_not_completion_and_cannot_start_another_phase() {
        let orchestrator = ExpressionOrchestrator::default();
        let channel = MockChannel::new();
        *channel.mutate.lock().unwrap() = accepted;
        let target = target();
        let mut plan = active_plan(ExpressionPriority::Normal);
        plan.timeline.push(timeline("post_speech", "small_nod"));
        let result = phase(&orchestrator, &channel, &target, &plan).await;
        assert_eq!(result.status, "accepted");
        assert!(!result.writer_retired());
        let next = execute_expression_plan_phase_with_response(
            &orchestrator,
            &channel,
            &target,
            &plan,
            ExpressionExecutionPhase::PostSpeech,
            Some(plan.id),
        )
        .await;
        assert_eq!(next.status, "skipped");
        assert_eq!(channel.count("execute_expression"), 1);
    }

    #[tokio::test]
    async fn unsupported_command_protocol_has_no_body_or_cancel_side_effects() {
        let orchestrator = ExpressionOrchestrator::default();
        let mut channel = MockChannel::new();
        channel.protocol = false;
        let target = target();
        let result = phase(
            &orchestrator,
            &channel,
            &target,
            &active_plan(ExpressionPriority::Normal),
        )
        .await;
        assert_eq!(result.status, "skipped");
        assert!(!result.writer_retired());
        let result =
            cancel_expression_execution(&orchestrator, &channel, &target, "interrupt").await;
        assert!(!result.writer_retired());
        assert!(channel.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn phases_filter_and_duplicate_phase_reuses_original_evidence() {
        let orchestrator = ExpressionOrchestrator::default();
        let channel = MockChannel::new();
        let target = target();
        let mut plan = active_plan(ExpressionPriority::Normal);
        plan.timeline = vec![
            timeline("pre_speech", "small_bow"),
            timeline("speech_start", "small_nod"),
            timeline("post_speech", "small_nod"),
        ];
        let first = phase(&orchestrator, &channel, &target, &plan).await;
        assert_eq!(first.status, "submitted");
        assert!(first.writer_retired());
        assert_eq!(
            first.action_receipts[0].status,
            ExpressionActionStatus::Submitted
        );
        assert_eq!(
            first
                .body
                .as_ref()
                .unwrap()
                .progress
                .as_ref()
                .unwrap()
                .position_confirmed_actions,
            None
        );
        assert!(first.skipped_actions.is_empty());
        let repeated = phase(&orchestrator, &channel, &target, &plan).await;
        assert_eq!(first.command_id, repeated.command_id);
        assert_eq!(channel.count("execute_expression"), 1);
        plan.timeline[1].intensity = 0.9;
        assert_eq!(
            phase(&orchestrator, &channel, &target, &plan).await.status,
            "skipped"
        );
        assert_eq!(channel.count("execute_expression"), 1);
    }

    #[tokio::test]
    async fn two_runtimes_and_users_have_independent_plan_owners() {
        let config: crate::config::ServerConfig =
            toml::from_str(include_str!("../../../../config/server.example.toml")).unwrap();
        let a = ExpressionRuntime::from_config(&config);
        let b = ExpressionRuntime::from_config(&config);
        let channel = MockChannel::new();
        let target = target();
        phase(
            a.orchestrator(),
            &channel,
            &target,
            &active_plan(ExpressionPriority::High),
        )
        .await;
        let low = active_plan(ExpressionPriority::Low);
        assert_eq!(
            phase(a.orchestrator(), &channel, &target, &low)
                .await
                .status,
            "skipped"
        );
        assert_eq!(
            phase(b.orchestrator(), &channel, &target, &low)
                .await
                .status,
            "submitted"
        );
        let mut other_user = target.clone();
        other_user.user_id = Uuid::from_u128(4).to_string();
        assert_eq!(
            phase(a.orchestrator(), &channel, &other_user, &low)
                .await
                .status,
            "submitted"
        );
        assert_eq!(channel.count("execute_expression"), 3);
    }

    #[tokio::test]
    async fn concurrent_same_plan_executes_once_and_old_completion_cannot_clear_new_plan() {
        let orchestrator = ExpressionOrchestrator::default();
        let channel = MockChannel::new();
        let target = target();
        channel.block_next.store(true, Ordering::SeqCst);
        let old = active_plan(ExpressionPriority::Low);
        let new = active_plan(ExpressionPriority::High);
        let first = execute_expression_plan_phase_with_response(
            &orchestrator,
            &channel,
            &target,
            &old,
            ExpressionExecutionPhase::Full,
            Some(old.id),
        );
        let rest = async {
            channel.entered.acquire().await.unwrap().forget();
            let duplicate = execute_expression_plan_phase_with_response(
                &orchestrator,
                &channel,
                &target,
                &old,
                ExpressionExecutionPhase::Full,
                Some(old.id),
            );
            let next = phase(&orchestrator, &channel, &target, &new);
            channel.release.add_permits(1);
            tokio::join!(duplicate, next)
        };
        let (first, (duplicate, next)) = tokio::join!(first, rest);
        assert_eq!(first.command_id, duplicate.command_id);
        assert!(next.writer_retired());
        assert_eq!(channel.count("execute_expression"), 2);
        assert_eq!(
            phase(
                &orchestrator,
                &channel,
                &target,
                &active_plan(ExpressionPriority::Low)
            )
            .await
            .status,
            "skipped"
        );
    }

    #[tokio::test]
    async fn cancellation_unknown_or_failed_does_not_confirm_or_preempt() {
        for (transport_failure, cancellation_output) in [
            (false, (|_| json!({})) as Mutate),
            (false, accepted as Mutate),
            (true, (|v| v) as Mutate),
        ] {
            let orchestrator = ExpressionOrchestrator::default();
            let channel = MockChannel::new();
            let target = target();
            *channel.mutate.lock().unwrap() = accepted;
            phase(
                &orchestrator,
                &channel,
                &target,
                &active_plan(ExpressionPriority::Low),
            )
            .await;
            *channel.cancel_mutate.lock().unwrap() = cancellation_output;
            channel
                .fail_cancel
                .store(transport_failure, Ordering::SeqCst);
            let cancelled =
                cancel_expression_execution(&orchestrator, &channel, &target, "old turn").await;
            assert!(matches!(cancelled.status.as_str(), "unknown" | "accepted"));
            assert!(!cancelled.writer_retired());
            let new = phase(
                &orchestrator,
                &channel,
                &target,
                &active_plan(ExpressionPriority::High),
            )
            .await;
            assert!(matches!(new.status.as_str(), "unknown" | "accepted"));
            assert_eq!(channel.count("execute_expression"), 1);
            let calls = channel.calls.lock().unwrap();
            for cancel in calls.iter().filter(|call| call.name == "cancel_expression") {
                assert_eq!(cancel.params["command_id"], calls[0].params["command_id"]);
                assert_eq!(cancel.params["voice_scope"], calls[0].params["voice_scope"]);
            }
        }
    }

    #[tokio::test]
    async fn dropped_execution_retains_exact_unconfirmed_command_until_confirmed_cancel() {
        let orchestrator = ExpressionOrchestrator::default();
        let channel = MockChannel::new();
        let target = target();
        channel.block_next.store(true, Ordering::SeqCst);
        let plan = active_plan(ExpressionPriority::Normal);
        let mut execution = Box::pin(phase(&orchestrator, &channel, &target, &plan));
        assert!(futures::poll!(&mut execution).is_pending());
        assert_eq!(channel.count("execute_expression"), 1);
        drop(execution);
        let repeated = phase(&orchestrator, &channel, &target, &plan).await;
        assert_eq!(repeated.status, "skipped");
        assert_eq!(channel.count("execute_expression"), 1);
        let cancel =
            cancel_expression_execution(&orchestrator, &channel, &target, "caller dropped").await;
        assert_eq!(cancel.status, "cancelled");
        assert!(cancel.writer_retired());
        let calls = channel.calls.lock().unwrap();
        let cancelled = calls
            .iter()
            .find(|call| call.name == "cancel_expression")
            .unwrap();
        assert_eq!(
            calls[0].params["command_id"],
            cancelled.params["command_id"]
        );
        assert_eq!(
            calls[0].params["voice_scope"],
            cancelled.params["voice_scope"]
        );
        assert!(
            orchestrator
                .channel(&channel, &target)
                .active
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .closed
        );
    }

    #[tokio::test(start_paused = true)]
    async fn command_timeout_preserves_uncertainty_and_exact_cancel_identity() {
        let orchestrator = ExpressionOrchestrator::default();
        let channel = MockChannel::new();
        let target = target();
        channel.block_next.store(true, Ordering::SeqCst);
        let result = phase(
            &orchestrator,
            &channel,
            &target,
            &active_plan(ExpressionPriority::Normal),
        )
        .await;
        assert_eq!(result.status, "unknown");
        assert!(!result.writer_retired());
        let cancel =
            cancel_expression_execution(&orchestrator, &channel, &target, "timed out").await;
        assert_eq!(cancel.status, "cancelled");
        assert!(cancel.writer_retired());
        assert_eq!(result.command_id, cancel.command_id);
        assert_eq!(channel.count("execute_expression"), 1);
        assert_eq!(channel.count("cancel_expression"), 1);
    }

    #[tokio::test]
    async fn delayed_cancel_and_old_scope_never_cancel_the_new_plan() {
        let orchestrator = ExpressionOrchestrator::default();
        let channel = MockChannel::new();
        let target = target();
        *channel.mutate.lock().unwrap() = accepted;
        phase(
            &orchestrator,
            &channel,
            &target,
            &active_plan(ExpressionPriority::Low),
        )
        .await;
        *channel.mutate.lock().unwrap() = |v| v;
        let state = orchestrator.channel(&channel, &target);
        let held = state.operation.lock().await;
        let new_plan = active_plan(ExpressionPriority::High);
        let mut new = Box::pin(phase(&orchestrator, &channel, &target, &new_plan));
        let mut late_cancel = Box::pin(cancel_expression_execution(
            &orchestrator,
            &channel,
            &target,
            "delayed cancel",
        ));
        assert!(futures::poll!(&mut new).is_pending());
        assert!(futures::poll!(&mut late_cancel).is_pending());
        drop(held);
        let (new, cancel) = tokio::join!(new, late_cancel);
        assert!(new.writer_retired());
        assert_eq!(cancel.status, "skipped");
        assert_eq!(channel.count("cancel_expression"), 1); // exact preemption of the old command only
        assert_eq!(
            state.active.lock().unwrap().as_ref().unwrap().plan_id,
            new_plan.id
        );
        let mut old_scope = target.clone();
        old_scope.voice.as_mut().unwrap().generation_epoch += 1;
        let cancel =
            cancel_expression_execution(&orchestrator, &channel, &old_scope, "wrong scope").await;
        assert_eq!(cancel.status, "skipped");
        assert_eq!(channel.count("cancel_expression"), 1);
        assert!(!state.active.lock().unwrap().as_ref().unwrap().closed);
    }
    fn expression_plan() -> ExpressionPlan {
        ExpressionPlan {
            id: Uuid::new_v4(),
            intent: "test".to_string(),
            social_function: "acknowledge".to_string(),
            style: "gentle".to_string(),
            intensity: 0.3,
            stance: ExpressiveStance::default(),
            priority: ExpressionPriority::Normal,
            interruptible: true,
            safety_class: ExpressionSafetyClass::LowRisk,
            speech: None,
            timeline: Vec::new(),
            safety_modifications: Vec::new(),
            fallback: FallbackPolicy::default(),
        }
    }

    fn timeline(at: &str, action: &str) -> ExpressionTimelineAction {
        ExpressionTimelineAction {
            at: at.to_string(),
            channel: "motion".to_string(),
            action: action.to_string(),
            intensity: 0.3,
            duration_ms: 800,
            params: serde_json::json!({}),
        }
    }
}
