use alia_server::providers::adapter::RouteDecision;
use alia_server::providers::router::multi_layer::{
    Classification, LlmMode, MultiLayerRouter, RouterConfig, parse_tool_selection_response,
    strip_code_fences,
};

const TEST_ASSISTANT_NAME: &str = "玲珑";

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
        skill_name: Some("smart_light_control".to_string()),
        confidence: Some(0.95),
        reasoning: Some("deterministic command".to_string()),
    };

    match MultiLayerRouter::to_decision(classification, LlmMode::BothEnabled, TEST_ASSISTANT_NAME) {
        RouteDecision::Skill { skill_name } => {
            assert_eq!(skill_name, "smart_light_control");
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
        MultiLayerRouter::to_decision(classification, LlmMode::BothEnabled, TEST_ASSISTANT_NAME),
        RouteDecision::MajorLlm { .. }
    ));
}

#[test]
fn test_to_decision_minor_llm_both_mode() {
    let classification = Classification {
        route: "minor_llm".to_string(),
        skill_name: None,
        confidence: Some(0.8),
        reasoning: None,
    };

    match MultiLayerRouter::to_decision(classification, LlmMode::BothEnabled, TEST_ASSISTANT_NAME) {
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

    match MultiLayerRouter::to_decision(classification, LlmMode::MinorOnly, TEST_ASSISTANT_NAME) {
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

    match MultiLayerRouter::to_decision(classification, LlmMode::BothEnabled, TEST_ASSISTANT_NAME) {
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

    match MultiLayerRouter::to_decision(classification, LlmMode::MajorOnly, TEST_ASSISTANT_NAME) {
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
        MultiLayerRouter::to_decision(classification, LlmMode::BothEnabled, TEST_ASSISTANT_NAME),
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
