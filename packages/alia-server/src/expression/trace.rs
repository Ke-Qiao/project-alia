//! Expression trace/replay helpers.
//!
//! The runtime records expression decisions as small conversation events. This
//! module stitches those events back into a single turn-level trace so debug
//! tools can answer "why did she say or move like that?" without knowing every
//! internal event shape.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{
    AffectSnapshot, AppraisalEvent, ExpressionInput, ExpressionPlan, ExpressionPriority,
    ExpressionSafetyClass, ExpressionSafetyModification,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionTraceEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    pub turn_id: Uuid,
    pub sequence: i32,
    pub event_type: String,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionTraceEventSummary {
    pub sequence: i32,
    pub event_type: String,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionTraceObservation {
    pub snapshot: AffectSnapshot,
    #[serde(default)]
    pub appraisals: Vec<AppraisalEvent>,
    #[serde(default)]
    pub active_event_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default)]
    pub used_tools: bool,
    #[serde(default)]
    pub response_was_fallback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionTraceIntent {
    pub intent: String,
    pub social_function: String,
    pub style: String,
    pub intensity: f32,
    pub priority: ExpressionPriority,
    pub interruptible: bool,
    pub safety_class: ExpressionSafetyClass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionTrace {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    pub turn_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_correction: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_observation: Option<ExpressionTraceObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_observation: Option<ExpressionTraceObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affect_snapshot: Option<AffectSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression_input: Option<ExpressionInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<ExpressionTraceIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<ExpressionPlan>,
    #[serde(default)]
    pub safety_modifications: Vec<ExpressionSafetyModification>,
    #[serde(default)]
    pub tts_requests: Vec<Value>,
    #[serde(default)]
    pub tts_provider_adjustments: Vec<Value>,
    #[serde(default)]
    pub sentence_expression_llm: Vec<Value>,
    #[serde(default)]
    pub source_events: Vec<ExpressionTraceEventSummary>,
}

impl ExpressionTrace {
    fn new(event: &ExpressionTraceEvent) -> Self {
        Self {
            session_id: event.session_id,
            turn_id: event.turn_id,
            stt_correction: None,
            user_observation: None,
            assistant_observation: None,
            affect_snapshot: None,
            expression_input: None,
            intent: None,
            plan: None,
            safety_modifications: Vec::new(),
            tts_requests: Vec::new(),
            tts_provider_adjustments: Vec::new(),
            sentence_expression_llm: Vec::new(),
            source_events: Vec::new(),
        }
    }
}

pub fn expression_traces_from_events(
    mut events: Vec<ExpressionTraceEvent>,
) -> Vec<ExpressionTrace> {
    events.sort_by_key(|event| (event.turn_id, event.sequence));
    let mut traces = std::collections::BTreeMap::<Uuid, ExpressionTrace>::new();

    for event in events {
        let trace = traces
            .entry(event.turn_id)
            .or_insert_with(|| ExpressionTrace::new(&event));
        trace.source_events.push(ExpressionTraceEventSummary {
            sequence: event.sequence,
            event_type: event.event_type.clone(),
            actor: event.actor.clone(),
            summary: event.summary.clone(),
            created_at: event.created_at.clone(),
        });

        match event.event_type.as_str() {
            "stt_correction" => {
                trace.stt_correction = Some(event.payload.clone());
            }
            "affect_user_input" => {
                if let Some(observation) = observation_from_payload(&event.payload) {
                    trace.affect_snapshot = Some(observation.snapshot.clone());
                    trace.user_observation = Some(observation);
                }
            }
            "affect_state" => {
                if let Some(observation) = observation_from_payload(&event.payload) {
                    trace.affect_snapshot = Some(observation.snapshot.clone());
                    trace.assistant_observation = Some(observation);
                }
            }
            "expression_plan" => {
                if let Some(input) = event
                    .payload
                    .get("input")
                    .and_then(|value| serde_json::from_value::<ExpressionInput>(value.clone()).ok())
                {
                    trace.expression_input = Some(input);
                }
                if let Some(plan) = event
                    .payload
                    .get("plan")
                    .and_then(|value| serde_json::from_value::<ExpressionPlan>(value.clone()).ok())
                {
                    trace.intent = Some(ExpressionTraceIntent::from(&plan));
                    trace.safety_modifications = plan.safety_modifications.clone();
                    trace.plan = Some(plan);
                } else if let Some(intent) = intent_from_legacy_payload(&event.payload) {
                    trace.intent = Some(intent);
                }
                if let Some(snapshot) = snapshot_from_payload(&event.payload) {
                    trace.affect_snapshot = Some(snapshot);
                }
            }
            "tts_provider_adjustments" => {
                if let Some(request) = event.payload.get("request") {
                    trace.tts_requests.push(request.clone());
                }
                if let Some(adjustments) =
                    event.payload.get("adjustments").and_then(Value::as_array)
                {
                    trace
                        .tts_provider_adjustments
                        .extend(adjustments.iter().cloned());
                }
            }
            "sentence_expression_llm" => {
                trace.sentence_expression_llm.push(event.payload.clone());
            }
            _ => {}
        }
    }

    traces.into_values().collect()
}

impl From<&ExpressionPlan> for ExpressionTraceIntent {
    fn from(plan: &ExpressionPlan) -> Self {
        Self {
            intent: plan.intent.clone(),
            social_function: plan.social_function.clone(),
            style: plan.style.clone(),
            intensity: plan.intensity,
            priority: plan.priority,
            interruptible: plan.interruptible,
            safety_class: plan.safety_class,
        }
    }
}

fn observation_from_payload(payload: &Value) -> Option<ExpressionTraceObservation> {
    let snapshot = snapshot_from_payload(payload)?;
    Some(ExpressionTraceObservation {
        snapshot,
        appraisals: payload
            .get("appraisals")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default(),
        active_event_count: payload
            .get("active_event_count")
            .and_then(Value::as_u64)
            .unwrap_or_default() as usize,
        route: payload
            .get("route")
            .and_then(Value::as_str)
            .map(str::to_string),
        used_tools: payload
            .get("used_tools")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        response_was_fallback: payload
            .get("response_was_fallback")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn snapshot_from_payload(payload: &Value) -> Option<AffectSnapshot> {
    serde_json::from_value(serde_json::json!({
        "affect": payload.get("affect")?,
        "social": payload.get("social")?,
        "resource": payload.get("resource")?,
        "dominant_causes": payload.get("dominant_causes").cloned().unwrap_or_else(|| serde_json::json!([])),
    }))
    .ok()
}

fn intent_from_legacy_payload(payload: &Value) -> Option<ExpressionTraceIntent> {
    Some(ExpressionTraceIntent {
        intent: payload.get("intent")?.as_str()?.to_string(),
        social_function: payload.get("social_function")?.as_str()?.to_string(),
        style: payload.get("style")?.as_str()?.to_string(),
        intensity: payload.get("intensity")?.as_f64()? as f32,
        priority: ExpressionPriority::Normal,
        interruptible: true,
        safety_class: ExpressionSafetyClass::LowRisk,
    })
}
