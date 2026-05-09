use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

#[derive(Serialize, Deserialize)]
pub struct AutomationRule {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub trigger_config: serde_json::Value,
    pub conditions: serde_json::Value,
    pub action_config: serde_json::Value,
    pub cooldown_sec: u32,
}

async fn list_rules(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<AutomationRule>>> {
    Ok(Json(Vec::new()))
}

async fn create_rule(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<AutomationRule>> {
    Err(crate::error::AppError::Internal(anyhow::anyhow!(
        "Not implemented"
    )))
}

async fn get_rule(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<AutomationRule>> {
    Err(crate::error::AppError::NotFound("Rule not found".into()))
}

async fn update_rule(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_id): Path<Uuid>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<AutomationRule>> {
    Err(crate::error::AppError::NotFound("Rule not found".into()))
}

async fn delete_rule(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({"success": true})))
}

async fn toggle_rule(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({"success": true})))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/rules", get(list_rules).post(create_rule))
        .route(
            "/rules/{id}",
            get(get_rule).put(update_rule).delete(delete_rule),
        )
        .route("/rules/{id}/toggle", post(toggle_rule))
}
