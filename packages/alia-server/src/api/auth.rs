use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{AppError, Result};
use crate::state::AppState;

#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize, ToSchema)]
pub struct RegisterRequest {
    pub email: String,
    pub name: String,
    pub password: String,
}

#[derive(Deserialize, ToSchema)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Serialize, ToSchema)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub user: UserInfo,
}

#[derive(Serialize, ToSchema)]
pub struct UserInfo {
    pub id: String,
    pub name: String,
    pub role: String,
}

#[derive(Serialize, ToSchema)]
pub struct TokenResponse {
    pub access_token: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = AuthResponse),
        (status = 401, description = "Invalid credentials")
    )
)]
async fn login(
    State(_state): State<AppState>,
    Json(_req): Json<LoginRequest>,
) -> Result<Json<AuthResponse>> {
    // TODO: implement login logic
    Err(AppError::Auth("Not implemented".into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/register",
    tag = "auth",
    request_body = RegisterRequest,
    responses(
        (status = 200, description = "Registration successful", body = AuthResponse),
        (status = 400, description = "Validation error")
    )
)]
async fn register(
    State(_state): State<AppState>,
    Json(_req): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>> {
    // TODO: implement registration logic
    Err(AppError::Internal(anyhow::anyhow!("Not implemented")))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    tag = "auth",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "Token refreshed", body = TokenResponse),
        (status = 401, description = "Invalid refresh token")
    )
)]
async fn refresh(
    State(_state): State<AppState>,
    Json(_req): Json<RefreshRequest>,
) -> Result<Json<TokenResponse>> {
    // TODO: implement token refresh
    Err(AppError::Auth("Not implemented".into()))
}

async fn logout(
    State(_state): State<AppState>,
    Json(_req): Json<RefreshRequest>,
) -> Result<Json<serde_json::Value>> {
    // TODO: implement logout (revoke refresh token)
    Ok(Json(serde_json::json!({"success": true})))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/register", post(register))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
}
