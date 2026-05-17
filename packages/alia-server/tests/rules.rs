use std::sync::Arc;

use alia_server::event::ServerEvent;
use alia_server::providers::adapter::{AutomationRule, RuleEvaluator};
use alia_server::rules::EventRuleEngine;
use alia_server::scheduler::ConditionRegistry;
use alia_server::scheduler::action::ActionRegistry;
use alia_server::scheduler::evaluators::EventFieldConditionEvaluator;
use uuid::Uuid;

#[tokio::test]
async fn test_event_rule_matching() {
    let conditions = Arc::new(ConditionRegistry::new());
    conditions
        .register(Arc::new(EventFieldConditionEvaluator))
        .await;
    let actions = Arc::new(ActionRegistry::new());
    let engine = EventRuleEngine::with_registries(conditions, actions);

    engine
        .create_rule(AutomationRule {
            id: Uuid::new_v4(),
            user_id: Uuid::nil(),
            name: "Test Rule".to_string(),
            enabled: true,
            event_pattern: serde_json::json!({"type": "DeviceOnline"}),
            conditions: serde_json::json!([
                {
                    "type": "event_field",
                    "field": "payload.device_id",
                    "op": "eq",
                    "value": "device-123"
                }
            ]),
            action: serde_json::json!({"type": "log", "message": "Device came online"}),
            cooldown_sec: 0,
            created_by: "test".to_string(),
        })
        .await
        .unwrap();

    let event = ServerEvent::DeviceOnline {
        device_id: "device-123".to_string(),
    };

    let actions = engine.evaluate(&event).await.unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].rule_name, "Test Rule");
}
