//! Regression cases for appraisal and expression planning.
//!
//! These are intentionally small, named fixtures that cover product-critical
//! expression boundaries: sarcasm should not look like praise, serious safety
//! turns should not become playful, and resource/safety contexts should
//! suppress unnecessary motion.

use std::collections::BTreeMap;

use super::types::{
    ExpressionActionCapability, ExpressionAdapterState, ExpressionCapabilitySnapshot,
    ExpressionDeviceContext, ExpressionInput, ExpressionPriority, ExpressionSafetyClass,
    ExpressionSafetyContext,
};

#[derive(Debug, Clone)]
pub struct AppraisalRegressionCase {
    pub name: &'static str,
    pub user_text: &'static str,
    pub expected_event_kinds: &'static [&'static str],
}

pub fn appraisal_regression_cases() -> Vec<AppraisalRegressionCase> {
    vec![
        AppraisalRegressionCase {
            name: "sarcasm_overrides_praise",
            user_text: "great job, you even got this wrong.",
            expected_event_kinds: &[
                "user_sarcasm_or_dissatisfaction",
                "relationship_repair_needed",
            ],
        },
        AppraisalRegressionCase {
            name: "direct_compliment",
            user_text: "thank you, awesome.",
            expected_event_kinds: &["user_gratitude", "user_affirmation"],
        },
        AppraisalRegressionCase {
            name: "user_frustration",
            user_text: "not this, you got it wrong again.",
            expected_event_kinds: &["user_frustration", "relationship_friction"],
        },
        AppraisalRegressionCase {
            name: "user_distress",
            user_text: "I feel sad and anxious today.",
            expected_event_kinds: &["user_support_need"],
        },
        AppraisalRegressionCase {
            name: "safety_salience",
            user_text: "Is this operation dangerous? Please give me a safety warning.",
            expected_event_kinds: &["safety_salience"],
        },
        AppraisalRegressionCase {
            name: "minimal_expression_request",
            user_text: "Please be brief, don't move, no gestures.",
            expected_event_kinds: &["user_requested_minimal_expression"],
        },
    ]
}

#[derive(Debug, Clone)]
pub struct PlannerRegressionCase {
    pub name: &'static str,
    pub input: ExpressionInput,
    pub expected_intent: &'static str,
    pub expected_social_function: &'static str,
    pub expected_style: &'static str,
    pub expected_priority: ExpressionPriority,
    pub expected_safety_class: ExpressionSafetyClass,
    pub expect_motion: bool,
    pub expected_timeline_actions: usize,
    pub expected_speech_style: &'static str,
    pub max_speech_intensity: Option<f32>,
    pub expected_safety_reason: Option<&'static str>,
}

pub fn planner_regression_cases() -> Vec<PlannerRegressionCase> {
    vec![
        PlannerRegressionCase {
            name: "serious_safety_warning",
            input: expression_input(
                "Please stop this dangerous operation immediately.",
                "warn",
                "safety",
            ),
            expected_intent: "clear_warning",
            expected_social_function: "warn",
            expected_style: "serious",
            expected_priority: ExpressionPriority::High,
            expected_safety_class: ExpressionSafetyClass::MediumRisk,
            expect_motion: false,
            expected_timeline_actions: 0,
            expected_speech_style: "serious",
            max_speech_intensity: Some(0.46),
            expected_safety_reason: None,
        },
        PlannerRegressionCase {
            name: "sadness_comfort",
            input: expression_input(
                "I am here, that really does sound painful.",
                "comfort",
                "supportive",
            ),
            expected_intent: "gentle_comfort",
            expected_social_function: "comfort",
            expected_style: "gentle",
            expected_priority: ExpressionPriority::Normal,
            expected_safety_class: ExpressionSafetyClass::LowRisk,
            expect_motion: false,
            expected_timeline_actions: 0,
            expected_speech_style: "gentle",
            max_speech_intensity: Some(0.36),
            expected_safety_reason: None,
        },
        PlannerRegressionCase {
            name: "frustration_repair",
            input: frustrated_expression_input(),
            expected_intent: "calm_supportive_response",
            expected_social_function: "apologize",
            expected_style: "gentle",
            expected_priority: ExpressionPriority::Normal,
            expected_safety_class: ExpressionSafetyClass::LowRisk,
            expect_motion: false,
            expected_timeline_actions: 0,
            expected_speech_style: "gentle",
            max_speech_intensity: Some(0.3),
            expected_safety_reason: None,
        },
        PlannerRegressionCase {
            name: "low_battery_caps_motion",
            input: low_battery_expression_input(),
            expected_intent: "warm_celebration",
            expected_social_function: "celebrate",
            expected_style: "cheerful",
            expected_priority: ExpressionPriority::Normal,
            expected_safety_class: ExpressionSafetyClass::LowRisk,
            expect_motion: true,
            expected_timeline_actions: 1,
            expected_speech_style: "cheerful",
            max_speech_intensity: None,
            expected_safety_reason: Some("battery_saver_intensity_cap"),
        },
        PlannerRegressionCase {
            name: "night_mode_suppresses_motion",
            input: night_mode_expression_input(),
            expected_intent: "minimal_clear_response",
            expected_social_function: "focus",
            expected_style: "calm",
            expected_priority: ExpressionPriority::Normal,
            expected_safety_class: ExpressionSafetyClass::LowRisk,
            expect_motion: false,
            expected_timeline_actions: 0,
            expected_speech_style: "calm",
            max_speech_intensity: Some(0.25),
            expected_safety_reason: Some("night_mode_minimal_expression"),
        },
        PlannerRegressionCase {
            name: "user_requested_minimal_expression",
            input: minimal_expression_input(),
            expected_intent: "minimal_clear_response",
            expected_social_function: "focus",
            expected_style: "calm",
            expected_priority: ExpressionPriority::Normal,
            expected_safety_class: ExpressionSafetyClass::LowRisk,
            expect_motion: false,
            expected_timeline_actions: 0,
            expected_speech_style: "calm",
            max_speech_intensity: Some(0.25),
            expected_safety_reason: None,
        },
    ]
}

fn expression_input(text: &str, dialogue_act: &str, content_tone: &str) -> ExpressionInput {
    ExpressionInput {
        response_text: text.to_string(),
        route: Some("minor_llm".to_string()),
        channel_id: "eval".to_string(),
        user_id: "eval_user".to_string(),
        dialogue_act: dialogue_act.to_string(),
        content_tone: content_tone.to_string(),
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
    }
}

fn frustrated_expression_input() -> ExpressionInput {
    let mut input = expression_input(
        "Sorry, I will realign with your question.",
        "apologize",
        "serious",
    );
    input.user_state.frustration = 0.72;
    input
}

fn low_battery_expression_input() -> ExpressionInput {
    let mut input = expression_input(
        "Great, it has been completed successfully.",
        "celebrate",
        "playful",
    );
    input.channel_id = "eval_low_battery".to_string();
    input.hardware_control = true;
    input.device_context = ExpressionDeviceContext {
        supports_motion: true,
        battery_level: Some(0.10),
        battery_saver: true,
        ..ExpressionDeviceContext::default()
    };
    input.constraints.allow_playful = true;
    input.expression_capabilities = Some(capabilities_with_action("small_nod", 0.8));
    input
}

fn night_mode_expression_input() -> ExpressionInput {
    let mut input = expression_input("Okay, I will explain briefly.", "respond", "neutral");
    input.channel_id = "eval_night".to_string();
    input.hardware_control = true;
    input.device_context.supports_motion = true;
    input.safety_context = ExpressionSafetyContext {
        night_mode: true,
        ..ExpressionSafetyContext::default()
    };
    input.expression_capabilities = Some(capabilities_with_action("small_nod", 0.8));
    input
}

fn minimal_expression_input() -> ExpressionInput {
    let mut input = expression_input("Brief answer: yes.", "respond", "concise");
    input.hardware_control = true;
    input.device_context.supports_motion = true;
    input.safety_context.minimal_expression = true;
    input.expression_capabilities = Some(capabilities_with_action("small_nod", 0.8));
    input
}

fn capabilities_with_action(action: &str, max_intensity: f32) -> ExpressionCapabilitySnapshot {
    let mut capabilities = BTreeMap::new();
    capabilities.insert(
        action.to_string(),
        ExpressionActionCapability {
            supported: true,
            max_intensity: Some(max_intensity),
            max_duration_ms: Some(900),
            max_frequency_per_min: Some(60),
            ..ExpressionActionCapability::default()
        },
    );
    ExpressionCapabilitySnapshot {
        adapter: "eval_adapter".to_string(),
        command_protocol: None,
        capabilities,
        current_state: ExpressionAdapterState {
            motion_adapter_registered: true,
            ..ExpressionAdapterState::default()
        },
    }
}
