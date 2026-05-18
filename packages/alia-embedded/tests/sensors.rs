use std::sync::Arc;

use alia_embedded::module::sensor::{
    SensorDefinition, SensorProvider, SensorReading, SensorRegistry,
};
use anyhow::Result;

struct StaticSensor {
    definition: SensorDefinition,
    value: serde_json::Value,
}

#[tonic::async_trait]
impl SensorProvider for StaticSensor {
    fn definition(&self) -> SensorDefinition {
        self.definition.clone()
    }

    async fn read(&self) -> Result<SensorReading> {
        Ok(SensorReading::now(
            self.definition.sensor_id.clone(),
            self.value.clone(),
        ))
    }

    async fn invoke(&self, command: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "command": command,
            "args": args,
        }))
    }
}

#[tokio::test]
async fn registry_lists_reads_and_invokes_modular_sensor_provider() {
    let registry = SensorRegistry::new();
    let definition = SensorDefinition {
        sensor_id: "internal:temperature".to_string(),
        display_name: "Internal temperature".to_string(),
        source: "builtin".to_string(),
        unit: "C".to_string(),
        value_type: "float".to_string(),
        capabilities: vec!["read".to_string(), "calibrate".to_string()],
        metadata: serde_json::json!({"location": "case"}),
    };

    let replaced = registry
        .register(Arc::new(StaticSensor {
            definition,
            value: serde_json::json!(42.5),
        }))
        .await
        .unwrap();
    assert!(!replaced);

    let sensors = registry.list().await;
    assert_eq!(sensors.len(), 1);
    assert_eq!(sensors[0].sensor_id, "internal:temperature");

    let reading = registry.read("internal:temperature").await.unwrap();
    assert_eq!(reading.value, serde_json::json!(42.5));
    assert_eq!(reading.quality, 1.0);

    let result = registry
        .invoke(
            "internal:temperature",
            "calibrate",
            serde_json::json!({"offset": -0.5}),
        )
        .await
        .unwrap();
    assert_eq!(result["command"], "calibrate");
    assert_eq!(result["args"]["offset"], -0.5);
}

#[tokio::test]
async fn registry_rejects_unknown_sensor_reads() {
    let registry = SensorRegistry::new();

    let error = registry.read("missing:co2").await.unwrap_err();

    assert!(error.to_string().contains("not registered"));
}
