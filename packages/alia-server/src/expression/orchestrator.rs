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
use crate::channels::adapter::{ChannelAction, ChannelAdapter, ChannelTarget};

/// This is a local adapter contract, not a claim that Embedded implements D3.
pub const EXPRESSION_COMMAND_PROTOCOL: &str = "expression-command-v1";
const OPERATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
struct ActiveExpression {
    plan_id: Uuid,
    plan: serde_json::Value,
    scope: Option<VoiceScope>,
    priority: ExpressionPriority,
    interruptible: bool,
    command_id: Option<String>,
    results: BTreeMap<ExpressionExecutionPhase, ExpressionExecutionResult>,
    closed: bool,
}

impl ActiveExpression {
    fn same_owner(&self, other: &Self) -> bool {
        self.plan_id == other.plan_id
            && self.scope == other.scope
            && self.command_id == other.command_id
    }
}

#[derive(Debug)]
struct ChannelState {
    identity: Uuid,
    operation: AsyncMutex<()>,
    active: Mutex<Option<ActiveExpression>>,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            identity: Uuid::new_v4(),
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
    fn channel(&self, channel: &dyn ChannelAdapter, target: &ChannelTarget) -> Arc<ChannelState> {
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

pub async fn execute_expression_plan(
    orchestrator: &ExpressionOrchestrator,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    plan: &ExpressionPlan,
) -> ExpressionExecutionResult {
    execute_expression_plan_phase(
        orchestrator,
        channel,
        target,
        plan,
        ExpressionExecutionPhase::Full,
    )
    .await
}

pub async fn cancel_expression_execution(
    orchestrator: &ExpressionOrchestrator,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
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
    if original.command_id.is_none() || result.completion_acknowledged {
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
        params: serde_json::json!({"command_id": command_id, "voice_scope": active.scope, "target_user_id": user, "plan_id": active.plan_id, "reason": reason}),
    })).await;
    match output {
        Ok(Ok(output)) => validate_result(
            output,
            command_id,
            active.scope,
            &["cancel_expression".into()],
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

pub async fn execute_expression_plan_phase(
    orchestrator: &ExpressionOrchestrator,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    plan: &ExpressionPlan,
    phase: ExpressionExecutionPhase,
) -> ExpressionExecutionResult {
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
    let original = state
        .active
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if let Some(active) = &original {
        if active.plan_id == plan.id
            && (active.scope != target.voice || active.plan != serialized_plan)
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
            if active.command_id.is_some() && !cancellation.completion_acknowledged {
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
    let command_id = format!("{}:{}:{}", state.identity, plan.id, phase.label());
    params["command_id"] = serde_json::json!(command_id);
    params["voice_scope"] = serde_json::json!(target.voice);
    params["target_user_id"] = serde_json::json!(target.user_id);
    params["phase"] = serde_json::json!(phase.label());
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
                priority: plan.priority,
                interruptible: plan.interruptible,
                command_id: None,
                results: BTreeMap::new(),
                closed: false,
            });
        }
        guard.as_mut().unwrap().command_id = Some(command_id.clone());
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
        Ok(Ok(output)) => validate_result(output, &command_id, target.voice, &expected),
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
    if !result.skipped_actions.is_empty() && result.status == "completed" {
        result.status = "partial".into();
        result.completion_acknowledged = false;
    }
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
            let terminal = result.action_receipts.iter().all(|receipt| {
                matches!(
                    receipt.status,
                    ExpressionActionStatus::Completed
                        | ExpressionActionStatus::Failed
                        | ExpressionActionStatus::Rejected
                        | ExpressionActionStatus::Skipped
                )
            });
            if terminal {
                active.command_id = None;
                active.closed = phase.closes_plan();
            }
            active.results.insert(phase, result.clone());
        }
    }
    result
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
    let completed = result
        .action_receipts
        .iter()
        .filter(|receipt| receipt.status == ExpressionActionStatus::Completed)
        .map(|receipt| receipt.action.clone())
        .collect::<Vec<_>>();
    let all_completed = completed.len() == expected.len() && !expected.is_empty();
    let statuses = result
        .action_receipts
        .iter()
        .map(|receipt| &receipt.status)
        .collect::<Vec<_>>();
    let consistent_status = match result.status.as_str() {
        "completed" => all_completed && result.skipped_actions.is_empty(),
        "accepted" => statuses
            .iter()
            .all(|status| **status == ExpressionActionStatus::Accepted),
        "started" => {
            statuses
                .iter()
                .any(|status| **status == ExpressionActionStatus::Started)
                && statuses.iter().all(|status| {
                    matches!(
                        status,
                        ExpressionActionStatus::Accepted
                            | ExpressionActionStatus::Started
                            | ExpressionActionStatus::Completed
                    )
                })
        }
        "failed" => statuses
            .iter()
            .all(|status| **status == ExpressionActionStatus::Failed),
        "rejected" => statuses
            .iter()
            .all(|status| **status == ExpressionActionStatus::Rejected),
        "skipped" => statuses
            .iter()
            .all(|status| **status == ExpressionActionStatus::Skipped),
        "partial" => {
            !completed.is_empty()
                && !all_completed
                && statuses.iter().all(|status| {
                    matches!(
                        status,
                        ExpressionActionStatus::Completed
                            | ExpressionActionStatus::Failed
                            | ExpressionActionStatus::Rejected
                            | ExpressionActionStatus::Skipped
                    )
                })
        }
        "unknown" => statuses.iter().any(|status| {
            matches!(
                status,
                ExpressionActionStatus::Unknown | ExpressionActionStatus::TimedOut
            )
        }),
        _ => false,
    };
    if !consistent_status
        || result.completion_acknowledged != (result.status == "completed" && all_completed)
        || (!result.executed_actions.is_empty() && result.executed_actions != completed)
    {
        return invalid("contradictory expression completion evidence");
    }
    let mut result = result;
    // Keep legacy display fields derived from typed observations, never the reverse.
    result.executed_actions = completed;
    result
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
        completion_acknowledged: false,
        action_receipts: vec![ExpressionActionReceipt {
            action: action.into(),
            status: ExpressionActionStatus::Unknown,
            reason: Some(reason),
        }],
        executed_actions: Vec::new(),
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
    use crate::module::ModuleLifecycle;
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Semaphore;

    type Mutate = fn(Value) -> Value;
    struct MockChannel {
        protocol: bool,
        calls: Mutex<Vec<ChannelAction>>,
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
    impl ChannelAdapter for MockChannel {
        fn channel_type(&self) -> &'static str {
            "test"
        }
        fn channel_id(&self) -> &str {
            "same-channel"
        }
        fn capabilities(&self) -> ChannelCapabilities {
            ChannelCapabilities {
                hardware_control: true,
                ..Default::default()
            }
        }
        async fn send_message(&self, _: &ChannelTarget, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<Value> {
            if action.name == "expression_capabilities" {
                return Ok(
                    json!({ "adapter": "test", "command_protocol": self.protocol.then_some(EXPRESSION_COMMAND_PROTOCOL),
                    "capabilities": {"small_nod": {"supported":true}, "small_bow": {"supported":false}}, "current_state":{"busy":false} }),
                );
            }
            self.calls.lock().unwrap().push(action.clone());
            if action.name == "cancel_expression" && self.fail_cancel.load(Ordering::SeqCst) {
                anyhow::bail!("cancel transport failed");
            }
            if action.name == "execute_expression" && self.block_next.swap(false, Ordering::SeqCst)
            {
                self.entered.add_permits(1);
                self.release.acquire().await.unwrap().forget();
            }
            let names = if action.name == "cancel_expression" {
                vec!["cancel_expression".to_owned()]
            } else {
                action.params["timeline"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item["action"].as_str().unwrap().to_owned())
                    .collect()
            };
            let result = json!({"status":"completed", "command_id":action.params["command_id"], "voice_scope":action.params["voice_scope"],
                "completion_acknowledged":true, "action_receipts":names.iter().map(|name| json!({"action":name,"status":"completed"})).collect::<Vec<_>>(),
                "executed_actions":names, "skipped_actions":[], "safety_modifications":[]});
            let mutate = if action.name == "cancel_expression" {
                *self.cancel_mutate.lock().unwrap()
            } else {
                *self.mutate.lock().unwrap()
            };
            Ok(mutate(result))
        }
    }
    fn target() -> ChannelTarget {
        ChannelTarget {
            user_id: "same-user".into(),
            voice: Some(VoiceScope {
                device_boot_id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                generation_epoch: 1,
            }),
        }
    }
    fn accepted(mut value: Value) -> Value {
        value["status"] = json!("accepted");
        value["completion_acknowledged"] = json!(false);
        value["executed_actions"] = json!([]);
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
        t: &ChannelTarget,
        p: &ExpressionPlan,
    ) -> ExpressionExecutionResult {
        execute_expression_plan_phase(o, c, t, p, ExpressionExecutionPhase::SpeechStart).await
    }

    #[test]
    fn completion_defaults_to_unconfirmed_and_local_skip_is_not_device_failure() {
        let result: ExpressionExecutionResult =
            serde_json::from_value(json!({"status":"accepted"})).unwrap();
        assert!(!result.completion_acknowledged);
        let skipped = ExpressionExecutionResult::skipped("unsupported");
        assert!(!skipped.completion_acknowledged);
        assert_eq!(
            skipped.action_receipts[0].status,
            ExpressionActionStatus::Skipped
        );
    }

    #[tokio::test]
    async fn invalid_or_legacy_receipts_never_become_completed() {
        let cases: Vec<(&str, Mutate)> = vec![
            ("empty object", |_| json!({})),
            ("non-object", |_| json!(false)),
            ("wrong field type", |mut v| {
                v["completion_acknowledged"] = json!("true");
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
            ("legacy only", |mut v| {
                v.as_object_mut().unwrap().remove("action_receipts");
                v
            }),
            ("completed without acknowledgement", |mut v| {
                v.as_object_mut().unwrap().remove("completion_acknowledged");
                v
            }),
            ("accepted claims completion", |mut v| {
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
            ("contradictory legacy actions", |mut v| {
                v["executed_actions"] = json!(["other"]);
                v
            }),
            ("accepted with no typed receipts", |v| {
                let mut v = accepted(v);
                v["action_receipts"] = json!([]);
                v
            }),
        ];
        for (name, mutate) in cases {
            let orchestrator = ExpressionOrchestrator::default();
            let channel = MockChannel::new();
            *channel.mutate.lock().unwrap() = mutate;
            let result = execute_expression_plan(
                &orchestrator,
                &channel,
                &target(),
                &active_plan(ExpressionPriority::Normal),
            )
            .await;
            assert_eq!(result.status, "unknown", "{name}");
            assert!(!result.completion_acknowledged, "{name}");
            assert!(result.executed_actions.is_empty(), "{name}");
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
        assert!(!result.completion_acknowledged);
        assert!(result.executed_actions.is_empty());
        let next = execute_expression_plan_phase(
            &orchestrator,
            &channel,
            &target,
            &plan,
            ExpressionExecutionPhase::PostSpeech,
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
        assert!(!result.completion_acknowledged);
        let result =
            cancel_expression_execution(&orchestrator, &channel, &target, "interrupt").await;
        assert!(!result.completion_acknowledged);
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
        assert_eq!(first.status, "completed");
        assert!(first.completion_acknowledged);
        assert_eq!(first.executed_actions, vec!["small_nod"]);
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
            "completed"
        );
        let mut other_user = target.clone();
        other_user.user_id = "other-user".into();
        assert_eq!(
            phase(a.orchestrator(), &channel, &other_user, &low)
                .await
                .status,
            "completed"
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
        let first = execute_expression_plan(&orchestrator, &channel, &target, &old);
        let rest = async {
            channel.entered.acquire().await.unwrap().forget();
            let duplicate = execute_expression_plan(&orchestrator, &channel, &target, &old);
            let next = phase(&orchestrator, &channel, &target, &new);
            channel.release.add_permits(1);
            tokio::join!(duplicate, next)
        };
        let (first, (duplicate, next)) = tokio::join!(first, rest);
        assert_eq!(first.command_id, duplicate.command_id);
        assert!(next.completion_acknowledged);
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
            assert!(!cancelled.completion_acknowledged);
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
        assert!(cancel.completion_acknowledged);
        let calls = channel.calls.lock().unwrap();
        assert_eq!(calls[0].params["command_id"], calls[1].params["command_id"]);
        assert_eq!(
            calls[0].params["voice_scope"],
            calls[1].params["voice_scope"]
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
        assert!(!result.completion_acknowledged);
        let cancel =
            cancel_expression_execution(&orchestrator, &channel, &target, "timed out").await;
        assert!(cancel.completion_acknowledged);
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
        assert!(new.completion_acknowledged);
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
