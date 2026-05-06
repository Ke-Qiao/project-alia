use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

async fn get_settings(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({})))
}

async fn update_settings(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({"success": true})))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings", get(get_settings).patch(update_settings))
}
