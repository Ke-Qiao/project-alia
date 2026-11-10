//! Expression trace/replay helpers.
//!
//! Stitches provider events into a turn-level diagnostic view.
//! Personal affect projections belong to canonical Context storage and are not
//! reconstructed from unbound conversation-event payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

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

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExpressionTraceEventSummary {
    pub sequence: i32,
    pub event_type: String,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExpressionTrace {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    pub turn_id: Uuid,
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
        if !matches!(
            event.event_type.as_str(),
            "tts_provider_adjustments" | "sentence_expression_llm"
        ) {
            continue;
        }
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
