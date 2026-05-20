use std::path::Path;

use alia_server::config::ServerConfig;
use alia_server::expression::{
    AffectUserInput, ExpressionActionCapability, ExpressionAdapterState,
    ExpressionCapabilitySnapshot, ExpressionInput, ExpressionRuntime, appraisal_regression_cases,
    planner_regression_cases,
};
use alia_server::providers::adapter::{
    MemoryAffectHint, MemoryContextPack, MemoryRecallEvent, MemorySnippet, TtsCapabilities,
};
use std::collections::BTreeMap;

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
            memory_context: None,
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

#[tokio::test]
async fn planner_uses_memory_boundaries_to_reduce_motion_intensity() {
    let runtime = test_runtime();
    let mut capabilities = BTreeMap::new();
    capabilities.insert(
        "small_nod".to_string(),
        ExpressionActionCapability {
            supported: true,
            max_intensity: Some(0.9),
            max_duration_ms: Some(900),
            max_frequency_per_min: Some(60),
            ..ExpressionActionCapability::default()
        },
    );

    let plan = runtime
        .plan(ExpressionInput {
            response_text: "Great, it is complete.".to_string(),
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
            memory_context: Some(MemoryContextPack {
                boundaries: vec![MemorySnippet {
                    id: "mem_motion_boundary".to_string(),
                    kind: "boundary".to_string(),
                    content: "The user prefers low intensity motion and not exaggerated gestures."
                        .to_string(),
                    confidence: 0.95,
                    source: "memory_runtime".to_string(),
                }],
                ..MemoryContextPack::default()
            }),
            voice_output: true,
            hardware_control: true,
            tts_capabilities: None,
            expression_capabilities: Some(ExpressionCapabilitySnapshot {
                adapter: "test_adapter".to_string(),
                capabilities,
                current_state: ExpressionAdapterState {
                    motion_adapter_registered: true,
                    ..ExpressionAdapterState::default()
                },
            }),
        })
        .await;

    assert!(plan.timeline.iter().all(|action| action.intensity <= 0.45));
}

#[tokio::test]
async fn memory_recall_biases_affect_only_through_limited_appraisal_events() {
    let runtime = test_runtime();
    let before = runtime.affect_snapshot("cli", "memory_user").await;
    let observation = runtime
        .observe_memory_recall(
            "cli",
            "memory_user",
            &[MemoryRecallEvent {
                memory_id: "mem_preference".to_string(),
                memory_kind: "preference".to_string(),
                content: "The user prefers direct engineering answers.".to_string(),
                confidence: 1.0,
                salience: 1.0,
                affect_hint: MemoryAffectHint {
                    valence_delta: Some(1.0),
                    arousal_delta: Some(1.0),
                    trust_delta: Some(1.0),
                    engagement_delta: Some(1.0),
                    frustration_risk_delta: Some(-1.0),
                },
                evidence_memory_ids: vec!["mem_preference".to_string()],
                source_event_ids: Vec::new(),
            }],
        )
        .await;

    assert_eq!(observation.appraisals.len(), 1);
    assert_eq!(observation.appraisals[0].kind, "memory_recall");
    assert!(observation.appraisals[0].deltas.valence <= 0.05);
    assert!(observation.appraisals[0].deltas.trust <= 0.02);
    assert!(observation.appraisals[0].deltas.engagement <= 0.08);
    assert!(observation.snapshot.affect.valence - before.affect.valence <= 0.05);
    assert!(observation.snapshot.social.trust - before.social.trust <= 0.02);
}

fn test_runtime() -> ExpressionRuntime {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();
    ExpressionRuntime::from_config(&config)
}
