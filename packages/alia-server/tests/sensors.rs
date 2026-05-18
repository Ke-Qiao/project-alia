use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "channel-embedded")]
use alia_proto::sensor::{
    SensorDefinition as EmbeddedProtoSensorDefinition, SensorReading as EmbeddedProtoSensorReading,
};
#[cfg(feature = "channel-embedded")]
use alia_server::channels::embedded::{
    embedded_sensor_public_id, embedded_sensor_remote_id, ingest_embedded_sensor_reading,
    mirror_embedded_sensor_definitions,
};
use alia_server::providers::adapter::{ToolContext, ToolExecutor};
use alia_server::providers::sensor::{
    SensorDefinition, SensorQuery, SensorReadProvider, SensorReading, SensorRegistry,
    SensorRegistryConfig,
};
use alia_server::scheduler::condition::{ConditionContext, ConditionEvaluator, ConditionRegistry};
use alia_server::scheduler::evaluators::SensorConditionEvaluator;
use alia_server::tools::sensor::{SensorListTool, SensorReadTool};
use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

struct FakeSensorReadProvider {
    source: String,
    value: serde_json::Value,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl SensorReadProvider for FakeSensorReadProvider {
    fn source(&self) -> &str {
        &self.source
    }

    async fn read_sensor(
        &self,
        sensor_id: &str,
        _definition: &SensorDefinition,
    ) -> anyhow::Result<SensorReading> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(SensorReading {
            sensor_id: sensor_id.to_string(),
            value: self.value.clone(),
            timestamp: Utc::now(),
            quality: 1.0,
        })
    }
}

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
async fn sensor_read_tool_refreshes_from_registered_provider() {
    let sensors = Arc::new(SensorRegistry::new(SensorRegistryConfig::default()));
    sensors
        .register_sensor(SensorDefinition {
            sensor_id: "probe:temperature".to_string(),
            display_name: "Probe temperature".to_string(),
            source: "probe".to_string(),
            unit: "C".to_string(),
            value_type: "float".to_string(),
            metadata: serde_json::json!({}),
            registered_at: Utc::now(),
            last_seen_at: None,
        })
        .await
        .unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    sensors
        .register_read_provider(Arc::new(FakeSensorReadProvider {
            source: "probe".to_string(),
            value: serde_json::json!(22.25),
            calls: calls.clone(),
        }))
        .await
        .unwrap();

    let ctx = ToolContext {
        user_id: Uuid::new_v4(),
        conversation_id: None,
        source: "test".to_string(),
        channel_id: None,
    };
    let read = SensorReadTool::new(sensors.clone())
        .execute(
            serde_json::json!({
                "sensor_id": "probe:temperature",
                "read_mode": "fresh",
                "max_age_sec": 60
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(read.success);
    assert_eq!(read.output["value"], serde_json::json!(22.25));
    assert_eq!(read.output["refreshed"], serde_json::json!(true));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        sensors
            .latest("probe:temperature")
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!(22.25)
    );
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
async fn sensor_condition_evaluator_refreshes_missing_provider_value() {
    let sensors = Arc::new(SensorRegistry::new(SensorRegistryConfig::default()));
    sensors
        .register_sensor(SensorDefinition {
            sensor_id: "probe:co2".to_string(),
            display_name: "Probe CO2".to_string(),
            source: "probe".to_string(),
            unit: "ppm".to_string(),
            value_type: "int".to_string(),
            metadata: serde_json::json!({}),
            registered_at: Utc::now(),
            last_seen_at: None,
        })
        .await
        .unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    sensors
        .register_read_provider(Arc::new(FakeSensorReadProvider {
            source: "probe".to_string(),
            value: serde_json::json!(810),
            calls: calls.clone(),
        }))
        .await
        .unwrap();

    let evaluator = SensorConditionEvaluator::new(sensors);
    let matched = evaluator
        .evaluate(
            &serde_json::json!({
                "type": "sensor",
                "sensor_id": "probe:co2",
                "op": "gte",
                "value": 800,
                "read_mode": "fresh",
                "timeout_ms": 500
            }),
            &ConditionContext::new(None),
        )
        .await
        .unwrap();

    assert!(matched);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
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

#[cfg(feature = "channel-embedded")]
#[tokio::test]
async fn embedded_sensor_mirror_registers_remote_definitions_and_marks_missing_offline() {
    let sensors = SensorRegistry::new(SensorRegistryConfig::default());

    let mirrored = mirror_embedded_sensor_definitions(
        &sensors,
        "embedded",
        vec![
            EmbeddedProtoSensorDefinition {
                sensor_id: "temperature".to_string(),
                display_name: "Temperature".to_string(),
                source: "embedded".to_string(),
                unit: "C".to_string(),
                value_type: "float".to_string(),
                capabilities: vec!["read".to_string()],
                metadata_json: r#"{"location":"desk"}"#.to_string(),
            },
            EmbeddedProtoSensorDefinition {
                sensor_id: "humidity".to_string(),
                display_name: "Humidity".to_string(),
                source: "embedded".to_string(),
                unit: "%".to_string(),
                value_type: "float".to_string(),
                capabilities: vec!["read".to_string()],
                metadata_json: "{}".to_string(),
            },
        ],
    )
    .await
    .unwrap();

    assert_eq!(mirrored, 2);
    assert_eq!(
        embedded_sensor_public_id("embedded", "temperature"),
        "embedded:temperature"
    );
    assert_eq!(
        embedded_sensor_remote_id("embedded", "embedded:temperature"),
        "temperature"
    );

    let temp = sensors
        .get_sensor("embedded:temperature")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(temp.source, "component:embedded");
    assert_eq!(temp.metadata["remote_sensor_id"], "temperature");
    assert_eq!(temp.metadata["remote_metadata"]["location"], "desk");
    assert!(temp.last_seen_at.is_some());

    mirror_embedded_sensor_definitions(
        &sensors,
        "embedded",
        vec![EmbeddedProtoSensorDefinition {
            sensor_id: "humidity".to_string(),
            display_name: "Humidity".to_string(),
            source: "embedded".to_string(),
            unit: "%".to_string(),
            value_type: "float".to_string(),
            capabilities: vec!["read".to_string()],
            metadata_json: "{}".to_string(),
        }],
    )
    .await
    .unwrap();

    let temp = sensors
        .get_sensor("embedded:temperature")
        .await
        .unwrap()
        .unwrap();
    let humidity = sensors
        .get_sensor("embedded:humidity")
        .await
        .unwrap()
        .unwrap();
    assert!(temp.last_seen_at.is_none());
    assert!(humidity.last_seen_at.is_some());
}

#[cfg(feature = "channel-embedded")]
#[tokio::test]
async fn embedded_sensor_reading_ingests_using_public_sensor_id() {
    let sensors = SensorRegistry::new(SensorRegistryConfig::default());
    ingest_embedded_sensor_reading(
        &sensors,
        "pi5",
        &EmbeddedProtoSensorReading {
            sensor_id: "temperature".to_string(),
            value_json: "24.5".to_string(),
            timestamp_us: 1_700_000_000_123_456,
            quality: 0.8,
            available: true,
            error: String::new(),
        },
    )
    .await
    .unwrap();

    let latest = sensors.latest("pi5:temperature").await.unwrap().unwrap();
    assert_eq!(latest.value, serde_json::json!(24.5));
    assert_eq!(latest.quality, 0.8);
    assert_eq!(
        sensors
            .get_sensor("pi5:temperature")
            .await
            .unwrap()
            .unwrap()
            .source,
        "component:pi5"
    );
}
