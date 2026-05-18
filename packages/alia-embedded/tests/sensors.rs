use std::sync::Arc;

use alia_embedded::module::hmmd_mmwave::{parse_hmmd_report_payload, parse_hmmd_serial_buffer};
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

#[test]
fn parses_hmmd_report_payload_presence_and_distance() {
    let reading = parse_hmmd_report_payload(&[1, 0x2C, 0x01, 0xAA]).unwrap();

    assert!(reading.presence);
    assert_eq!(reading.target_distance_cm, Some(300));
    assert_eq!(reading.raw_payload_hex, "012C01AA");
}

#[test]
fn parses_hmmd_binary_report_frame_from_serial_buffer() {
    let mut buffer = vec![
        0x00, 0xF4, 0xF3, 0xF2, 0xF1, 0x04, 0x00, 0x01, 0x64, 0x00, 0x7F, 0xF8, 0xF7, 0xF6, 0xF5,
    ];

    let readings = parse_hmmd_serial_buffer(&mut buffer);

    assert_eq!(readings.len(), 1);
    assert!(readings[0].presence);
    assert_eq!(readings[0].target_distance_cm, Some(100));
    assert!(buffer.is_empty());
}

#[test]
fn parses_hmmd_ascii_fallback_lines() {
    let mut buffer = b"ON 250\r\nOFF\r\n".to_vec();

    let readings = parse_hmmd_serial_buffer(&mut buffer);

    assert_eq!(readings.len(), 2);
    assert!(readings[0].presence);
    assert_eq!(readings[0].target_distance_cm, Some(250));
    assert!(!readings[1].presence);
}
