use std::sync::Arc;

use alia_server::providers::adapter::{ToolContext, ToolExecutor};
use alia_server::providers::sensor::{
    SensorDefinition, SensorQuery, SensorReading, SensorRegistry, SensorRegistryConfig,
};
use alia_server::scheduler::condition::{ConditionContext, ConditionEvaluator, ConditionRegistry};
use alia_server::scheduler::evaluators::SensorConditionEvaluator;
use alia_server::tools::sensor::{SensorListTool, SensorReadTool};
use chrono::Utc;
use uuid::Uuid;

#[tokio::test]
async fn registry_ingest_auto_registers_and_queries_latest() {
    let sensors = SensorRegistry::new(SensorRegistryConfig::default());
    sensors
        .ingest_from_source(
            SensorReading {
                sensor_id: "embedded:temperature".to_string(),
                value: serde_json::json!(23.5),
                timestamp: Utc::now(),
                quality: 0.95,
            },
            "embedded",
        )
        .await
        .unwrap();

    let def = sensors
        .get_sensor("embedded:temperature")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(def.source, "embedded");
    assert_eq!(def.value_type, "float");

    let latest = sensors
        .latest("embedded:temperature")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.value, serde_json::json!(23.5));

    let query = sensors
        .query(&SensorQuery {
            source: Some("embedded".to_string()),
            latest_only: true,
            ..SensorQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(query.len(), 1);
    assert_eq!(query[0].sensor_id, "embedded:temperature");
}

#[tokio::test]
async fn sensor_tools_read_from_shared_registry() {
    let sensors = Arc::new(SensorRegistry::new(SensorRegistryConfig::default()));
    sensors
        .ingest_from_source(
            SensorReading {
                sensor_id: "room:humidity".to_string(),
                value: serde_json::json!(61),
                timestamp: Utc::now(),
                quality: 1.0,
            },
            "room",
        )
        .await
        .unwrap();

    let ctx = ToolContext {
        user_id: Uuid::new_v4(),
        conversation_id: None,
        source: "test".to_string(),
        channel_id: None,
    };

    let read = SensorReadTool::new(sensors.clone())
        .execute(serde_json::json!({"sensor_id": "room:humidity"}), &ctx)
        .await
        .unwrap();
    assert!(read.success);
    assert_eq!(read.output["value"], serde_json::json!(61));

    let list = SensorListTool::new(sensors)
        .execute(serde_json::json!({"source": "room"}), &ctx)
        .await
        .unwrap();
    assert_eq!(list.output["count"], serde_json::json!(1));
}

#[tokio::test]
async fn sensor_condition_evaluator_uses_latest_registry_value() {
    let sensors = Arc::new(SensorRegistry::new(SensorRegistryConfig::default()));
    sensors
        .ingest_from_source(
            SensorReading {
                sensor_id: "room:co2".to_string(),
                value: serde_json::json!(900),
                timestamp: Utc::now(),
                quality: 1.0,
            },
            "room",
        )
        .await
        .unwrap();

    let evaluator = SensorConditionEvaluator::new(sensors.clone());
    let matched = evaluator
        .evaluate(
            &serde_json::json!({
                "type": "sensor",
                "sensor_id": "room:co2",
                "op": "gte",
                "value": 800
            }),
            &ConditionContext::new(None),
        )
        .await
        .unwrap();
    assert!(matched);

    let registry = ConditionRegistry::new();
    registry
        .register(Arc::new(SensorConditionEvaluator::new(sensors)))
        .await;
    let matched = registry
        .evaluate_all(
            &serde_json::json!([
                {"type": "sensor", "sensor_id": "room:co2", "op": "lt", "value": 1000},
                {"type": "sensor", "sensor_id": "room:co2", "op": "gt", "value": 500}
            ]),
            &ConditionContext::new(None),
        )
        .await
        .unwrap();
    assert!(matched);
}

#[tokio::test]
async fn plugin_sensor_helpers_namespace_track_and_unregister() {
    let sensors = SensorRegistry::new(SensorRegistryConfig::default());

    let full_id = sensors
        .register_plugin_sensor(
            "weather_plugin",
            SensorDefinition {
                sensor_id: "temperature".to_string(),
                display_name: "Outdoor temperature".to_string(),
                source: "wrong-source".to_string(),
                unit: "C".to_string(),
                value_type: "float".to_string(),
                metadata: serde_json::json!({"location": "balcony"}),
                registered_at: Utc::now(),
                last_seen_at: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(full_id, "weather_plugin:temperature");

    sensors
        .ingest_plugin_reading("weather_plugin", "humidity", serde_json::json!(58), 0.8)
        .await
        .unwrap();

    let temperature = sensors.get_sensor(&full_id).await.unwrap().unwrap();
    assert_eq!(temperature.source, "weather_plugin");

    let definitions = sensors
        .list_definitions_by_source("weather_plugin")
        .await
        .unwrap();
    assert_eq!(definitions.len(), 2);
    assert!(
        definitions
            .iter()
            .any(|def| def.sensor_id == "weather_plugin:humidity")
    );

    let removed = sensors
        .unregister_plugin_sensors("weather_plugin")
        .await
        .unwrap();
    assert_eq!(removed, 2);
    assert!(sensors.get_sensor(&full_id).await.unwrap().is_none());
}
