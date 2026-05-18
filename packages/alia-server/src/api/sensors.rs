use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use utoipa::ToSchema;

use crate::auth::AuthUser;
use crate::error::{AppError, Result};
use crate::event::ServerEvent;
use crate::providers::sensor::{SensorDefinition, SensorQuery, SensorReading, SensorSummary};
use crate::state::AppState;

// ── List all sensors ────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/v1/sensors",
    tag = "sensors",
    security(("bearer" = [])),
    responses((status = 200, description = "List sensors", body = [SensorSummary]))
)]
pub async fn list_sensors(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<SensorSummary>>> {
    let sensors = state.sensors.list_sensors().await?;
    Ok(Json(sensors))
}

// ── Get single sensor definition ────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/v1/sensors/{sensor_id}",
    tag = "sensors",
    security(("bearer" = [])),
    params(("sensor_id" = String, Path, description = "Sensor id")),
    responses((status = 200, description = "Sensor definition", body = Option<SensorDefinition>))
)]
pub async fn get_sensor(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(sensor_id): Path<String>,
) -> Result<Json<Option<SensorDefinition>>> {
    let def = state.sensors.get_sensor(&sensor_id).await?;
    Ok(Json(def))
}

// ── Register a sensor ───────────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
pub struct RegisterSensorRequest {
    pub sensor_id: String,
    pub display_name: String,
    pub source: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default = "default_value_type")]
    pub value_type: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

fn default_value_type() -> String {
    "float".into()
}

#[utoipa::path(
    post,
    path = "/api/v1/sensors",
    tag = "sensors",
    security(("bearer" = [])),
    request_body = RegisterSensorRequest,
    responses((status = 200, description = "Sensor registered", body = SensorDefinition))
)]
pub async fn register_sensor(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<RegisterSensorRequest>,
) -> Result<Json<SensorDefinition>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }

    let def = SensorDefinition {
        sensor_id: req.sensor_id.clone(),
        display_name: req.display_name,
        source: req.source,
        unit: req.unit,
        value_type: req.value_type,
        metadata: req.metadata,
        registered_at: Utc::now(),
        last_seen_at: None,
    };
    state.sensors.register_sensor(def.clone()).await?;
    Ok(Json(def))
}

// ── Unregister a sensor ─────────────────────────────────────────────────

#[utoipa::path(
    delete,
    path = "/api/v1/sensors/{sensor_id}",
    tag = "sensors",
    security(("bearer" = [])),
    params(("sensor_id" = String, Path, description = "Sensor id")),
    responses((status = 200, description = "Sensor unregistered"))
)]
pub async fn unregister_sensor(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(sensor_id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }

    let removed = state.sensors.unregister_sensor(&sensor_id).await?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

// ── Ingest readings ─────────────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
pub struct IngestRequest {
    pub sensor_id: String,
    pub value: serde_json::Value,
    #[serde(default = "default_quality")]
    pub quality: f32,
}

fn default_quality() -> f32 {
    1.0
}

#[utoipa::path(
    post,
    path = "/api/v1/sensors/ingest",
    tag = "sensors",
    security(("bearer" = [])),
    request_body = IngestRequest,
    responses((status = 200, description = "Reading ingested"))
)]
pub async fn ingest_reading(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<IngestRequest>,
) -> Result<Json<serde_json::Value>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }

    let reading = SensorReading {
        sensor_id: req.sensor_id,
        value: req.value,
        timestamp: Utc::now(),
        quality: req.quality,
    };
    state
        .sensors
        .ingest_from_source(reading.clone(), "api")
        .await?;
    state.event_bus.publish(ServerEvent::SensorDataIngested {
        sensor_id: reading.sensor_id.clone(),
        source: "api".to_string(),
        value: reading.value.clone(),
    });
    Ok(Json(serde_json::json!({ "ingested": true })))
}

// ── Batch ingest ────────────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/v1/sensors/ingest/batch",
    tag = "sensors",
    security(("bearer" = [])),
    request_body = [IngestRequest],
    responses((status = 200, description = "Readings ingested"))
)]
pub async fn ingest_batch(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(readings): Json<Vec<IngestRequest>>,
) -> Result<Json<serde_json::Value>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }

    let count = readings.len();
    for req in readings {
        let reading = SensorReading {
            sensor_id: req.sensor_id,
            value: req.value,
            timestamp: Utc::now(),
            quality: req.quality,
        };
        state
            .sensors
            .ingest_from_source(reading.clone(), "api")
            .await?;
        state.event_bus.publish(ServerEvent::SensorDataIngested {
            sensor_id: reading.sensor_id.clone(),
            source: "api".to_string(),
            value: reading.value.clone(),
        });
    }
    Ok(Json(serde_json::json!({ "ingested": count })))
}

// ── Get latest reading ──────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/v1/sensors/{sensor_id}/latest",
    tag = "sensors",
    security(("bearer" = [])),
    params(("sensor_id" = String, Path, description = "Sensor id")),
    responses((status = 200, description = "Latest sensor reading", body = Option<SensorReading>))
)]
pub async fn get_latest(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(sensor_id): Path<String>,
) -> Result<Json<Option<SensorReading>>> {
    let reading = state.sensors.latest(&sensor_id).await?;
    Ok(Json(reading))
}

// ── Query readings ──────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/v1/sensors/query",
    tag = "sensors",
    security(("bearer" = [])),
    params(SensorQuery),
    responses((status = 200, description = "Query sensor readings", body = [SensorReading]))
)]
pub async fn query_readings(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(q): Query<SensorQuery>,
) -> Result<Json<Vec<SensorReading>>> {
    let readings = state.sensors.query(&q).await?;
    Ok(Json(readings))
}

// ── Routes ──────────────────────────────────────────────────────────────

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sensors", get(list_sensors).post(register_sensor))
        .route(
            "/sensors/{sensor_id}",
            get(get_sensor).delete(unregister_sensor),
        )
        .route("/sensors/{sensor_id}/latest", get(get_latest))
        .route("/sensors/ingest", post(ingest_reading))
        .route("/sensors/ingest/batch", post(ingest_batch))
        .route("/sensors/query", get(query_readings))
}
