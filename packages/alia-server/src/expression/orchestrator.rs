use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::channels::adapter::{ChannelAction, ChannelAdapter, ChannelTarget};

use super::types::{
    ExpressionActionReceipt, ExpressionActionStatus, ExpressionCapabilitySnapshot,
    ExpressionExecutionResult, ExpressionPlan, ExpressionPriority, ExpressionTimelineAction,
    SkippedExpressionAction,
};

#[derive(Debug, Clone)]
struct ActiveExpression {
    plan_id: String,
    priority: ExpressionPriority,
    interruptible: bool,
}

static ORCHESTRATION_STATE: OnceLock<Mutex<BTreeMap<String, ActiveExpression>>> = OnceLock::new();

fn orchestration_state() -> &'static Mutex<BTreeMap<String, ActiveExpression>> {
    ORCHESTRATION_STATE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

pub async fn negotiate_expression_capabilities(
    channel: &dyn ChannelAdapter,
) -> ExpressionCapabilitySnapshot {
    if !channel.capabilities().hardware_control {
        return ExpressionCapabilitySnapshot::unsupported("hardware_control_disabled");
    }

    match channel
        .execute_action(ChannelAction {
            name: "expression_capabilities".to_string(),
            params: serde_json::json!({}),
        })
        .await
    {
        Ok(value) => serde_json::from_value(value).unwrap_or_else(|error| {
            tracing::warn!(
                channel_type = channel.channel_type(),
                channel_id = channel.channel_id(),
                error = %error,
                "channel returned invalid expression capabilities"
            );
            ExpressionCapabilitySnapshot::unsupported("invalid_capability_snapshot")
        }),
        Err(error) => {
            tracing::debug!(
                channel_type = channel.channel_type(),
                channel_id = channel.channel_id(),
                error = %error,
                "channel does not expose expression capabilities"
            );
            ExpressionCapabilitySnapshot::unsupported("capability_negotiation_failed")
        }
    }
}

pub async fn execute_expression_plan(
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    plan: &ExpressionPlan,
) -> ExpressionExecutionResult {
    execute_expression_plan_phase(channel, target, plan, ExpressionExecutionPhase::Full).await
}

pub async fn cancel_expression_execution(
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    reason: &str,
) -> ExpressionExecutionResult {
    if !channel.capabilities().hardware_control {
        return ExpressionExecutionResult::skipped("channel has no expression execution to cancel");
    }
    clear_active_expression(channel_key(channel, target));

    match channel
        .execute_action(ChannelAction {
            name: "cancel_expression".to_string(),
            params: serde_json::json!({ "reason": reason }),
        })
        .await
    {
        Ok(output) => {
            serde_json::from_value::<ExpressionExecutionResult>(output).unwrap_or_else(|_| {
                ExpressionExecutionResult {
                    status: "completed".to_string(),
                    command_id: None,
                    completion_acknowledged: true,
                    action_receipts: vec![ExpressionActionReceipt {
                        action: "cancel_expression".to_string(),
                        status: ExpressionActionStatus::Completed,
                        reason: None,
                    }],
                    executed_actions: vec!["cancel_expression".to_string()],
                    skipped_actions: Vec::new(),
                    safety_modifications: Vec::new(),
                }
            })
        }
        Err(error) => ExpressionExecutionResult {
            status: "skipped".to_string(),
            command_id: None,
            completion_acknowledged: true,
            action_receipts: vec![ExpressionActionReceipt {
                action: "cancel_expression".to_string(),
                status: ExpressionActionStatus::Failed,
                reason: Some(error.to_string()),
            }],
            executed_actions: Vec::new(),
            skipped_actions: vec![SkippedExpressionAction {
                action: "cancel_expression".to_string(),
                reason: error.to_string(),
            }],
            safety_modifications: Vec::new(),
        },
    }
}

pub async fn execute_expression_plan_phase(
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    plan: &ExpressionPlan,
    phase: ExpressionExecutionPhase,
) -> ExpressionExecutionResult {
    if plan.timeline.is_empty() || !channel.capabilities().hardware_control {
        return ExpressionExecutionResult::skipped("channel has no executable expression timeline");
    }

    let key = channel_key(channel, target);
    if let Some(blocked) = begin_orchestrated_execution(channel, target, plan, phase).await {
        return blocked;
    }

    let capabilities = negotiate_expression_capabilities(channel).await;
    if capabilities.current_state.busy {
        if matches!(
            phase,
            ExpressionExecutionPhase::Full | ExpressionExecutionPhase::PostSpeech
        ) {
            clear_active_expression(key);
        }
        return phase_result(
            "blocked",
            Vec::new(),
            plan.timeline
                .iter()
                .filter(|action| phase.action_matches(action))
                .map(|action| skipped(action, "expression adapter is busy"))
                .collect(),
            plan,
        );
    }

    let mut executable_actions = plan
        .timeline
        .iter()
        .filter(|action| phase.action_matches(action))
        .cloned()
        .collect::<Vec<_>>();
    executable_actions.sort_by_key(|action| timeline_order(&action.at));
    executable_actions = coalesce_actions(executable_actions);

    if executable_actions.is_empty() {
        if matches!(
            phase,
            ExpressionExecutionPhase::Full | ExpressionExecutionPhase::PostSpeech
        ) {
            clear_active_expression(key);
        }
        return ExpressionExecutionResult::skipped(format!(
            "no expression actions for {} phase",
            phase.label()
        ));
    }

    let mut executable = Vec::new();
    let mut skipped_actions = Vec::new();
    for action in executable_actions {
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
        if matches!(
            phase,
            ExpressionExecutionPhase::Full | ExpressionExecutionPhase::PostSpeech
        ) {
            clear_active_expression(key);
        }
        return phase_result("skipped", Vec::new(), skipped_actions, plan);
    }

    let mut phase_plan = plan.clone();
    phase_plan.timeline = executable;
    let params = match serde_json::to_value(&phase_plan) {
        Ok(value) => value,
        Err(error) => {
            return ExpressionExecutionResult {
                status: "blocked".to_string(),
                command_id: None,
                completion_acknowledged: true,
                action_receipts: vec![ExpressionActionReceipt {
                    action: "execute_expression".to_string(),
                    status: ExpressionActionStatus::Failed,
                    reason: Some(format!("failed to serialize expression plan: {error}")),
                }],
                executed_actions: Vec::new(),
                skipped_actions: vec![SkippedExpressionAction {
                    action: "execute_expression".to_string(),
                    reason: format!("failed to serialize expression plan: {error}"),
                }],
                safety_modifications: Vec::new(),
            };
        }
    };

    let result = match channel
        .execute_action(ChannelAction {
            name: "execute_expression".to_string(),
            params,
        })
        .await
    {
        Ok(output) => merge_channel_result(output, skipped_actions, plan),
        Err(error) => {
            let mut skipped_actions = skipped_actions;
            skipped_actions.extend(
                phase_plan
                    .timeline
                    .iter()
                    .map(|action| skipped(action, error.to_string())),
            );
            phase_result("partial", Vec::new(), skipped_actions, plan)
        }
    };

    if matches!(
        phase,
        ExpressionExecutionPhase::Full | ExpressionExecutionPhase::PostSpeech
    ) {
        clear_active_expression(key);
    }
    result
}

async fn begin_orchestrated_execution(
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    plan: &ExpressionPlan,
    phase: ExpressionExecutionPhase,
) -> Option<ExpressionExecutionResult> {
    let key = channel_key(channel, target);
    let mut should_cancel_active = false;
    {
        let mut state = orchestration_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.get(&key) {
            Some(active) if active.plan_id == plan.id.to_string() => return None,
            Some(active) if !active.interruptible => {
                return Some(phase_result(
                    "blocked",
                    Vec::new(),
                    plan.timeline
                        .iter()
                        .filter(|action| phase.action_matches(action))
                        .map(|action| {
                            skipped(action, "non-interruptible expression is already active")
                        })
                        .collect(),
                    plan,
                ));
            }
            Some(active) if priority_rank(plan.priority) <= priority_rank(active.priority) => {
                return Some(phase_result(
                    "blocked",
                    Vec::new(),
                    plan.timeline
                        .iter()
                        .filter(|action| phase.action_matches(action))
                        .map(|action| skipped(action, "expression orchestrator is busy"))
                        .collect(),
                    plan,
                ));
            }
            Some(_) => {
                should_cancel_active = true;
                state.insert(
                    key.clone(),
                    ActiveExpression {
                        plan_id: plan.id.to_string(),
                        priority: plan.priority,
                        interruptible: plan.interruptible,
                    },
                );
            }
            None => {
                state.insert(
                    key,
                    ActiveExpression {
                        plan_id: plan.id.to_string(),
                        priority: plan.priority,
                        interruptible: plan.interruptible,
                    },
                );
            }
        }
    }

    if should_cancel_active {
        let _ = channel
            .execute_action(ChannelAction {
                name: "cancel_expression".to_string(),
                params: serde_json::json!({ "reason": "preempted_by_higher_priority_expression" }),
            })
            .await;
    }
    None
}

fn clear_active_expression(key: String) {
    orchestration_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&key);
}

fn channel_key(channel: &dyn ChannelAdapter, target: &ChannelTarget) -> String {
    format!("{}:{}", channel.channel_id(), target.user_id)
}

fn priority_rank(priority: ExpressionPriority) -> u8 {
    match priority {
        ExpressionPriority::Low => 0,
        ExpressionPriority::Normal => 1,
        ExpressionPriority::High => 2,
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

fn merge_channel_result(
    output: serde_json::Value,
    mut skipped_actions: Vec<SkippedExpressionAction>,
    plan: &ExpressionPlan,
) -> ExpressionExecutionResult {
    match serde_json::from_value::<ExpressionExecutionResult>(output) {
        Ok(mut result) => {
            result.skipped_actions.append(&mut skipped_actions);
            if result.safety_modifications.is_empty() {
                result.safety_modifications = safety_modifications(plan);
            }
            if result.action_receipts.is_empty() {
                result.action_receipts =
                    receipts_from_legacy_result(&result.executed_actions, &result.skipped_actions);
            }
            result
        }
        Err(_) => phase_result(
            "completed",
            plan.timeline
                .iter()
                .map(|action| action.action.clone())
                .collect(),
            skipped_actions,
            plan,
        ),
    }
}

fn phase_result(
    status: impl Into<String>,
    executed_actions: Vec<String>,
    skipped_actions: Vec<SkippedExpressionAction>,
    plan: &ExpressionPlan,
) -> ExpressionExecutionResult {
    ExpressionExecutionResult {
        status: status.into(),
        command_id: None,
        completion_acknowledged: true,
        action_receipts: receipts_from_legacy_result(&executed_actions, &skipped_actions),
        executed_actions,
        skipped_actions,
        safety_modifications: safety_modifications(plan),
    }
}

fn receipts_from_legacy_result(
    executed_actions: &[String],
    skipped_actions: &[SkippedExpressionAction],
) -> Vec<ExpressionActionReceipt> {
    let mut receipts = executed_actions
        .iter()
        .map(|action| ExpressionActionReceipt {
            action: action.clone(),
            status: ExpressionActionStatus::Completed,
            reason: None,
        })
        .collect::<Vec<_>>();
    receipts.extend(
        skipped_actions
            .iter()
            .map(|skipped| ExpressionActionReceipt {
                action: skipped.action.clone(),
                status: ExpressionActionStatus::Failed,
                reason: Some(skipped.reason.clone()),
            }),
    );
    receipts
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
    use std::sync::Mutex;

    use async_trait::async_trait;
    use uuid::Uuid;

    use crate::channels::adapter::{ChannelCapabilities, ChannelTarget};
    use crate::expression::{
        ExpressionPriority, ExpressionSafetyClass, ExpressiveStance, FallbackPolicy,
    };
    use crate::module::ModuleLifecycle;
    use crate::module::lifecycle::ModuleHealth;

    use super::*;

    struct MockExpressionChannel {
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl MockExpressionChannel {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ModuleLifecycle for MockExpressionChannel {
        fn name(&self) -> &'static str {
            "mock_expression_channel"
        }

        async fn health_check(&self) -> ModuleHealth {
            ModuleHealth::Healthy
        }
    }

    #[async_trait]
    impl ChannelAdapter for MockExpressionChannel {
        fn channel_type(&self) -> &'static str {
            "mock"
        }

        fn channel_id(&self) -> &str {
            "mock"
        }

        fn capabilities(&self) -> ChannelCapabilities {
            ChannelCapabilities {
                hardware_control: true,
                ..ChannelCapabilities::default()
            }
        }

        async fn send_message(&self, _target: &ChannelTarget, _msg: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
            match action.name.as_str() {
                "expression_capabilities" => Ok(serde_json::json!({
                    "adapter": "mock_expression",
                    "capabilities": {
                        "small_nod": { "supported": true },
                        "small_bow": { "supported": false }
                    },
                    "current_state": { "busy": false }
                })),
                "execute_expression" => {
                    let actions = action
                        .params
                        .get("timeline")
                        .and_then(serde_json::Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|item| {
                            item.get("action")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        })
                        .collect::<Vec<_>>();
                    self.calls.lock().unwrap().push(actions.clone());
                    Ok(serde_json::json!({
                        "status": "completed",
                        "executed_actions": actions,
                        "skipped_actions": [],
                        "safety_modifications": []
                    }))
                }
                "cancel_expression" => {
                    self.calls
                        .lock()
                        .unwrap()
                        .push(vec!["cancel_expression".to_string()]);
                    Ok(serde_json::json!({
                        "status": "completed",
                        "executed_actions": ["cancel_expression"],
                        "skipped_actions": [],
                        "safety_modifications": []
                    }))
                }
                _ => anyhow::bail!("unsupported action"),
            }
        }
    }

    #[tokio::test]
    async fn phase_execution_filters_by_timeline_and_capability() {
        let channel = MockExpressionChannel::new();
        let target = ChannelTarget {
            voice: None,
            user_id: "user".to_string(),
        };
        clear_active_expression(channel_key(&channel, &target));
        let mut plan = expression_plan();
        plan.timeline = vec![
            timeline("pre_speech", "small_bow"),
            timeline("speech_start", "small_nod"),
            timeline("post_speech", "small_nod"),
        ];

        let result = execute_expression_plan_phase(
            &channel,
            &target,
            &plan,
            ExpressionExecutionPhase::SpeechStart,
        )
        .await;

        assert_eq!(result.status, "completed");
        assert_eq!(result.executed_actions, vec!["small_nod".to_string()]);
        assert!(result.skipped_actions.is_empty());
        assert_eq!(
            *channel.calls.lock().unwrap(),
            vec![vec!["small_nod".to_string()]]
        );
        clear_active_expression(channel_key(&channel, &target));
    }

    #[tokio::test]
    async fn orchestrator_blocks_lower_priority_while_expression_is_active() {
        let channel = MockExpressionChannel::new();
        let target = ChannelTarget {
            voice: None,
            user_id: "busy_user".to_string(),
        };
        clear_active_expression(channel_key(&channel, &target));
        let mut active = expression_plan();
        active.priority = ExpressionPriority::Normal;
        active.timeline = vec![timeline("speech_start", "small_nod")];

        let first = execute_expression_plan_phase(
            &channel,
            &target,
            &active,
            ExpressionExecutionPhase::SpeechStart,
        )
        .await;
        assert_eq!(first.status, "completed");

        let mut lower = expression_plan();
        lower.priority = ExpressionPriority::Low;
        lower.timeline = vec![timeline("speech_start", "small_nod")];
        let blocked = execute_expression_plan_phase(
            &channel,
            &target,
            &lower,
            ExpressionExecutionPhase::SpeechStart,
        )
        .await;

        assert_eq!(blocked.status, "blocked");
        assert_eq!(
            blocked.skipped_actions[0].reason,
            "expression orchestrator is busy"
        );
        clear_active_expression(channel_key(&channel, &target));
    }

    #[tokio::test]
    async fn orchestrator_preempts_interruptible_lower_priority_expression() {
        let channel = MockExpressionChannel::new();
        let target = ChannelTarget {
            voice: None,
            user_id: "preempt_user".to_string(),
        };
        clear_active_expression(channel_key(&channel, &target));
        let mut active = expression_plan();
        active.priority = ExpressionPriority::Low;
        active.interruptible = true;
        active.timeline = vec![timeline("speech_start", "small_nod")];
        execute_expression_plan_phase(
            &channel,
            &target,
            &active,
            ExpressionExecutionPhase::SpeechStart,
        )
        .await;

        let mut urgent = expression_plan();
        urgent.priority = ExpressionPriority::High;
        urgent.timeline = vec![timeline("speech_start", "small_nod")];
        let result = execute_expression_plan_phase(
            &channel,
            &target,
            &urgent,
            ExpressionExecutionPhase::SpeechStart,
        )
        .await;

        assert_eq!(result.status, "completed");
        assert!(
            channel
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|actions| actions == &vec!["cancel_expression".to_string()])
        );
        clear_active_expression(channel_key(&channel, &target));
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
