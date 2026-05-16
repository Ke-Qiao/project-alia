use alia_server::event::ServerEvent;
use alia_server::providers::RuleEvaluator;
use alia_server::rules::engine::{Condition, Rule, SimpleRuleEngine};
use uuid::Uuid;

#[tokio::test]
async fn test_condition_evaluation() {
    let data = serde_json::json!({
        "type": "device_online",
        "payload": {
            "device_id": "device-123"
        }
    });

    let cond = Condition::Equals {
        field: "payload.device_id".to_string(),
        value: serde_json::json!("device-123"),
    };
    assert!(SimpleRuleEngine::evaluate_condition(&cond, &data));

    let cond = Condition::Contains {
        field: "payload.device_id".to_string(),
        value: "123".to_string(),
    };
    assert!(SimpleRuleEngine::evaluate_condition(&cond, &data));
}

#[tokio::test]
async fn test_rule_matching() {
    let engine = SimpleRuleEngine::new();

    {
        let mut rules = engine.rules.write().await;
        rules.push(Rule {
            id: Uuid::new_v4(),
            name: "Test Rule".to_string(),
            enabled: true,
            trigger_event: "device_online".to_string(),
            conditions: None,
            action_type: "notify".to_string(),
            action_payload: serde_json::json!({"message": "Device came online"}),
        });
    }

    let event = ServerEvent::DeviceOnline {
        device_id: "device-123".to_string(),
    };

    let actions = engine.evaluate(&event).await.unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].rule_name, "Test Rule");
}
