use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::modules::execution::types::{EffectRecord, ExecutionError};
use crate::state::AppState;

pub struct ExecutionApiError(ExecutionError);

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionErrorCode {
    IdentityUnlinked,
    Unauthorized,
    NotFound,
    IdempotencyConflict,
    VersionChanged,
    InvalidTransition,
    CompletionConflict,
    CapacityExceeded,
    InvalidInput,
    ExecutionUnavailable,
}

#[derive(Serialize, ToSchema)]
pub struct ExecutionErrorResponse {
    pub error: ExecutionErrorDetail,
}
#[derive(Serialize, ToSchema)]
pub struct ExecutionErrorDetail {
    pub code: ExecutionErrorCode,
    pub message: String,
}

impl From<ExecutionError> for ExecutionApiError {
    fn from(value: ExecutionError) -> Self {
        Self(value)
    }
}
impl From<JsonRejection> for ExecutionApiError {
    fn from(value: JsonRejection) -> Self {
        Self(ExecutionError::InvalidInput(value.body_text()))
    }
}
impl From<QueryRejection> for ExecutionApiError {
    fn from(value: QueryRejection) -> Self {
        Self(ExecutionError::InvalidInput(value.body_text()))
    }
}
impl From<PathRejection> for ExecutionApiError {
    fn from(value: PathRejection) -> Self {
        Self(ExecutionError::InvalidInput(value.body_text()))
    }
}

impl IntoResponse for ExecutionApiError {
    fn into_response(self) -> Response {
        use ExecutionErrorCode as Code;
        let (status, code) = match &self.0 {
            ExecutionError::IdentityUnlinked => (StatusCode::FORBIDDEN, Code::IdentityUnlinked),
            ExecutionError::Unauthorized(_) => (StatusCode::FORBIDDEN, Code::Unauthorized),
            ExecutionError::NotFound => (StatusCode::NOT_FOUND, Code::NotFound),
            ExecutionError::IdempotencyConflict => {
                (StatusCode::CONFLICT, Code::IdempotencyConflict)
            }
            ExecutionError::VersionChanged => (StatusCode::CONFLICT, Code::VersionChanged),
            ExecutionError::InvalidTransition(_) => (StatusCode::CONFLICT, Code::InvalidTransition),
            ExecutionError::CompletionConflict => (StatusCode::CONFLICT, Code::CompletionConflict),
            ExecutionError::CapacityExceeded => {
                (StatusCode::TOO_MANY_REQUESTS, Code::CapacityExceeded)
            }
            ExecutionError::InvalidInput(_) => (StatusCode::BAD_REQUEST, Code::InvalidInput),
            ExecutionError::Database(_) | ExecutionError::Data(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, Code::ExecutionUnavailable)
            }
        };
        let message = if status == StatusCode::SERVICE_UNAVAILABLE {
            tracing::error!(error = %self.0, "execution API unavailable");
            "Execution acknowledgement unavailable. Query the existing effect or retry the same payload with the same request_id.".into()
        } else {
            self.0.to_string()
        };
        (
            status,
            Json(ExecutionErrorResponse {
                error: ExecutionErrorDetail { code, message },
            }),
        )
            .into_response()
    }
}

#[derive(Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Recover an original request even when its tool is no longer installed.
    pub request_id: Option<Uuid>,
    /// Exclusive effect_id cursor from the last record of the previous page.
    pub before: Option<Uuid>,
    /// Page size, clamped to 100. Default: 25.
    #[serde(default = "default_limit")]
    #[param(default = 25, minimum = 1, maximum = 100)]
    pub limit: u32,
}
fn default_limit() -> u32 {
    25
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfirmRequest {
    /// Exact digest returned by the effect to which the owner is consenting.
    pub intent_digest: String,
}

#[utoipa::path(get, path = "/api/v1/effects", tag = "execution", security(("bearer" = [])),
    params(ListQuery), responses(
        (status = 200, description = "Owner's effect records in descending created_at, effect_id order", body = [EffectRecord]),
        (status = 400, description = "Invalid cursor or query", body = ExecutionErrorResponse),
        (status = 401, description = "Authentication required"),
        (status = 503, description = "Execution ledger unavailable", body = ExecutionErrorResponse)
    ))]
pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Vec<EffectRecord>>, ExecutionApiError> {
    let Query(query) = query?;
    if query.limit == 0 {
        return Err(ExecutionError::InvalidInput("limit must be positive".into()).into());
    }
    Ok(Json(
        state
            .execution
            .list(auth.id, query.before, query.limit, query.request_id)
            .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/effects/{id}", tag = "execution", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Effect identity")), responses(
        (status = 200, description = "Current durable effect fact", body = EffectRecord),
        (status = 400, description = "Invalid effect UUID", body = ExecutionErrorResponse),
        (status = 401, description = "Authentication required"),
        (status = 404, description = "Effect absent or owned by another account", body = ExecutionErrorResponse),
        (status = 503, description = "Execution ledger unavailable", body = ExecutionErrorResponse)
    ))]
pub async fn get_effect(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<EffectRecord>, ExecutionApiError> {
    let Path(id) = id?;
    Ok(Json(state.execution.get(auth.id, id).await?))
}

#[utoipa::path(post, path = "/api/v1/effects/{id}/confirm", tag = "execution", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Effect identity")), request_body = ConfirmRequest,
    responses(
        (status = 200, description = "Approval recorded; execution may still be pending", body = EffectRecord),
        (status = 400, description = "Invalid effect UUID or body", body = ExecutionErrorResponse),
        (status = 401, description = "Authentication required"),
        (status = 403, description = "Owner no longer authorized", body = ExecutionErrorResponse),
        (status = 404, description = "Effect absent or owned by another account", body = ExecutionErrorResponse),
        (status = 409, description = "Digest, version or transition conflict", body = ExecutionErrorResponse),
        (status = 429, description = "Dispatch queue full", body = ExecutionErrorResponse),
        (status = 503, description = "Approval acknowledgement unavailable", body = ExecutionErrorResponse)
    ))]
pub async fn confirm(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<ConfirmRequest>, JsonRejection>,
) -> Result<Json<EffectRecord>, ExecutionApiError> {
    let Path(id) = id?;
    let Json(body) = body?;
    Ok(Json(
        state
            .execution
            .confirm(auth.id, id, body.intent_digest)
            .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/effects/{id}/cancel", tag = "execution", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Effect identity")), responses(
        (status = 200, description = "Undispatched effect cancelled", body = EffectRecord),
        (status = 400, description = "Invalid effect UUID", body = ExecutionErrorResponse),
        (status = 401, description = "Authentication required"),
        (status = 404, description = "Effect absent or owned by another account", body = ExecutionErrorResponse),
        (status = 409, description = "Effect has already been dispatched or completed", body = ExecutionErrorResponse),
        (status = 503, description = "Cancellation acknowledgement unavailable", body = ExecutionErrorResponse)
    ))]
pub async fn cancel(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<EffectRecord>, ExecutionApiError> {
    let Path(id) = id?;
    Ok(Json(state.execution.cancel(auth.id, id).await?))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/effects", get(list))
        .route("/effects/{id}", get(get_effect))
        .route("/effects/{id}/confirm", post(confirm))
        .route("/effects/{id}/cancel", post(cancel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::extract::FromRequest;

    #[tokio::test]
    async fn database_errors_are_sanitized_and_invalid_json_uses_the_machine_error_envelope() {
        let response = ExecutionApiError::from(ExecutionError::Database(sea_orm::DbErr::Custom(
            "private database diagnostic".into(),
        )))
        .into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "execution_unavailable");
        assert!(!body.to_string().contains("private database diagnostic"));

        let request = axum::http::Request::builder().header("content-type", "application/json")
            .body(Body::from(r#"{"request_id":"00000000-0000-0000-0000-000000000001","tool_name":"probe","arguments":{},"confirmed":true}"#)).unwrap();
        let rejected = Json::<crate::api::tools::ExecuteToolRequest>::from_request(request, &())
            .await
            .err()
            .unwrap();
        let response = ExecutionApiError::from(rejected).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "invalid_input");
    }
}
