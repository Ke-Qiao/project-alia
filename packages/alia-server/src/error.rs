use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailure {
    InvalidCredentials,
    TokenMissing,
    TokenInvalid,
    AccountInactive,
    RefreshTokenInvalid,
}

impl AuthFailure {
    fn reason(self) -> ErrorReason {
        match self {
            Self::InvalidCredentials => ErrorReason::InvalidCredentials,
            Self::TokenMissing => ErrorReason::TokenMissing,
            Self::TokenInvalid => ErrorReason::TokenInvalid,
            Self::AccountInactive => ErrorReason::AccountInactive,
            Self::RefreshTokenInvalid => ErrorReason::RefreshTokenInvalid,
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::InvalidCredentials => "invalid email or password",
            Self::TokenMissing => "missing or malformed authorization header",
            Self::TokenInvalid => "invalid or expired token",
            Self::AccountInactive => "account does not exist or is not active",
            Self::RefreshTokenInvalid => "invalid refresh token",
        }
    }
}

impl std::fmt::Display for AuthFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// Application-level error type.
#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("authentication failed: {0}")]
    Unauthenticated(AuthFailure),
    #[error("account verification unavailable")]
    AccountVerificationUnavailable,
    #[error("email already registered")]
    EmailAlreadyRegistered,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    AuthInvalidCredentials,
    AuthForbidden,
    NotFound,
    ValidationError,
    VersionConflict,
    AlreadyExists,
    ServiceUnavailable,
    RateLimited,
    InternalDbError,
    InternalGrpcError,
    AiServiceError,
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorReason {
    InvalidCredentials,
    TokenMissing,
    TokenInvalid,
    AccountInactive,
    RefreshTokenInvalid,
    AccountVerificationUnavailable,
    EmailAlreadyRegistered,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorDetail {
    pub code: ErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<ErrorReason>,
    pub message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, reason, message) = match &self {
            Self::Unauthenticated(failure) => (
                StatusCode::UNAUTHORIZED,
                ErrorCode::AuthInvalidCredentials,
                Some(failure.reason()),
                failure.message().into(),
            ),
            Self::AccountVerificationUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::ServiceUnavailable,
                Some(ErrorReason::AccountVerificationUnavailable),
                "account verification unavailable".into(),
            ),
            Self::EmailAlreadyRegistered => (
                StatusCode::CONFLICT,
                ErrorCode::AlreadyExists,
                Some(ErrorReason::EmailAlreadyRegistered),
                "email already registered".into(),
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                ErrorCode::AuthForbidden,
                None,
                "insufficient permissions".into(),
            ),
            Self::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                None,
                msg.clone(),
            ),
            Self::Validation(msg) => (
                StatusCode::BAD_REQUEST,
                ErrorCode::ValidationError,
                None,
                msg.clone(),
            ),
            Self::Conflict(msg) => (
                StatusCode::CONFLICT,
                ErrorCode::VersionConflict,
                None,
                msg.clone(),
            ),
            Self::Unavailable(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::ServiceUnavailable,
                None,
                (*msg).into(),
            ),
            Self::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                ErrorCode::RateLimited,
                None,
                "too many requests".into(),
            ),
            Self::Database(e) => {
                tracing::error!(error = ?e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorCode::InternalDbError,
                    None,
                    "internal error".into(),
                )
            }
            #[cfg(feature = "channel-embedded")]
            Self::Grpc(e) => {
                tracing::error!(error = ?e, "gRPC error");
                (
                    StatusCode::BAD_GATEWAY,
                    ErrorCode::InternalGrpcError,
                    None,
                    "device communication failed".into(),
                )
            }
            Self::AiService(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::AiServiceError,
                None,
                msg.clone(),
            ),
            Self::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorCode::InternalError,
                    None,
                    "internal error".into(),
                )
            }
        };
        (
            status,
            axum::Json(ErrorResponse {
                error: ErrorDetail {
                    code,
                    reason,
                    message,
                },
            }),
        )
            .into_response()
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

pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    #[tokio::test]
    async fn app_errors_publish_status_code_reason_and_safe_message() {
        let cases = [
            (
                AppError::Forbidden,
                403,
                "AUTH_FORBIDDEN",
                None,
                "insufficient permissions",
            ),
            (
                AppError::NotFound("missing".into()),
                404,
                "NOT_FOUND",
                None,
                "missing",
            ),
            (
                AppError::Validation("invalid".into()),
                400,
                "VALIDATION_ERROR",
                None,
                "invalid",
            ),
            (
                AppError::Conflict("changed".into()),
                409,
                "VERSION_CONFLICT",
                None,
                "changed",
            ),
            (
                AppError::Unavailable("unavailable"),
                503,
                "SERVICE_UNAVAILABLE",
                None,
                "unavailable",
            ),
            (
                AppError::RateLimited,
                429,
                "RATE_LIMITED",
                None,
                "too many requests",
            ),
            (
                AppError::Database(sea_orm::DbErr::Custom("private database diagnostic".into())),
                500,
                "INTERNAL_DB_ERROR",
                None,
                "internal error",
            ),
            (
                AppError::AiService("AI unavailable".into()),
                503,
                "AI_SERVICE_ERROR",
                None,
                "AI unavailable",
            ),
            (
                AppError::Internal(anyhow::anyhow!("private internal diagnostic")),
                500,
                "INTERNAL_ERROR",
                None,
                "internal error",
            ),
            (
                AppError::AccountVerificationUnavailable,
                503,
                "SERVICE_UNAVAILABLE",
                Some("account_verification_unavailable"),
                "account verification unavailable",
            ),
            (
                AppError::EmailAlreadyRegistered,
                409,
                "ALREADY_EXISTS",
                Some("email_already_registered"),
                "email already registered",
            ),
            #[cfg(feature = "channel-embedded")]
            (
                AppError::Grpc(tonic::Status::internal("private grpc diagnostic")),
                502,
                "INTERNAL_GRPC_ERROR",
                None,
                "device communication failed",
            ),
        ];
        for (error, status, code, reason, message) in cases {
            assert_response(error, status, code, reason, message).await;
        }
        for (failure, reason, message) in [
            (
                AuthFailure::InvalidCredentials,
                "invalid_credentials",
                "invalid email or password",
            ),
            (
                AuthFailure::TokenMissing,
                "token_missing",
                "missing or malformed authorization header",
            ),
            (
                AuthFailure::TokenInvalid,
                "token_invalid",
                "invalid or expired token",
            ),
            (
                AuthFailure::AccountInactive,
                "account_inactive",
                "account does not exist or is not active",
            ),
            (
                AuthFailure::RefreshTokenInvalid,
                "refresh_token_invalid",
                "invalid refresh token",
            ),
        ] {
            assert_response(
                AppError::Unauthenticated(failure),
                401,
                "AUTH_INVALID_CREDENTIALS",
                Some(reason),
                message,
            )
            .await;
        }
    }

    async fn assert_response(
        error: AppError,
        status: u16,
        code: &str,
        reason: Option<&str>,
        message: &str,
    ) {
        let response = error.into_response();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(response.headers()["content-type"], "application/json");
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
        assert_eq!(body.as_object().unwrap().len(), 1);
        let detail = &body["error"];
        assert_eq!(detail["code"], code);
        assert_eq!(detail["message"], message);
        match reason {
            Some(reason) => assert_eq!(detail["reason"], reason),
            None => assert!(detail.get("reason").is_none()),
        }
        assert_eq!(
            detail.as_object().unwrap().len(),
            if reason.is_some() { 3 } else { 2 }
        );
        assert!(!body.to_string().contains("private"));
    }
}
