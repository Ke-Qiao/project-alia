use alia_server::expression::{
    ExpressionInput, ExpressionPlan, ExpressionPriority, ExpressionSafetyClass,
    ExpressionSafetyModification, ExpressionTraceEvent, FallbackPolicy, ProsodyControls,
    SpeechExpression, expression_traces_from_events,
};
use uuid::Uuid;

#[test]
fn replays_expression_turn_from_agent_events() {
    let turn_id = Uuid::new_v4();
    let plan = ExpressionPlan {
        id: Uuid::new_v4(),
        intent: "calm_explanation".to_string(),
        social_function: "explain".to_string(),
        style: "calm".to_string(),
        intensity: 0.34,
        stance: Default::default(),
        priority: ExpressionPriority::Normal,
        interruptible: true,
        safety_class: ExpressionSafetyClass::LowRisk,
        speech: Some(SpeechExpression {
            style: "calm".to_string(),
            intensity: 0.34,
            voice_profile: Some("default".to_string()),
            prosody: ProsodyControls::default(),
        }),
        timeline: Vec::new(),
        safety_modifications: vec![ExpressionSafetyModification {
            policy: "builtin_expression_safety".to_string(),
            path: "timeline".to_string(),
            reason: "motion_unavailable".to_string(),
            before: None,
            after: None,
        }],
        fallback: FallbackPolicy::default(),
    };
    let input = ExpressionInput {
        response_text: "I will explain this.".to_string(),
        route: Some("llm".to_string()),
        channel_id: "cli".to_string(),
        user_id: "user".to_string(),
        dialogue_act: "explain".to_string(),
        content_tone: "technical".to_string(),
        user_state: Default::default(),
        device_context: Default::default(),
        safety_context: Default::default(),
        conversation_context: Default::default(),
        constraints: Default::default(),
        memory_context: None,
        voice_output: true,
        hardware_control: false,
        tts_capabilities: None,
        expression_capabilities: None,
    };
    let events = vec![
        ExpressionTraceEvent {
            session_id: None,
            turn_id,
            sequence: 0,
            event_type: "affect_state".to_string(),
            actor: "affect".to_string(),
            summary: None,
            payload: serde_json::json!({
                "affect": {"valence": 0.1, "arousal": 0.3, "calmness": 0.7},
                "social": {"trust": 0.6, "familiarity": 0.2, "engagement": 0.5},
                "resource": {"fatigue": 0.1, "battery_stress": 0.0, "thermal_stress": 0.0, "sleepiness": 0.0, "interaction_load": 0.1},
                "dominant_causes": ["baseline"],
                "appraisals": [],
                "active_event_count": 1
            }),
            created_at: None,
        },
        ExpressionTraceEvent {
            session_id: None,
            turn_id,
            sequence: 1,
            event_type: "expression_plan".to_string(),
            actor: "expression".to_string(),
            summary: Some("calm_explanation".to_string()),
            payload: serde_json::json!({
                "input": input,
                "plan": plan,
                "affect": {"valence": 0.1, "arousal": 0.3, "calmness": 0.7},
                "social": {"trust": 0.6, "familiarity": 0.2, "engagement": 0.5},
                "resource": {"fatigue": 0.1, "battery_stress": 0.0, "thermal_stress": 0.0, "sleepiness": 0.0, "interaction_load": 0.1},
                "dominant_causes": ["baseline"]
            }),
            created_at: None,
        },
        ExpressionTraceEvent {
            session_id: None,
            turn_id,
            sequence: 2,
            event_type: "tts_provider_adjustments".to_string(),
            actor: "tts".to_string(),
            summary: Some("TTS provider expression adjustments".to_string()),
            payload: serde_json::json!({
                "request": {"text": "I will explain this."},
                "adjustments": [{"provider": "plain", "path": "speech.style", "reason": "provider_style_control_unavailable"}]
            }),
            created_at: None,
        },
    ];

    let traces = expression_traces_from_events(events);

    assert_eq!(traces.len(), 1);
    assert_eq!(
        traces[0].intent.as_ref().unwrap().intent,
        "calm_explanation"
    );
    assert_eq!(traces[0].safety_modifications.len(), 1);
    assert_eq!(traces[0].tts_provider_adjustments.len(), 1);
    assert_eq!(
        traces[0].expression_input.as_ref().unwrap().dialogue_act,
        "explain"
    );
}
