use alia_server::scheduler::cron::matches_pattern;

#[test]
fn test_matches_pattern() {
    let event = serde_json::json!({
        "type": "PluginEvent",
        "payload": {
            "plugin_name": "weather",
            "event_type": "update"
        }
    });

    let pattern1 = serde_json::json!({
        "type": "PluginEvent",
        "payload": {
            "plugin_name": "weather"
        }
    });
    assert!(matches_pattern(&event, &pattern1));

    let pattern2 = serde_json::json!({
        "type": "PluginEvent"
    });
    assert!(matches_pattern(&event, &pattern2));

    let pattern3 = serde_json::json!({
        "type": "DeviceOnline"
    });
    assert!(!matches_pattern(&event, &pattern3));
}
