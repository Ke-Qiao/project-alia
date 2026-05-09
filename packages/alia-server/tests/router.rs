#![cfg(feature = "llm-ollama")]

use alia_server::providers::router::multi_layer::{
    Classification, LlmMode, MultiLayerRouter, RouterConfig,
};
use alia_server::providers::router::multi_layer::strip_code_fences;
use alia_server::providers::adapter::RouteDecision;

#[test]
fn test_strip_code_fences_bare_json() {
    let input = r#"{"route": "remote_llm", "confidence": 0.9}"#;
    assert_eq!(strip_code_fences(input), input);
}

#[test]
fn test_strip_code_fences_with_json_tag() {
    let input = "```json\n{\"route\": \"local_llm\"}\n```";
    assert_eq!(strip_code_fences(input), "{\"route\": \"local_llm\"}");
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
        params: Some(serde_json::json!({"action": "turn_on", "device": "light"})),
        confidence: Some(0.95),
        reasoning: Some("deterministic command".to_string()),
    };
    match MultiLayerRouter::to_decision(c, LlmMode::BothEnabled) {
        RouteDecision::Skill { skill_name, params } => {
            assert_eq!(skill_name, "smart_home");
            assert_eq!(params["action"], "turn_on");
        }
        other => panic!("expected Skill, got {other:?}"),
    }
}

#[test]
fn test_to_decision_local_llm_both_mode() {
    let c = Classification {
        route: "local_llm".to_string(),
        skill_name: None,
        params: None,
        confidence: Some(0.8),
        reasoning: None,
    };
    match MultiLayerRouter::to_decision(c, LlmMode::BothEnabled) {
        RouteDecision::LocalLlm { system_prompt } => {
            let prompt = system_prompt.unwrap();
            assert!(
                prompt.contains("[ESCALATE]"),
                "both-mode local prompt should mention escalation"
            );
        }
        other => panic!("expected LocalLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_local_llm_solo_mode() {
    let c = Classification {
        route: "local_llm".to_string(),
        skill_name: None,
        params: None,
        confidence: Some(0.8),
        reasoning: None,
    };
    match MultiLayerRouter::to_decision(c, LlmMode::LocalOnly) {
        RouteDecision::LocalLlm { system_prompt } => {
            let prompt = system_prompt.unwrap();
            assert!(
                !prompt.contains("[ESCALATE]"),
                "solo-mode local prompt should NOT mention escalation"
            );
            assert!(
                prompt.contains("primary and only"),
                "solo prompt should indicate it's the only model"
            );
        }
        other => panic!("expected LocalLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_remote_llm_both_mode() {
    let c = Classification {
        route: "remote_llm".to_string(),
        skill_name: None,
        params: None,
        confidence: Some(0.9),
        reasoning: None,
    };
    match MultiLayerRouter::to_decision(c, LlmMode::BothEnabled) {
        RouteDecision::RemoteLlm { system_prompt } => {
            let prompt = system_prompt.unwrap();
            assert!(prompt.contains("primary reasoning model"));
        }
        other => panic!("expected RemoteLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_remote_llm_solo_mode() {
    let c = Classification {
        route: "remote_llm".to_string(),
        skill_name: None,
        params: None,
        confidence: Some(0.9),
        reasoning: None,
    };
    match MultiLayerRouter::to_decision(c, LlmMode::RemoteOnly) {
        RouteDecision::RemoteLlm { system_prompt } => {
            let prompt = system_prompt.unwrap();
            assert!(prompt.contains("all conversations"));
        }
        other => panic!("expected RemoteLlm, got {other:?}"),
    }
}

#[test]
fn test_to_decision_unknown_falls_back() {
    let c = Classification {
        route: "unknown_thing".to_string(),
        skill_name: None,
        params: None,
        confidence: None,
        reasoning: None,
    };
    assert!(matches!(
        MultiLayerRouter::to_decision(c, LlmMode::BothEnabled),
        RouteDecision::RemoteLlm { .. }
    ));
}

#[test]
fn test_llm_mode() {
    let mut cfg = RouterConfig {
        ollama_endpoint: String::new(),
        model: String::new(),
        temperature: 0.0,
        timeout_sec: 5,
        local_llm_enabled: true,
        remote_llm_enabled: true,
    };
    assert_eq!(cfg.llm_mode(), LlmMode::BothEnabled);

    cfg.remote_llm_enabled = false;
    assert_eq!(cfg.llm_mode(), LlmMode::LocalOnly);

    cfg.local_llm_enabled = false;
    cfg.remote_llm_enabled = true;
    assert_eq!(cfg.llm_mode(), LlmMode::RemoteOnly);
}
