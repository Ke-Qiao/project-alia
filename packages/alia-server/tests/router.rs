use alia_server::modules::interaction::routing::RouteDecision;
use alia_server::modules::interaction::routing::{
    Classification, LlmMode, RouterConfig, RouterService, parse_tool_selection_response,
    strip_code_fences,
};

use alia_server::modules::interaction::skills::SkillCandidate;

const TEST_ASSISTANT_NAME: &str = "玲珑";

fn candidate(name: &str) -> SkillCandidate {
    SkillCandidate {
        name: name.into(),
        display_name: name.into(),
        description: "A dynamically available skill".into(),
        category: "test".into(),
        keywords: vec!["custom keyword".into()],
    }
}

#[test]
fn test_strip_code_fences_bare_json() {
    let input = r#"{"route": "major_llm", "confidence": 0.9}"#;
    assert_eq!(strip_code_fences(input), input);
}

#[test]
fn test_strip_code_fences_with_json_tag() {
    let input = "```json\n{\"route\": \"minor_llm\"}\n```";
    assert_eq!(strip_code_fences(input), "{\"route\": \"minor_llm\"}");
}

#[test]
fn test_strip_code_fences_without_tag() {
    let input = "```\n{\"route\": \"skill\"}\n```";
    assert_eq!(strip_code_fences(input), "{\"route\": \"skill\"}");
}

#[test]
fn test_to_decision_skill() {
    let classification = Classification {
        route: "skill".to_string(),
        skill_name: Some("new_custom_skill".to_string()),
        confidence: Some(0.95),
        reasoning: Some("deterministic command".to_string()),
    };

    match RouterService::to_decision(
        classification,
        LlmMode::BothEnabled,
        TEST_ASSISTANT_NAME,
        &[candidate("new_custom_skill")],
    ) {
        RouteDecision::Skill { skill_name } => {
            assert_eq!(skill_name, "new_custom_skill");
        }
        other => panic!("expected Skill, got {other:?}"),
    }
}

#[test]
fn test_to_decision_skill_without_name_falls_back() {
    let classification = Classification {
        route: "skill".to_string(),
        skill_name: None,
        confidence: Some(0.4),
        reasoning: None,
    };

    assert!(matches!(
        RouterService::to_decision(
            classification,
            LlmMode::BothEnabled,
            TEST_ASSISTANT_NAME,
            &[]
        ),
        RouteDecision::MajorLlm { .. }
    ));
}

#[test]
fn skill_decision_requires_an_offered_name_and_preserves_mode_fallback() {
    for name in [None, Some(""), Some("  \n"), Some("unknown_skill")] {
        for mode in [LlmMode::BothEnabled, LlmMode::MinorOnly] {
            let decision = RouterService::to_decision(
                Classification {
                    route: "skill".into(),
                    skill_name: name.map(str::to_string),
                    confidence: Some(1.0),
                    reasoning: None,
                },
                mode,
                TEST_ASSISTANT_NAME,
                &[candidate("available_skill")],
            );
            match (mode, decision) {
                (LlmMode::BothEnabled, RouteDecision::MajorLlm { .. })
                | (
                    LlmMode::MinorOnly,
                    RouteDecision::MinorLlm {
                        escalation_prompt: None,
                        ..
                    },
                ) => {}
                other => panic!("unexpected fallback: {other:?}"),
            }
        }
    }
}

#[test]
fn offered_skill_names_prefer_exact_match_before_trim_compatibility() {
    for (candidates, expected) in [
        (vec![candidate("custom"), candidate(" custom ")], " custom "),
        (vec![candidate("custom")], "custom"),
    ] {
        let decision = RouterService::to_decision(
            Classification {
                route: "skill".into(),
                skill_name: Some(" custom ".into()),
                confidence: Some(0.0),
                reasoning: None,
            },
            LlmMode::BothEnabled,
            TEST_ASSISTANT_NAME,
            &candidates,
        );
        let RouteDecision::Skill { skill_name } = decision else {
            panic!("offered name should resolve without a confidence threshold");
        };
        assert_eq!(skill_name, expected);
    }
}

#[test]
fn major_only_rejects_minor_and_skill_classification_outputs() {
    for route in ["minor_llm", "skill"] {
        assert!(matches!(
            RouterService::to_decision(
                Classification {
                    route: route.into(),
                    skill_name: Some("available_skill".into()),
                    confidence: Some(1.0),
                    reasoning: None,
                },
                LlmMode::MajorOnly,
                TEST_ASSISTANT_NAME,
                &[candidate("available_skill")],
            ),
            RouteDecision::MajorLlm { .. }
        ));
    }
}

#[test]
fn test_to_decision_minor_llm_both_mode() {
    let classification = Classification {
        route: "minor_llm".to_string(),
        skill_name: None,
        confidence: Some(0.8),
        reasoning: None,
    };

    match RouterService::to_decision(
        classification,
        LlmMode::BothEnabled,
        TEST_ASSISTANT_NAME,
        &[],
    ) {
        RouteDecision::MinorLlm {
            system_prompt,
            escalation_prompt,
        } => {
            let prompt = system_prompt.unwrap();
            assert!(prompt.contains(TEST_ASSISTANT_NAME));
            assert!(prompt.contains("[ESCALATE]"));
            assert!(
                escalation_prompt
                    .as_deref()
                    .is_some_and(|prompt| prompt.contains("primary reasoning model"))
            );
        }
        other => panic!("expected MinorLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_minor_llm_solo_mode() {
    let classification = Classification {
        route: "minor_llm".to_string(),
        skill_name: None,
        confidence: Some(0.8),
        reasoning: None,
    };

    match RouterService::to_decision(classification, LlmMode::MinorOnly, TEST_ASSISTANT_NAME, &[]) {
        RouteDecision::MinorLlm {
            system_prompt,
            escalation_prompt,
        } => {
            let prompt = system_prompt.unwrap();
            assert!(prompt.contains(TEST_ASSISTANT_NAME));
            assert!(!prompt.contains("[ESCALATE]"));
            assert!(prompt.contains("primary and only"));
            assert!(escalation_prompt.is_none());
        }
        other => panic!("expected MinorLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_major_llm_both_mode() {
    let classification = Classification {
        route: "major_llm".to_string(),
        skill_name: None,
        confidence: Some(0.9),
        reasoning: None,
    };

    match RouterService::to_decision(
        classification,
        LlmMode::BothEnabled,
        TEST_ASSISTANT_NAME,
        &[],
    ) {
        RouteDecision::MajorLlm { system_prompt } => {
            let prompt = system_prompt.unwrap();
            assert!(prompt.contains(TEST_ASSISTANT_NAME));
            assert!(prompt.contains("primary reasoning model"));
        }
        other => panic!("expected MajorLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_major_llm_solo_mode() {
    let classification = Classification {
        route: "major_llm".to_string(),
        skill_name: None,
        confidence: Some(0.9),
        reasoning: None,
    };

    match RouterService::to_decision(classification, LlmMode::MajorOnly, TEST_ASSISTANT_NAME, &[]) {
        RouteDecision::MajorLlm { system_prompt } => {
            let prompt = system_prompt.unwrap();
            assert!(prompt.contains(TEST_ASSISTANT_NAME));
            assert!(prompt.contains("all conversations"));
        }
        other => panic!("expected MajorLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_unknown_falls_back() {
    let classification = Classification {
        route: "unknown_thing".to_string(),
        skill_name: None,
        confidence: None,
        reasoning: None,
    };

    assert!(matches!(
        RouterService::to_decision(
            classification,
            LlmMode::BothEnabled,
            TEST_ASSISTANT_NAME,
            &[]
        ),
        RouteDecision::MajorLlm { .. }
    ));
}

#[test]
fn test_llm_mode() {
    let mut config = RouterConfig {
        timeout_sec: 5,
        minor_llm_enabled: true,
        major_llm_enabled: true,
    };
    assert_eq!(config.llm_mode(), LlmMode::BothEnabled);

    config.major_llm_enabled = false;
    assert_eq!(config.llm_mode(), LlmMode::MinorOnly);

    config.minor_llm_enabled = false;
    config.major_llm_enabled = true;
    assert_eq!(config.llm_mode(), LlmMode::MajorOnly);
}

#[test]
fn test_parse_tool_selection_response_filters_to_candidates() {
    let raw = r#"```json
{"selected":["fetch","unknown","fetch","memory_search"],"reasoning":"need web and memory"}
```"#;
    let selected =
        parse_tool_selection_response(raw, &["fetch", "memory_search", "calculate"], 2).unwrap();

    assert_eq!(selected, vec!["fetch", "memory_search"]);
}
