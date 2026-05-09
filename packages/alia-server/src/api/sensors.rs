use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::providers::sensor::{SensorDefinition, SensorQuery, SensorReading, SensorSummary};
use crate::state::AppState;

// ── List all sensors ────────────────────────────────────────────────────

async fn list_sensors(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<SensorSummary>>> {
    let sensors = state.sensors.list_sensors().await;
    Ok(Json(sensors))
}

// ── Get single sensor definition ────────────────────────────────────────

async fn get_sensor(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(sensor_id): Path<String>,
) -> Result<Json<Option<SensorDefinition>>> {
    let def = state.sensors.get_sensor(&sensor_id).await;
    Ok(Json(def))
}

// ── Register a sensor ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct RegisterSensorRequest {
    sensor_id: String,
    display_name: String,
    source: String,
    #[serde(default)]
    unit: String,
    #[serde(default = "default_value_type")]
    value_type: String,
    #[serde(default)]
    metadata: serde_json::Value,
}

fn default_value_type() -> String {
    "float".into()
}

async fn register_sensor(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(req): Json<RegisterSensorRequest>,
) -> Result<Json<SensorDefinition>> {
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
    state.sensors.register_sensor(def.clone()).await;
    Ok(Json(def))
}

// ── Unregister a sensor ─────────────────────────────────────────────────

async fn unregister_sensor(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(sensor_id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let removed = state.sensors.unregister_sensor(&sensor_id).await;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

// ── Ingest readings ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct IngestRequest {
    sensor_id: String,
    value: serde_json::Value,
    #[serde(default = "default_quality")]
    quality: f32,
}

fn default_quality() -> f32 {
    1.0
}

async fn ingest_reading(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(req): Json<IngestRequest>,
) -> Result<Json<serde_json::Value>> {
    let reading = SensorReading {
        sensor_id: req.sensor_id,
        value: req.value,
        timestamp: Utc::now(),
        quality: req.quality,
    };
    state.sensors.ingest(reading).await;
    Ok(Json(serde_json::json!({ "ingested": true })))
}

// ── Batch ingest ────────────────────────────────────────────────────────

async fn ingest_batch(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(readings): Json<Vec<IngestRequest>>,
) -> Result<Json<serde_json::Value>> {
    let count = readings.len();
    for req in readings {
        let reading = SensorReading {
            sensor_id: req.sensor_id,
            value: req.value,
            timestamp: Utc::now(),
            quality: req.quality,
        };
        state.sensors.ingest(reading).await;
    }
    Ok(Json(serde_json::json!({ "ingested": count })))
}

// ── Get latest reading ──────────────────────────────────────────────────

async fn get_latest(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(sensor_id): Path<String>,
) -> Result<Json<Option<SensorReading>>> {
    let reading = state.sensors.latest(&sensor_id).await;
    Ok(Json(reading))
}

// ── Query readings ──────────────────────────────────────────────────────

async fn query_readings(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(q): Query<SensorQuery>,
) -> Result<Json<Vec<SensorReading>>> {
    let readings = state.sensors.query(&q).await;
    Ok(Json(readings))
}

// ── Routes ──────────────────────────────────────────────────────────────

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sensors", get(list_sensors).post(register_sensor))
        .route("/sensors/{sensor_id}", get(get_sensor).delete(unregister_sensor))
        .route("/sensors/{sensor_id}/latest", get(get_latest))
        .route("/sensors/ingest", post(ingest_reading))
        .route("/sensors/ingest/batch", post(ingest_batch))
        .route("/sensors/query", get(query_readings))
}
