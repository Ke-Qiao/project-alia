use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Set,
    TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{EMAIL_IDENTITY_PROVIDER, hash_password, verify_password};
use crate::entity::{refresh_token, user, user_identity_link};
use crate::error::{AppError, AuthFailure, ErrorResponse, Result};
use crate::state::AppState;

const ROLE_ADMIN: &str = "admin";
const ROLE_USER: &str = "user";
const STATUS_ACTIVE: &str = "active";

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
    pub email: String,
    pub name: String,
    pub role: String,
    pub status: String,
}

#[derive(Serialize, ToSchema)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
}

fn normalize_email(email: &str) -> Result<String> {
    let email = email.trim().to_ascii_lowercase();
    if email.len() > 254 || !email.contains('@') {
        return Err(AppError::Validation("invalid email address".into()));
    }
    Ok(email)
}

fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 120 {
        return Err(AppError::Validation(
            "name must be between 1 and 120 characters".into(),
        ));
    }
    Ok(name.to_string())
}

fn validate_password(password: &str) -> Result<()> {
    if password.len() < 8 {
        return Err(AppError::Validation(
            "password must be at least 8 characters".into(),
        ));
    }
    Ok(())
}

fn user_info(user: &user::Model) -> UserInfo {
    UserInfo {
        id: user.id.to_string(),
        email: user.email.clone(),
        name: user.name.clone(),
        role: user.role.clone(),
        status: user.status.clone(),
    }
}

fn random_refresh_token() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

fn hash_refresh_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn request_user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(512).collect())
}

async fn issue_refresh_token(
    state: &AppState,
    db: &impl ConnectionTrait,
    user_id: Uuid,
    headers: &HeaderMap,
) -> Result<String> {
    let refresh = random_refresh_token();
    let now = Utc::now();
    let expires_at = now + Duration::days(state.config.auth.refresh_expiry_days as i64);

    refresh_token::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        token_hash: Set(hash_refresh_token(&refresh)),
        user_agent: Set(request_user_agent(headers)),
        ip_address: Set(None),
        issued_at: Set(now.into()),
        expires_at: Set(expires_at.into()),
        revoked_at: Set(None),
    }
    .insert(db)
    .await?;

    Ok(refresh)
}

async fn issue_auth_response(
    state: &AppState,
    db: &impl ConnectionTrait,
    user: &user::Model,
    headers: &HeaderMap,
) -> Result<AuthResponse> {
    let access_token = state
        .jwt_keys
        .encode(
            user.id,
            &user.email,
            &user.name,
            &user.role,
            state.config.auth.jwt_expiry_hours,
        )
        .map_err(AppError::Internal)?;
    let refresh_token = issue_refresh_token(state, db, user.id, headers).await?;

    Ok(AuthResponse {
        access_token,
        refresh_token,
        user: user_info(user),
    })
}

async fn find_active_user_by_email(state: &AppState, email: &str) -> Result<Option<user::Model>> {
    Ok(user::Entity::find()
        .filter(user::Column::Email.eq(email))
        .filter(user::Column::Status.eq(STATUS_ACTIVE))
        .one(&state.db)
        .await?)
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = AuthResponse),
        (status = 400, description = "Invalid email address: JSON error; malformed JSON or body read failure: plain text", content((ErrorResponse = "application/json"), (String = "text/plain"))),
        (status = 401, description = "Invalid credentials", body = ErrorResponse),
        (status = 413, description = "JSON body exceeds the 2 MiB limit", body = String, content_type = "text/plain"),
        (status = 415, description = "Missing or unsupported JSON Content-Type", body = String, content_type = "text/plain"),
        (status = 422, description = "JSON fields are missing or have invalid types", body = String, content_type = "text/plain"),
        (status = 500, description = "Account storage or token issuance failed", body = ErrorResponse)
    )
)]
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Json<AuthResponse>> {
    let email = normalize_email(&req.email)?;
    let Some(user) = find_active_user_by_email(&state, &email).await? else {
        return Err(AppError::Unauthenticated(AuthFailure::InvalidCredentials));
    };

    let password_ok = verify_password(&req.password, &user.password_hash)
        .await
        .map_err(AppError::Internal)?;
    if !password_ok {
        return Err(AppError::Unauthenticated(AuthFailure::InvalidCredentials));
    }

    let now = Utc::now();
    let mut active: user::ActiveModel = user.clone().into();
    active.last_login_at = Set(Some(now.into()));
    active.updated_at = Set(now.into());
    let user = active.update(&state.db).await?;

    Ok(Json(
        issue_auth_response(&state, &state.db, &user, &headers).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/register",
    tag = "auth",
    request_body = RegisterRequest,
    responses(
        (status = 200, description = "Registration successful", body = AuthResponse),
        (status = 400, description = "Invalid email, name or password: JSON error; malformed JSON or body read failure: plain text", content((ErrorResponse = "application/json"), (String = "text/plain"))),
        (status = 409, description = "Email already registered", body = ErrorResponse),
        (status = 413, description = "JSON body exceeds the 2 MiB limit", body = String, content_type = "text/plain"),
        (status = 415, description = "Missing or unsupported JSON Content-Type", body = String, content_type = "text/plain"),
        (status = 422, description = "JSON fields are missing or have invalid types", body = String, content_type = "text/plain"),
        (status = 500, description = "Registration storage or token issuance failed", body = ErrorResponse)
    )
)]
pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>> {
    let email = normalize_email(&req.email)?;
    let name = validate_name(&req.name)?;
    validate_password(&req.password)?;
    let password_hash = hash_password(&req.password)
        .await
        .map_err(AppError::Internal)?;

    let tx = state.db.begin().await?;
    // The empty users table has no row to lock for first-administrator election.
    tx.execute_unprepared("LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE")
        .await?;

    if user::Entity::find()
        .filter(user::Column::Email.eq(&email))
        .one(&tx)
        .await?
        .is_some()
    {
        return Err(AppError::EmailAlreadyRegistered);
    }

    let existing_users = user::Entity::find().count(&tx).await?;
    let role = if existing_users == 0 {
        ROLE_ADMIN
    } else {
        ROLE_USER
    };
    let now = Utc::now();
    let user_id = Uuid::new_v4();
    let new_user = user::ActiveModel {
        id: Set(user_id),
        email: Set(email.clone()),
        name: Set(name),
        password_hash: Set(password_hash),
        role: Set(role.to_string()),
        status: Set(STATUS_ACTIVE.to_string()),
        last_login_at: Set(Some(now.into())),
        created_at: Set(now.into()),
        updated_at: Set(now.into()),
    }
    .insert(&tx)
    .await?;

    user_identity_link::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        provider: Set(EMAIL_IDENTITY_PROVIDER.to_string()),
        subject: Set(email),
        label: Set(Some("Primary email".to_string())),
        confidence_threshold: Set(None),
        metadata: Set(serde_json::json!({})),
        verified_at: Set(Some(now.into())),
        created_at: Set(now.into()),
        updated_at: Set(now.into()),
    }
    .insert(&tx)
    .await?;

    let response = issue_auth_response(&state, &tx, &new_user, &headers).await?;
    tx.commit().await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    operation_id = "refresh_token",
    tag = "auth",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "Token refreshed", body = TokenResponse),
        (status = 400, description = "Malformed JSON or body read failure", body = String, content_type = "text/plain"),
        (status = 401, description = "Invalid refresh token or inactive account", body = ErrorResponse),
        (status = 413, description = "JSON body exceeds the 2 MiB limit", body = String, content_type = "text/plain"),
        (status = 415, description = "Missing or unsupported JSON Content-Type", body = String, content_type = "text/plain"),
        (status = 422, description = "JSON fields are missing or have invalid types", body = String, content_type = "text/plain"),
        (status = 500, description = "Token storage or issuance failed", body = ErrorResponse)
    )
)]
pub async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<TokenResponse>> {
    let token_hash = hash_refresh_token(req.refresh_token.trim());
    let tx = state.db.begin().await?;
    let Some(stored) = refresh_token::Entity::update_many()
        .col_expr(
            refresh_token::Column::RevokedAt,
            Expr::current_timestamp().into(),
        )
        .filter(refresh_token::Column::TokenHash.eq(token_hash))
        .filter(refresh_token::Column::RevokedAt.is_null())
        .filter(Expr::col(refresh_token::Column::ExpiresAt).gt(Expr::cust("clock_timestamp()")))
        .exec_with_returning(&tx)
        .await?
        .pop()
    else {
        return Err(AppError::Unauthenticated(AuthFailure::RefreshTokenInvalid));
    };

    let Some(user) = user::Entity::find_by_id(stored.user_id).one(&tx).await? else {
        return Err(AppError::Unauthenticated(AuthFailure::RefreshTokenInvalid));
    };
    if user.status != STATUS_ACTIVE {
        return Err(AppError::Unauthenticated(AuthFailure::AccountInactive));
    }

    let access_token = state
        .jwt_keys
        .encode(
            user.id,
            &user.email,
            &user.name,
            &user.role,
            state.config.auth.jwt_expiry_hours,
        )
        .map_err(AppError::Internal)?;
    let refresh_token = issue_refresh_token(&state, &tx, user.id, &headers).await?;
    tx.commit().await?;

    Ok(Json(TokenResponse {
        access_token,
        refresh_token,
    }))
}

#[derive(Serialize, ToSchema)]
pub struct LogoutResult {
    pub success: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    operation_id = "logout",
    tag = "auth",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "Refresh token revoked or already absent", body = LogoutResult),
        (status = 400, description = "Malformed JSON or body read failure", body = String, content_type = "text/plain"),
        (status = 413, description = "JSON body exceeds the 2 MiB limit", body = String, content_type = "text/plain"),
        (status = 415, description = "Missing or unsupported JSON Content-Type", body = String, content_type = "text/plain"),
        (status = 422, description = "JSON fields are missing or have invalid types", body = String, content_type = "text/plain"),
        (status = 500, description = "Token storage unavailable", body = ErrorResponse)
    )
)]
pub async fn logout(
    State(state): State<AppState>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<LogoutResult>> {
    let token_hash = hash_refresh_token(req.refresh_token.trim());
    if let Some(stored) = refresh_token::Entity::find()
        .filter(refresh_token::Column::TokenHash.eq(token_hash))
        .one(&state.db)
        .await?
    {
        if stored.revoked_at.is_none() {
            let mut revoke: refresh_token::ActiveModel = stored.into();
            revoke.revoked_at = Set(Some(Utc::now().into()));
            revoke.update(&state.db).await?;
        }
    }

    Ok(Json(LogoutResult { success: true }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/register", post(register))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
}
