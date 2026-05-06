use axum::{Json, Router};
use axum::routing::get;
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/health", get(health_check))
}
