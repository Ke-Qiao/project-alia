use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use alia_transport::discover_service_endpoint;
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use ed25519_dalek::SigningKey;
use tokio::sync::RwLock;
use tonic::transport::Channel;

use alia_proto::sensor::embedded_sensor_client::EmbeddedSensorClient;

use crate::modules::embodiment::sensors::{
    SensorDefinition as ServerSensorDefinition, SensorReadProvider,
    SensorReading as ServerSensorReading, SensorRegistry,
};

use super::auth::auth_request_with_signing_key;

pub(super) struct EmbeddedSensorReadProvider {
    pub(super) channel_id: String,
    pub(super) source: String,
    pub(super) endpoint: String,
    pub(super) discovery_service_name: String,
    pub(super) discovery_timeout_sec: u64,
    pub(super) sensor_client: Arc<RwLock<Option<EmbeddedSensorClient<Channel>>>>,
    pub(super) signing_key: SigningKey,
    pub(super) server_id: String,
}

impl EmbeddedSensorReadProvider {
    async fn client(&self) -> anyhow::Result<EmbeddedSensorClient<Channel>> {
        if let Some(client) = self.sensor_client.read().await.clone() {
            return Ok(client);
        }

        let endpoint = if self.endpoint.trim().is_empty() {
            discover_service_endpoint(
                &self.discovery_service_name,
                Duration::from_secs(self.discovery_timeout_sec),
            )
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no embedded endpoint discovered for {}",
                    self.discovery_service_name
                )
            })?
        } else {
            self.endpoint.clone()
        };

        let channel = Channel::from_shared(endpoint.clone())
            .map_err(|error| anyhow::anyhow!("invalid embedded endpoint {endpoint}: {error}"))?
            .connect()
            .await?;
        let client = EmbeddedSensorClient::new(channel);
        *self.sensor_client.write().await = Some(client.clone());
        Ok(client)
    }
}

#[async_trait]
impl SensorReadProvider for EmbeddedSensorReadProvider {
    fn source(&self) -> &str {
        &self.source
    }

    async fn read_sensor(
        &self,
        sensor_id: &str,
        _definition: &ServerSensorDefinition,
    ) -> anyhow::Result<ServerSensorReading> {
        let remote_sensor_id = embedded_sensor_remote_id(&self.channel_id, sensor_id);
        let mut client = self.client().await?;
        let response = match client
            .read_sensor(auth_request_with_signing_key(
                &self.signing_key,
                &self.server_id,
                alia_proto::sensor::SensorRef {
                    sensor_id: remote_sensor_id,
                },
            )?)
            .await
        {
            Ok(response) => response.into_inner(),
            Err(error) => {
                *self.sensor_client.write().await = None;
                return Err(error.into());
            }
        };

        if !response.available {
            let message = if response.error.trim().is_empty() {
                "embedded sensor reported unavailable".to_string()
            } else {
                response.error.clone()
            };
            anyhow::bail!(message);
        }

        Ok(embedded_sensor_reading_to_server(
            &self.channel_id,
            &response,
        ))
    }
}

pub fn embedded_sensor_source(channel_id: &str) -> String {
    format!("component:{}", channel_id.trim())
}

pub fn embedded_sensor_public_id(channel_id: &str, remote_sensor_id: &str) -> String {
    let channel_id = channel_id.trim();
    let remote_sensor_id = remote_sensor_id.trim().trim_matches('/');
    if sensor_id_has_channel_prefix(channel_id, remote_sensor_id) {
        remote_sensor_id.to_string()
    } else {
        format!("{channel_id}:{remote_sensor_id}")
    }
}

pub fn embedded_sensor_remote_id(channel_id: &str, sensor_id: &str) -> String {
    let sensor_id = sensor_id.trim();
    let channel_id = channel_id.trim();
    for separator in [":", "/", "."] {
        if let Some(remote) = sensor_id.strip_prefix(&format!("{channel_id}{separator}"))
            && !remote.trim().is_empty()
        {
            return remote.to_string();
        }
    }
    sensor_id.to_string()
}

pub async fn mirror_embedded_sensor_definitions(
    registry: &SensorRegistry,
    channel_id: &str,
    definitions: Vec<alia_proto::sensor::SensorDefinition>,
) -> anyhow::Result<usize> {
    let source = embedded_sensor_source(channel_id);
    let now = Utc::now();
    let mut active_sensor_ids = HashSet::with_capacity(definitions.len());

    for definition in &definitions {
        active_sensor_ids.insert(embedded_sensor_public_id(channel_id, &definition.sensor_id));
    }

    let offline_count = registry
        .mark_source_sensors_offline_except(&source, &active_sensor_ids)
        .await?;
    if offline_count > 0 {
        tracing::info!(
            channel_id = %channel_id,
            count = offline_count,
            "embedded sensors no longer advertised; marked offline"
        );
    }

    for definition in definitions {
        registry
            .register_sensor(embedded_sensor_definition_to_server(
                channel_id, definition, now,
            ))
            .await?;
    }

    Ok(active_sensor_ids.len())
}

pub async fn ingest_embedded_sensor_reading(
    registry: &SensorRegistry,
    channel_id: &str,
    reading: &alia_proto::sensor::SensorReading,
) -> anyhow::Result<()> {
    if !reading.available {
        return Ok(());
    }

    registry
        .ingest_from_source(
            embedded_sensor_reading_to_server(channel_id, reading),
            &embedded_sensor_source(channel_id),
        )
        .await
}

pub(super) fn embedded_sensor_reading_to_server(
    channel_id: &str,
    reading: &alia_proto::sensor::SensorReading,
) -> ServerSensorReading {
    ServerSensorReading {
        sensor_id: embedded_sensor_public_id(channel_id, &reading.sensor_id),
        value: parse_json_or_null(&reading.value_json),
        timestamp: timestamp_us_to_utc(reading.timestamp_us),
        quality: reading.quality,
    }
}

fn embedded_sensor_definition_to_server(
    channel_id: &str,
    definition: alia_proto::sensor::SensorDefinition,
    seen_at: DateTime<Utc>,
) -> ServerSensorDefinition {
    let remote_sensor_id = definition.sensor_id;
    let public_sensor_id = embedded_sensor_public_id(channel_id, &remote_sensor_id);
    let remote_metadata = parse_json_or_empty(&definition.metadata_json);

    ServerSensorDefinition {
        sensor_id: public_sensor_id,
        display_name: if definition.display_name.trim().is_empty() {
            remote_sensor_id.clone()
        } else {
            definition.display_name
        },
        source: embedded_sensor_source(channel_id),
        unit: definition.unit,
        value_type: if definition.value_type.trim().is_empty() {
            "json".to_string()
        } else {
            definition.value_type
        },
        metadata: serde_json::json!({
            "channel_id": channel_id,
            "remote_sensor_id": remote_sensor_id,
            "remote_source": definition.source,
            "capabilities": definition.capabilities,
            "remote_metadata": remote_metadata,
            "read_via": {
                "channel_action": "read_embedded_sensor"
            },
            "invoke_via": {
                "channel_action": "invoke_embedded_sensor"
            }
        }),
        registered_at: seen_at,
        last_seen_at: Some(seen_at),
    }
}

pub(super) fn embedded_sensor_definition_to_json(
    channel_id: &str,
    definition: alia_proto::sensor::SensorDefinition,
) -> serde_json::Value {
    let remote_sensor_id = definition.sensor_id;
    serde_json::json!({
        "sensor_id": embedded_sensor_public_id(channel_id, &remote_sensor_id),
        "remote_sensor_id": remote_sensor_id,
        "display_name": definition.display_name,
        "source": definition.source,
        "unit": definition.unit,
        "value_type": definition.value_type,
        "capabilities": definition.capabilities,
        "metadata": parse_json_or_empty(&definition.metadata_json),
    })
}

pub(super) fn embedded_sensor_reading_to_json(
    channel_id: &str,
    reading: alia_proto::sensor::SensorReading,
) -> serde_json::Value {
    let remote_sensor_id = reading.sensor_id;
    serde_json::json!({
        "sensor_id": embedded_sensor_public_id(channel_id, &remote_sensor_id),
        "remote_sensor_id": remote_sensor_id,
        "value": parse_json_or_null(&reading.value_json),
        "timestamp_us": reading.timestamp_us,
        "quality": reading.quality,
        "available": reading.available,
        "error": reading.error,
    })
}

pub(super) fn embedded_sensor_command_response_to_json(
    response: alia_proto::sensor::SensorCommandResponse,
) -> serde_json::Value {
    serde_json::json!({
        "success": response.success,
        "result": parse_json_or_null(&response.result_json),
        "message": response.message,
    })
}

fn parse_json_or_empty(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or_else(|_| serde_json::json!({}))
}

fn parse_json_or_null(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or(serde_json::Value::Null)
}

fn sensor_id_has_channel_prefix(channel_id: &str, sensor_id: &str) -> bool {
    sensor_id
        .strip_prefix(channel_id)
        .is_some_and(|rest| matches!(rest.chars().next(), Some(':' | '/' | '.')))
}

fn timestamp_us_to_utc(timestamp_us: u64) -> DateTime<Utc> {
    if timestamp_us == 0 {
        return Utc::now();
    }

    let secs = (timestamp_us / 1_000_000) as i64;
    let nanos = ((timestamp_us % 1_000_000) * 1_000) as u32;
    Utc.timestamp_opt(secs, nanos)
        .single()
        .unwrap_or_else(Utc::now)
}
