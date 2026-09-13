use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Application-level error type
#[derive(thiserror::Error, Debug)]
pub enum AppError {
    // --- User-visible errors ---
    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("forbidden")]
    Forbidden,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("service unavailable: {0}")]
    Unavailable(&'static str),

    #[error("rate limited")]
    RateLimited,

    // --- Internal errors (details not exposed to users) ---
    #[error("database error")]
    Database(#[source] sea_orm::DbErr),

    #[cfg(feature = "channel-embedded")]
    #[error("gRPC communication error")]
    Grpc(#[source] tonic::Status),

    #[error("AI service error: {0}")]
    AiService(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            Self::Auth(msg) => (
                StatusCode::UNAUTHORIZED,
                "AUTH_INVALID_CREDENTIALS",
                msg.clone(),
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "AUTH_FORBIDDEN",
                "insufficient permissions".into(),
            ),
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, "NOT_FOUND", msg.clone()),
            Self::Validation(msg) => (StatusCode::BAD_REQUEST, "VALIDATION_ERROR", msg.clone()),
            Self::Conflict(msg) => (StatusCode::CONFLICT, "VERSION_CONFLICT", msg.clone()),
            Self::Unavailable(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "SERVICE_UNAVAILABLE",
                (*msg).into(),
            ),
            Self::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "RATE_LIMITED",
                "too many requests".into(),
            ),
            Self::Database(e) => {
                tracing::error!(error = ?e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "INTERNAL_DB_ERROR",
                    "internal error".into(),
                )
            }
            #[cfg(feature = "channel-embedded")]
            Self::Grpc(e) => {
                tracing::error!(error = ?e, "gRPC error");
                (
                    StatusCode::BAD_GATEWAY,
                    "INTERNAL_GRPC_ERROR",
                    "device communication failed".into(),
                )
            }
            Self::AiService(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "AI_SERVICE_ERROR",
                msg.clone(),
            ),
            Self::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "INTERNAL_ERROR",
                    "internal error".into(),
                )
            }
        };

        let body = ErrorBody {
            error: ErrorDetail { code, message },
        };
        (status, axum::Json(body)).into_response()
    }
}

impl From<sea_orm::DbErr> for AppError {
    fn from(err: sea_orm::DbErr) -> Self {
        Self::Database(err)
    }
}

#[cfg(feature = "channel-embedded")]
impl From<tonic::Status> for AppError {
    fn from(err: tonic::Status) -> Self {
        Self::Grpc(err)
    }
}

/// Handler return type alias
pub type Result<T> = std::result::Result<T, AppError>;
