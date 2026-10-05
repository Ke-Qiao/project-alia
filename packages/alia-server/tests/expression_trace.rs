use alia_server::expression::{ExpressionTraceEvent, expression_traces_from_events};
use uuid::Uuid;

#[test]
fn groups_current_provider_events_in_original_turn_order() {
    let turn_id = Uuid::new_v4();
    let events = vec![
        ExpressionTraceEvent {
            session_id: None,
            turn_id,
            sequence: 2,
            event_type: "tts_provider_adjustments".into(),
            actor: "tts".into(),
            summary: Some("TTS provider expression adjustments".into()),
            payload: serde_json::json!({
                "request": {"text":"I will explain this."},
                "adjustments": [{"provider":"plain", "path":"speech.style", "reason":"provider_style_control_unavailable"}]
            }),
            created_at: None,
        },
        ExpressionTraceEvent {
            session_id: None,
            turn_id,
            sequence: 1,
            event_type: "sentence_expression_llm".into(),
            actor: "expression".into(),
            summary: Some("Sentence expression LLM applied".into()),
            payload: serde_json::json!({
                "sentence":"I will explain this.",
                "baseline":{"style":"gentle", "intensity":0.4},
                "patch":{"style":"calm", "confidence":0.8},
                "final":{"style":"calm", "intensity":0.32},
                "decision":"applied"
            }),
            created_at: None,
        },
    ];
    let traces = expression_traces_from_events(events);
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].turn_id, turn_id);
    assert_eq!(traces[0].tts_requests.len(), 1);
    assert_eq!(traces[0].tts_provider_adjustments.len(), 1);
    assert_eq!(traces[0].sentence_expression_llm.len(), 1);
    assert_eq!(traces[0].source_events[0].sequence, 1);
    assert_eq!(traces[0].source_events[1].sequence, 2);
}

#[test]
fn removed_plan_affect_and_correction_events_are_not_replayed() {
    let turn_id = Uuid::new_v4();
    let events = [
        "expression_plan",
        "affect_state",
        "affect_user_input",
        "stt_correction",
    ]
    .into_iter()
    .enumerate()
    .map(|(sequence, event_type)| ExpressionTraceEvent {
        session_id: None,
        turn_id,
        sequence: sequence as i32,
        event_type: event_type.into(),
        actor: "expression".into(),
        summary: None,
        payload: serde_json::json!({
            "intent":"unbound", "social_function":"acknowledge", "style":"gentle", "intensity":0.3,
            "plan":{"intent":"unbound"}, "input":{"response_text":"private"},
            "affect":{"valence":0.5}, "social":{"trust":0.9}, "resource":{}
        }),
        created_at: None,
    })
    .collect();
    assert!(expression_traces_from_events(events).is_empty());
}
