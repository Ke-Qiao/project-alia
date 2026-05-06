use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// Authenticated user extracted from JWT
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub name: String,
    pub role: String,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
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

        let claims = state
            .jwt_keys
            .decode(token)
            .map_err(|_| AppError::Auth("Invalid or expired token".into()))?;

        Ok(AuthUser {
            id: claims.sub,
            name: claims.name,
            role: claims.role,
        })
    }
}

/// Auth middleware for protected routes
pub async fn require_auth(request: Request, next: Next) -> Response {
    // The actual auth check happens in FromRequestParts for AuthUser
    // This middleware can be used for additional logic like rate limiting
    next.run(request).await
}
