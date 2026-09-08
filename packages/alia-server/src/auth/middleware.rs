use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use sea_orm::EntityTrait;
use uuid::Uuid;

use crate::entity::user;
use crate::error::AppError;
use crate::state::AppState;

/// Authenticated user extracted from JWT
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }

    /// Resolve current account state; JWT role claims alone are not authority.
    pub async fn from_token(state: &AppState, token: &str) -> Result<Self, AppError> {
        if token.len() > 8192 {
            return Err(AppError::Auth("Invalid token".into()));
        }
        let claims = state
            .jwt_keys
            .decode(token)
            .map_err(|_| AppError::Auth("Invalid or expired token".into()))?;
        let user = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            user::Entity::find_by_id(claims.sub).one(&state.db),
        )
        .await
        .map_err(|_| AppError::Auth("Account verification unavailable".into()))??
        .ok_or_else(|| AppError::Auth("Authenticated user no longer exists".into()))?;
        if user.status != "active" {
            return Err(AppError::Auth("Authenticated user is not active".into()));
        }
        Ok(Self {
            id: user.id,
            email: user.email,
            name: user.name,
            role: user.role,
        })
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::Auth("Missing authorization header".into()))?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AppError::Auth("Invalid authorization format".into()))?;

        Self::from_token(state, token).await
    }
}

/// Auth middleware for protected routes
pub async fn require_auth(request: Request, next: Next) -> Response {
    // The actual auth check happens in FromRequestParts for AuthUser
    // This middleware can be used for additional logic like rate limiting
    next.run(request).await
}
