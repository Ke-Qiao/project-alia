use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, Result};
use crate::state::AppState;

#[derive(Serialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
}

async fn list_users(State(_state): State<AppState>, auth: AuthUser) -> Result<Json<Vec<User>>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    // TODO: query database
    Ok(Json(Vec::new()))
}

async fn get_user(
    State(_state): State<AppState>,
    auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<User>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    Err(AppError::NotFound("User not found".into()))
}

async fn update_user(
    State(_state): State<AppState>,
    auth: AuthUser,
    Path(_id): Path<Uuid>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<User>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    Err(AppError::NotFound("User not found".into()))
}

async fn delete_user(
    State(_state): State<AppState>,
    auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(Json(serde_json::json!({"success": true})))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/users", get(list_users)).route(
        "/users/{id}",
        get(get_user).patch(update_user).delete(delete_user),
    )
}
