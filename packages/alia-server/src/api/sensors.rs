use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

#[derive(Serialize)]
pub struct SensorData {
    pub plugin_name: String,
    pub metric_name: String,
    pub value: f64,
    pub recorded_at: String,
}

async fn get_sensor_data(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_plugin_name): Path<String>,
) -> Result<Json<Vec<SensorData>>> {
    Ok(Json(Vec::new()))
}

async fn get_sensor_latest(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_plugin_name): Path<String>,
) -> Result<Json<Option<SensorData>>> {
    Ok(Json(None))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sensors/{plugin_name}/data", get(get_sensor_data))
        .route("/sensors/{plugin_name}/latest", get(get_sensor_latest))
}
