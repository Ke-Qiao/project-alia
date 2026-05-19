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
            user_text: "你真厉害啊，连这个都能答错。",
            expected_event_kinds: &[
                "user_sarcasm_or_dissatisfaction",
                "relationship_repair_needed",
            ],
        },
        AppraisalRegressionCase {
            name: "direct_compliment",
            user_text: "谢谢你，真棒。",
            expected_event_kinds: &["user_gratitude", "user_affirmation"],
        },
        AppraisalRegressionCase {
            name: "user_frustration",
            user_text: "不是这个，你又答错了。",
            expected_event_kinds: &["user_frustration", "relationship_friction"],
        },
        AppraisalRegressionCase {
            name: "user_distress",
            user_text: "我今天有点难过和焦虑。",
            expected_event_kinds: &["user_support_need"],
        },
        AppraisalRegressionCase {
            name: "safety_salience",
            user_text: "这个操作有危险吗？请给我安全警告。",
            expected_event_kinds: &["safety_salience"],
        },
        AppraisalRegressionCase {
            name: "minimal_expression_request",
            user_text: "请简洁一点，别动，不要动作。",
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
            input: expression_input("请立刻停止这个危险操作。", "warn", "safety"),
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
            input: expression_input("我在，这确实很难受。", "comfort", "supportive"),
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
        voice_output: true,
        hardware_control: false,
        expression_capabilities: None,
    }
}

fn frustrated_expression_input() -> ExpressionInput {
    let mut input = expression_input("抱歉，我来重新对齐你的问题。", "apologize", "serious");
    input.user_state.frustration = 0.72;
    input
}

fn low_battery_expression_input() -> ExpressionInput {
    let mut input = expression_input("太好了，已经成功完成。", "celebrate", "playful");
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
    let mut input = expression_input("好的，我简单说明。", "respond", "neutral");
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
    let mut input = expression_input("简洁回答：可以。", "respond", "concise");
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
        capabilities,
        current_state: ExpressionAdapterState {
            motion_adapter_registered: true,
            ..ExpressionAdapterState::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::config::ServerConfig;
    use crate::expression::{AffectUserInput, ExpressionRuntime};

    use super::*;

    #[tokio::test]
    async fn appraisal_regression_cases_hold() {
        let runtime = test_runtime();

        for case in appraisal_regression_cases() {
            let observation = runtime
                .observe_user_input(AffectUserInput {
                    channel_id: case.name,
                    user_id: "eval_user",
                    user_text: case.user_text,
                })
                .await;
            for expected in case.expected_event_kinds {
                assert!(
                    observation
                        .appraisals
                        .iter()
                        .any(|event| event.kind == *expected),
                    "case '{}' missed expected appraisal '{}'; got {:?}",
                    case.name,
                    expected,
                    observation
                        .appraisals
                        .iter()
                        .map(|event| event.kind.as_str())
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[tokio::test]
    async fn planner_regression_cases_hold() {
        let runtime = test_runtime();

        for case in planner_regression_cases() {
            let plan = runtime.plan(case.input).await;

            assert_eq!(plan.intent, case.expected_intent, "case {}", case.name);
            assert_eq!(
                plan.social_function, case.expected_social_function,
                "case {}",
                case.name
            );
            assert_eq!(plan.style, case.expected_style, "case {}", case.name);
            assert_eq!(plan.priority, case.expected_priority, "case {}", case.name);
            assert_eq!(
                plan.safety_class, case.expected_safety_class,
                "case {}",
                case.name
            );
            assert_eq!(
                plan.timeline.len(),
                case.expected_timeline_actions,
                "case {}",
                case.name
            );
            assert_eq!(
                !plan.timeline.is_empty(),
                case.expect_motion,
                "case {}",
                case.name
            );
            assert_eq!(
                plan.speech.as_ref().map(|speech| speech.style.as_str()),
                Some(case.expected_speech_style),
                "case {}",
                case.name
            );
            if let Some(max_intensity) = case.max_speech_intensity {
                assert!(
                    plan.speech
                        .as_ref()
                        .map(|speech| speech.intensity <= max_intensity)
                        .unwrap_or(false),
                    "case '{}' speech intensity exceeded {}: {:?}",
                    case.name,
                    max_intensity,
                    plan.speech
                );
            }
            if let Some(reason) = case.expected_safety_reason {
                assert!(
                    plan.safety_modifications
                        .iter()
                        .any(|modification| modification.reason == reason),
                    "case '{}' missed safety reason '{}'; got {:?}",
                    case.name,
                    reason,
                    plan.safety_modifications
                );
            }
        }
    }

    #[test]
    fn resource_governance_config_is_present() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();

        let zh = config.resources.appraisal_lexicons.get("zh").unwrap();
        assert_eq!(
            zh.metadata.source_name.as_deref(),
            Some("Project Alia zh appraisal overlay")
        );
        assert_eq!(zh.metadata.redistributable, Some(true));
        assert!(zh.metadata.overlay);

        let en = config.resources.appraisal_lexicons.get("en").unwrap();
        assert_eq!(
            en.metadata.source_name.as_deref(),
            Some("Project Alia en appraisal overlay")
        );
        assert_eq!(en.metadata.redistributable, Some(true));
        assert!(en.metadata.overlay);

        assert!(
            config
                .resources
                .dialogue_policy
                .support_need_phrases
                .iter()
                .any(|phrase| phrase == "难过")
        );
        assert!(
            config
                .resources
                .dialogue_policy
                .technical_phrases
                .iter()
                .any(|phrase| phrase == "api")
        );
    }

    fn test_runtime() -> ExpressionRuntime {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();
        ExpressionRuntime::from_config(&config)
    }
}
