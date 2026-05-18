use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::entity::user;
use crate::error::{AppError, Result};
use crate::state::AppState;

const ROLE_ADMIN: &str = "admin";
const ROLE_USER: &str = "user";
const STATUS_ACTIVE: &str = "active";
const STATUS_DISABLED: &str = "disabled";
const STATUS_DELETED: &str = "deleted";

#[derive(Serialize, ToSchema)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
    pub status: String,
    pub last_login_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateUserRequest {
    pub name: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
}

fn to_response(user: user::Model) -> UserResponse {
    UserResponse {
        id: user.id,
        email: user.email,
        name: user.name,
        role: user.role,
        status: user.status,
        last_login_at: user.last_login_at.map(|value| value.to_rfc3339()),
        created_at: user.created_at.to_rfc3339(),
        updated_at: user.updated_at.to_rfc3339(),
    }
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

fn validate_role(role: &str) -> Result<String> {
    match role.trim() {
        ROLE_ADMIN => Ok(ROLE_ADMIN.to_string()),
        ROLE_USER => Ok(ROLE_USER.to_string()),
        _ => Err(AppError::Validation("invalid user role".into())),
    }
}

fn validate_status(status: &str) -> Result<String> {
    match status.trim() {
        STATUS_ACTIVE => Ok(STATUS_ACTIVE.to_string()),
        STATUS_DISABLED => Ok(STATUS_DISABLED.to_string()),
        STATUS_DELETED => Ok(STATUS_DELETED.to_string()),
        _ => Err(AppError::Validation("invalid user status".into())),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/users/me",
    tag = "users",
    security(("bearer" = [])),
    responses((status = 200, description = "Current user", body = UserResponse))
)]
pub async fn get_me(State(state): State<AppState>, auth: AuthUser) -> Result<Json<UserResponse>> {
    let Some(user) = user::Entity::find_by_id(auth.id).one(&state.db).await? else {
        return Err(AppError::Auth("authenticated user no longer exists".into()));
    };
    Ok(Json(to_response(user)))
}

#[utoipa::path(
    get,
    path = "/api/v1/users",
    tag = "users",
    security(("bearer" = [])),
    responses((status = 200, description = "List users", body = [UserResponse]))
)]
pub async fn list_users(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<UserResponse>>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }

    let users = user::Entity::find()
        .filter(user::Column::Status.ne(STATUS_DELETED))
        .order_by_asc(user::Column::CreatedAt)
        .all(&state.db)
        .await?
        .into_iter()
        .map(to_response)
        .collect();

    Ok(Json(users))
}

#[utoipa::path(
    get,
    path = "/api/v1/users/{id}",
    tag = "users",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "User id")),
    responses((status = 200, description = "User detail", body = UserResponse))
)]
pub async fn get_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<UserResponse>> {
    if !auth.is_admin() && auth.id != id {
        return Err(AppError::Forbidden);
    }

    let Some(user) = user::Entity::find_by_id(id)
        .filter(user::Column::Status.ne(STATUS_DELETED))
        .one(&state.db)
        .await?
    else {
        return Err(AppError::NotFound("user not found".into()));
    };

    Ok(Json(to_response(user)))
}

#[utoipa::path(
    patch,
    path = "/api/v1/users/{id}",
    tag = "users",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "User id")),
    request_body = UpdateUserRequest,
    responses((status = 200, description = "Updated user", body = UserResponse))
)]
pub async fn update_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>> {
    if !auth.is_admin() && auth.id != id {
        return Err(AppError::Forbidden);
    }
    if !auth.is_admin() && (body.role.is_some() || body.status.is_some()) {
        return Err(AppError::Forbidden);
    }

    let Some(model) = user::Entity::find_by_id(id)
        .filter(user::Column::Status.ne(STATUS_DELETED))
        .one(&state.db)
        .await?
    else {
        return Err(AppError::NotFound("user not found".into()));
    };

    let mut active: user::ActiveModel = model.into();
    if let Some(name) = body.name {
        active.name = Set(validate_name(&name)?);
    }
    if let Some(role) = body.role {
        active.role = Set(validate_role(&role)?);
    }
    if let Some(status) = body.status {
        active.status = Set(validate_status(&status)?);
    }
    active.updated_at = Set(Utc::now().into());

    let updated = active.update(&state.db).await?;
    Ok(Json(to_response(updated)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/users/{id}",
    tag = "users",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "User id")),
    responses((status = 200, description = "User deleted"))
)]
pub async fn delete_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    if auth.id == id {
        return Err(AppError::Validation(
            "cannot delete the currently authenticated user".into(),
        ));
    }

    let Some(model) = user::Entity::find_by_id(id)
        .filter(user::Column::Status.ne(STATUS_DELETED))
        .one(&state.db)
        .await?
    else {
        return Err(AppError::NotFound("user not found".into()));
    };

    let mut active: user::ActiveModel = model.into();
    active.status = Set(STATUS_DELETED.to_string());
    active.updated_at = Set(Utc::now().into());
    active.update(&state.db).await?;

    Ok(Json(serde_json::json!({"success": true})))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users/me", get(get_me).patch(update_user_me))
        .route("/users", get(list_users))
        .route(
            "/users/{id}",
            get(get_user).patch(update_user).delete(delete_user),
        )
}

async fn update_user_me(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>> {
    update_user(State(state), auth.clone(), Path(auth.id), Json(body)).await
}
