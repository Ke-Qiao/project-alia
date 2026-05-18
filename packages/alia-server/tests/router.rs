#![cfg(feature = "llm-ollama")]

use alia_server::providers::adapter::RouteDecision;
use alia_server::providers::router::multi_layer::{
    Classification, LlmMode, MultiLayerRouter, RouterConfig, parse_tool_selection_response,
    strip_code_fences,
};

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
    let c = Classification {
        route: "skill".to_string(),
        skill_name: Some("smart_home".to_string()),
        confidence: Some(0.95),
        reasoning: Some("deterministic command".to_string()),
    };
    match MultiLayerRouter::to_decision(c, LlmMode::BothEnabled) {
        RouteDecision::Skill { skill_name } => {
            assert_eq!(skill_name, "smart_home");
        }
        other => panic!("expected Skill, got {other:?}"),
    }
}

#[test]
fn test_to_decision_minor_llm_both_mode() {
    let c = Classification {
        route: "minor_llm".to_string(),
        skill_name: None,
        confidence: Some(0.8),
        reasoning: None,
    };
    match MultiLayerRouter::to_decision(c, LlmMode::BothEnabled) {
        RouteDecision::MinorLlm {
            system_prompt,
            escalation_prompt,
        } => {
            let prompt = system_prompt.unwrap();
            assert!(
                prompt.contains("[ESCALATE]"),
                "both-mode minor prompt should mention escalation"
            );
            assert!(
                escalation_prompt
                    .as_deref()
                    .is_some_and(|prompt| prompt.contains("primary reasoning model")),
                "both-mode minor route should carry a major escalation prompt"
            );
        }
        other => panic!("expected MinorLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_minor_llm_solo_mode() {
    let c = Classification {
        route: "minor_llm".to_string(),
        skill_name: None,
        confidence: Some(0.8),
        reasoning: None,
    };
    match MultiLayerRouter::to_decision(c, LlmMode::MinorOnly) {
        RouteDecision::MinorLlm {
            system_prompt,
            escalation_prompt,
        } => {
            let prompt = system_prompt.unwrap();
            assert!(
                !prompt.contains("[ESCALATE]"),
                "solo-mode minor prompt should NOT mention escalation"
            );
            assert!(
                prompt.contains("primary and only"),
                "solo prompt should indicate it's the only model"
            );
            assert!(
                escalation_prompt.is_none(),
                "minor-only mode must not carry a major escalation prompt"
            );
        }
        other => panic!("expected MinorLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_major_llm_both_mode() {
    let c = Classification {
        route: "major_llm".to_string(),
        skill_name: None,
        confidence: Some(0.9),
        reasoning: None,
    };
    match MultiLayerRouter::to_decision(c, LlmMode::BothEnabled) {
        RouteDecision::MajorLlm { system_prompt } => {
            let prompt = system_prompt.unwrap();
            assert!(prompt.contains("primary reasoning model"));
        }
        other => panic!("expected MajorLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_major_llm_solo_mode() {
    let c = Classification {
        route: "major_llm".to_string(),
        skill_name: None,
        confidence: Some(0.9),
        reasoning: None,
    };
    match MultiLayerRouter::to_decision(c, LlmMode::MajorOnly) {
        RouteDecision::MajorLlm { system_prompt } => {
            let prompt = system_prompt.unwrap();
            assert!(prompt.contains("all conversations"));
        }
        other => panic!("expected MajorLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_unknown_falls_back() {
    let c = Classification {
        route: "unknown_thing".to_string(),
        skill_name: None,
        confidence: None,
        reasoning: None,
    };
    assert!(matches!(
        MultiLayerRouter::to_decision(c, LlmMode::BothEnabled),
        RouteDecision::MajorLlm { .. }
    ));
}

#[test]
fn test_llm_mode() {
    let mut cfg = RouterConfig {
        ollama_endpoint: String::new(),
        model: String::new(),
        temperature: 0.0,
        timeout_sec: 5,
        minor_llm_enabled: true,
        major_llm_enabled: true,
    };
    assert_eq!(cfg.llm_mode(), LlmMode::BothEnabled);

    cfg.major_llm_enabled = false;
    assert_eq!(cfg.llm_mode(), LlmMode::MinorOnly);

    cfg.minor_llm_enabled = false;
    cfg.major_llm_enabled = true;
    assert_eq!(cfg.llm_mode(), LlmMode::MajorOnly);
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
