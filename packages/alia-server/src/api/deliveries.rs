use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::modules::delivery::types::{
    DeliveryError, DeliveryEvent, DeliveryRecord, ResponseRecord,
};
use crate::state::AppState;

pub struct DeliveryApiError(DeliveryError);

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryErrorCode {
    InvalidInput,
    Unauthorized,
    NotFound,
    Conflict,
    InvalidEvidence,
    CapacityExceeded,
    DeliveryUnavailable,
}

#[derive(Serialize, ToSchema)]
pub struct DeliveryErrorResponse {
    pub error: DeliveryErrorDetail,
}

#[derive(Serialize, ToSchema)]
pub struct DeliveryErrorDetail {
    pub code: DeliveryErrorCode,
    pub message: String,
}

impl From<DeliveryError> for DeliveryApiError {
    fn from(error: DeliveryError) -> Self {
        Self(error)
    }
}

impl From<PathRejection> for DeliveryApiError {
    fn from(error: PathRejection) -> Self {
        Self(DeliveryError::InvalidInput(error.body_text()))
    }
}

impl From<QueryRejection> for DeliveryApiError {
    fn from(error: QueryRejection) -> Self {
        Self(DeliveryError::InvalidInput(error.body_text()))
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ResponseEventsQuery {
    /// Return events strictly after this event ID; defaults to zero.
    pub after: Option<i64>,
    /// Page size from 1 to 256; defaults to 64.
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct TextChunksQuery {
    /// Return immutable chunks strictly after this zero-based sequence.
    pub after: Option<u64>,
    /// Page size from 1 to 256; defaults to 64.
    pub limit: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct TextChunkRecord {
    pub sequence: u64,
    pub utf8_offset: u64,
    pub text: String,
}

impl IntoResponse for DeliveryApiError {
    fn into_response(self) -> Response {
        use DeliveryErrorCode as Code;
        let (status, code) = match &self.0 {
            DeliveryError::InvalidInput(_) => (StatusCode::BAD_REQUEST, Code::InvalidInput),
            DeliveryError::Unauthorized(_) => (StatusCode::FORBIDDEN, Code::Unauthorized),
            DeliveryError::NotFound => (StatusCode::NOT_FOUND, Code::NotFound),
            DeliveryError::IdempotencyConflict | DeliveryError::InvalidTransition(_) => {
                (StatusCode::CONFLICT, Code::Conflict)
            }
            DeliveryError::InvalidEvidence(_) => (StatusCode::CONFLICT, Code::InvalidEvidence),
            DeliveryError::CapacityExceeded => {
                (StatusCode::TOO_MANY_REQUESTS, Code::CapacityExceeded)
            }
            DeliveryError::Database(_) | DeliveryError::Data(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, Code::DeliveryUnavailable)
            }
        };
        let message = if status == StatusCode::SERVICE_UNAVAILABLE {
            tracing::error!(error = %self.0, "delivery API unavailable");
            "Delivery acknowledgement unavailable. Query the existing response or delivery; do not replay output to infer its outcome.".to_string()
        } else {
            self.0.to_string()
        };
        (
            status,
            Json(DeliveryErrorResponse {
                error: DeliveryErrorDetail { code, message },
            }),
        )
            .into_response()
    }
}

#[utoipa::path(get, path = "/api/v1/responses/{id}", tag = "delivery", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Canonical response identity")), responses(
        (status = 200, description = "Owner's response generation fact, independent of playback", body = ResponseRecord),
        (status = 400, description = "Invalid UUID", body = DeliveryErrorResponse),
        (status = 401, description = "Authentication required"),
        (status = 404, description = "Response absent or owned by another account", body = DeliveryErrorResponse),
        (status = 503, description = "Ledger unavailable", body = DeliveryErrorResponse)
    ))]
pub async fn get_response(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<ResponseRecord>, DeliveryApiError> {
    let Path(id) = id?;
    Ok(Json(state.delivery.get_response(auth.id, id).await?))
}

#[utoipa::path(get, path = "/api/v1/responses/{id}/deliveries", tag = "delivery", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Canonical response identity")), responses(
        (status = 200, description = "Bounded collection of this response's delivery facts; at most 256", body = [DeliveryRecord]),
        (status = 400, description = "Invalid UUID", body = DeliveryErrorResponse),
        (status = 401, description = "Authentication required"),
        (status = 404, description = "Response absent or owned by another account", body = DeliveryErrorResponse),
        (status = 503, description = "Ledger unavailable", body = DeliveryErrorResponse)
    ))]
pub async fn list_deliveries(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<Vec<DeliveryRecord>>, DeliveryApiError> {
    let Path(id) = id?;
    Ok(Json(state.delivery.list_deliveries(auth.id, id).await?))
}

#[utoipa::path(get, path = "/api/v1/deliveries/{id}", tag = "delivery", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Delivery identity")), responses(
        (status = 200, description = "Owner's durable evidence; submitted does not mean presented", body = DeliveryRecord),
        (status = 400, description = "Invalid UUID", body = DeliveryErrorResponse),
        (status = 401, description = "Authentication required"),
        (status = 404, description = "Delivery absent or owned by another account", body = DeliveryErrorResponse),
        (status = 503, description = "Ledger unavailable", body = DeliveryErrorResponse)
    ))]
pub async fn get_delivery(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<DeliveryRecord>, DeliveryApiError> {
    let Path(id) = id?;
    Ok(Json(state.delivery.get_delivery(auth.id, id).await?))
}

#[utoipa::path(post, path = "/api/v1/responses/{id}/cancel", tag = "delivery", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Canonical response identity")), responses(
        (status = 200, description = "Cancellation requested; inspect deliveries for exact device acknowledgement", body = ResponseRecord),
        (status = 400, description = "Invalid UUID", body = DeliveryErrorResponse),
        (status = 401, description = "Authentication required"),
        (status = 404, description = "Response absent or owned by another account", body = DeliveryErrorResponse),
        (status = 503, description = "Cancellation acknowledgement unavailable", body = DeliveryErrorResponse)
    ))]
pub async fn cancel_response(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<ResponseRecord>, DeliveryApiError> {
    let Path(id) = id?;
    Ok(Json(state.delivery.cancel_response(auth.id, id).await?))
}

#[utoipa::path(get, path = "/api/v1/responses/{id}/events", tag = "delivery", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Canonical response identity"), ResponseEventsQuery), responses(
        (status = 200, description = "Owner's ordered audit events, including late evidence after Unknown; paginate with the last event ID", body = [DeliveryEvent]),
        (status = 400, description = "Invalid UUID, cursor or page size", body = DeliveryErrorResponse),
        (status = 401, description = "Authentication required"),
        (status = 404, description = "Response absent or owned by another account", body = DeliveryErrorResponse),
        (status = 503, description = "Ledger unavailable", body = DeliveryErrorResponse)
    ))]
pub async fn response_events(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<ResponseEventsQuery>, QueryRejection>,
) -> Result<Json<Vec<DeliveryEvent>>, DeliveryApiError> {
    let Path(id) = id?;
    let Query(query) = query?;
    Ok(Json(
        state
            .delivery
            .response_events(
                auth.id,
                id,
                query.after.unwrap_or(0),
                query.limit.unwrap_or(64),
            )
            .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/deliveries/{id}/text-chunks", tag = "delivery", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Text delivery identity"), TextChunksQuery), responses(
        (status = 200, description = "Owner's immutable offered UTF-8 chunks; a durable chunk is not proof it was written or read", body = [TextChunkRecord]),
        (status = 400, description = "Invalid UUID, cursor or page size", body = DeliveryErrorResponse),
        (status = 401, description = "Authentication required"),
        (status = 404, description = "Delivery absent or owned by another account", body = DeliveryErrorResponse),
        (status = 409, description = "Delivery medium is not text", body = DeliveryErrorResponse),
        (status = 503, description = "Ledger unavailable", body = DeliveryErrorResponse)
    ))]
pub async fn text_chunks(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<TextChunksQuery>, QueryRejection>,
) -> Result<Json<Vec<TextChunkRecord>>, DeliveryApiError> {
    let Path(id) = id?;
    let Query(query) = query?;
    Ok(Json(
        state
            .delivery
            .text_chunks(auth.id, id, query.after, query.limit.unwrap_or(64))
            .await?
            .into_iter()
            .map(|chunk| TextChunkRecord {
                sequence: chunk.sequence,
                utf8_offset: chunk.utf8_offset,
                text: chunk.text,
            })
            .collect(),
    ))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/responses/{id}", get(get_response))
        .route("/responses/{id}/deliveries", get(list_deliveries))
        .route("/responses/{id}/events", get(response_events))
        .route("/responses/{id}/cancel", post(cancel_response))
        .route("/deliveries/{id}", get(get_delivery))
        .route("/deliveries/{id}/text-chunks", get(text_chunks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn unconfirmed_database_errors_never_expose_private_diagnostics_or_advise_replay() {
        let response = DeliveryApiError::from(DeliveryError::Database(sea_orm::DbErr::Custom(
            "private connection credential".into(),
        )))
        .into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
        assert_eq!(body["error"]["code"], "delivery_unavailable");
        assert!(!body.to_string().contains("private connection credential"));
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("do not replay")
        );
    }
}
