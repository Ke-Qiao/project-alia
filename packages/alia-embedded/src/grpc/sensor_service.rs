use std::sync::Arc;

use tonic::{Request, Response, Status};

use alia_proto::sensor::embedded_sensor_server::EmbeddedSensor as EmbeddedSensorTrait;
use alia_proto::sensor::{
    SensorCommandRequest, SensorCommandResponse, SensorDefinition as ProtoSensorDefinition,
    SensorList, SensorReading as ProtoSensorReading, SensorRef,
};

use super::auth::require_auth_if_bound;
use crate::binding::BindingManager;
use crate::module::sensor::{
    SensorDefinition as EmbeddedSensorDefinition, SensorReading as EmbeddedSensorReading,
    SensorRegistry,
};

pub struct SensorService {
    sensors: Arc<SensorRegistry>,
    binding: Arc<BindingManager>,
}

impl SensorService {
    pub fn new(sensors: Arc<SensorRegistry>, binding: Arc<BindingManager>) -> Self {
        Self { sensors, binding }
    }
}

#[tonic::async_trait]
impl EmbeddedSensorTrait for SensorService {
    async fn list_sensors(&self, request: Request<()>) -> Result<Response<SensorList>, Status> {
        require_auth_if_bound(&self.binding, &request).await?;
        let sensors = self
            .sensors
            .list()
            .await
            .into_iter()
            .map(definition_to_proto)
            .collect();
        Ok(Response::new(SensorList { sensors }))
    }

    async fn read_sensor(
        &self,
        request: Request<SensorRef>,
    ) -> Result<Response<ProtoSensorReading>, Status> {
        require_auth_if_bound(&self.binding, &request).await?;
        let sensor_id = request.into_inner().sensor_id;
        match self.sensors.read(&sensor_id).await {
            Ok(reading) => Ok(Response::new(reading_to_proto(
                reading,
                true,
                String::new(),
            ))),
            Err(error) => {
                if self
                    .sensors
                    .get(&sensor_id)
                    .await
                    .map_err(|error| Status::invalid_argument(error.to_string()))?
                    .is_none()
                {
                    return Err(Status::not_found(error.to_string()));
                }
                Ok(Response::new(ProtoSensorReading {
                    sensor_id,
                    value_json: "null".to_string(),
                    timestamp_us: 0,
                    quality: 0.0,
                    available: false,
                    error: error.to_string(),
                }))
            }
        }
    }

    async fn invoke_sensor(
        &self,
        request: Request<SensorCommandRequest>,
    ) -> Result<Response<SensorCommandResponse>, Status> {
        require_auth_if_bound(&self.binding, &request).await?;
        let request = request.into_inner();
        let args = if request.args_json.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&request.args_json)
                .map_err(|error| Status::invalid_argument(error.to_string()))?
        };

        match self
            .sensors
            .invoke(&request.sensor_id, &request.command, args)
            .await
        {
            Ok(result) => Ok(Response::new(SensorCommandResponse {
                success: true,
                result_json: serde_json::to_string(&result)
                    .map_err(|error| Status::internal(error.to_string()))?,
                message: String::new(),
            })),
            Err(error) => {
                if self
                    .sensors
                    .get(&request.sensor_id)
                    .await
                    .map_err(|error| Status::invalid_argument(error.to_string()))?
                    .is_none()
                {
                    return Err(Status::not_found(error.to_string()));
                }
                Ok(Response::new(SensorCommandResponse {
                    success: false,
                    result_json: "null".to_string(),
                    message: error.to_string(),
                }))
            }
        }
    }
}

fn definition_to_proto(definition: EmbeddedSensorDefinition) -> ProtoSensorDefinition {
    ProtoSensorDefinition {
        sensor_id: definition.sensor_id,
        display_name: definition.display_name,
        source: definition.source,
        unit: definition.unit,
        value_type: definition.value_type,
        capabilities: definition.capabilities,
        metadata_json: serde_json::to_string(&definition.metadata)
            .unwrap_or_else(|_| "{}".to_string()),
    }
}

fn reading_to_proto(
    reading: EmbeddedSensorReading,
    available: bool,
    error: String,
) -> ProtoSensorReading {
    ProtoSensorReading {
        sensor_id: reading.sensor_id,
        value_json: serde_json::to_string(&reading.value).unwrap_or_else(|_| "null".to_string()),
        timestamp_us: reading.timestamp_us,
        quality: reading.quality,
        available,
        error,
    }
}
