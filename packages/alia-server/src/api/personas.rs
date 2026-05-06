use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, Result};
use crate::state::AppState;

#[derive(Serialize)]
pub struct PersonaInfo {
    pub id: Uuid,
    pub name: String,
    pub is_system: bool,
    pub greeting: Option<String>,
}

async fn list_personas(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<PersonaInfo>>> {
    Ok(Json(Vec::new()))
}

async fn get_persona(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<PersonaInfo>> {
    Err(AppError::NotFound("Persona not found".into()))
}

async fn create_persona(
    State(_state): State<AppState>,
    auth: AuthUser,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<PersonaInfo>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    Err(AppError::Internal(anyhow::anyhow!("Not implemented")))
}

async fn update_persona(
    State(_state): State<AppState>,
    auth: AuthUser,
    Path(_id): Path<Uuid>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<PersonaInfo>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    Err(AppError::NotFound("Persona not found".into()))
}

async fn delete_persona(
    State(_state): State<AppState>,
    auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(Json(serde_json::json!({"success": true})))
}

async fn get_my_persona(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({})))
}

async fn update_my_persona(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({"success": true})))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/personas", get(list_personas).post(create_persona))
        .route("/personas/me", get(get_my_persona).patch(update_my_persona))
        .route("/personas/{id}", get(get_persona).put(update_persona).delete(delete_persona))
}
