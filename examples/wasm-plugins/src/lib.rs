use alia_plugin_api::{ProcessorRequest, ProcessorResponse};
use extism_pdk::{FnResult, Json, plugin_fn};
use serde_json::{Value, json};

#[plugin_fn]
pub fn alia_processor(Json(request): Json<ProcessorRequest>) -> FnResult<Json<ProcessorResponse>> {
    Ok(Json(ProcessorResponse {
        success: request.action == "echo",
        result: if request.action == "echo" {
            request.payload
        } else {
            json!({"error":"unsupported action"})
        },
    }))
}

#[plugin_fn]
pub fn alia_tool(Json(input): Json<Value>) -> FnResult<Json<Value>> {
    Ok(Json(input))
}

#[plugin_fn]
pub fn alia_expression(Json(request): Json<Value>) -> FnResult<Json<Value>> {
    let output = match request["type"].as_str() {
        Some("health_check") => json!({"type":"health","ok":true,"message":"Rust WASM ready"}),
        Some("appraise_user_input") => {
            let text = request["context"]["user_text"]
                .as_str()
                .unwrap_or("")
                .to_lowercase();
            let positive = ["谢谢", "good job", "thanks"]
                .iter()
                .any(|marker| text.contains(marker));
            let events = if positive {
                vec![json!({
                    "kind":"external_positive_feedback","source":"classifier","target":"dialogue",
                    "confidence":0.72,"scope":"session",
                    "deltas":{"valence":0.12,"arousal":0.0,"calmness":0.0,"engagement":0.08,
                        "fatigue":0.0,"battery_stress":0.0,"thermal_stress":0.0,"sleepiness":0.0,
                        "interaction_load":0.0,"trust":0.01,"familiarity":0.0},
                    "evidence_summary":"Positive feedback detected."
                })]
            } else {
                vec![]
            };
            json!({"type":"appraisal_events","events":events})
        }
        _ => json!({"type":"noop"}),
    };
    Ok(Json(output))
}
