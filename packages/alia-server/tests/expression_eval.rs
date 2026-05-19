use std::path::Path;

use alia_server::config::ServerConfig;
use alia_server::expression::{
    AffectUserInput, ExpressionInput, ExpressionRuntime, appraisal_regression_cases,
    planner_regression_cases,
};
use alia_server::providers::adapter::TtsCapabilities;

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
            .any(|phrase| phrase == "sad")
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

#[tokio::test]
async fn planner_respects_tts_capability_limits_before_synthesis() {
    let runtime = test_runtime();
    let plan = runtime
        .plan(ExpressionInput {
            response_text: "Great, I can help with that.".to_string(),
            route: Some("minor_llm".to_string()),
            channel_id: "cli".to_string(),
            user_id: "user".to_string(),
            dialogue_act: "celebrate".to_string(),
            content_tone: "positive".to_string(),
            user_state: Default::default(),
            device_context: Default::default(),
            safety_context: Default::default(),
            conversation_context: Default::default(),
            constraints: Default::default(),
            voice_output: true,
            hardware_control: false,
            tts_capabilities: Some(TtsCapabilities::minimal("plain_tts")),
            expression_capabilities: None,
        })
        .await;

    let speech = plan.speech.expect("voice output should produce speech");
    assert_eq!(speech.style, "neutral");
    assert_eq!(speech.intensity, 0.0);
    assert!(speech.voice_profile.is_none());
    assert_eq!(speech.prosody.rate, 1.0);
    assert_eq!(speech.prosody.pitch_shift, 0.0);
    assert_eq!(speech.prosody.energy, 0.5);
    assert_eq!(speech.prosody.pause_bias, 0.0);
}

fn test_runtime() -> ExpressionRuntime {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();
    ExpressionRuntime::from_config(&config)
}
