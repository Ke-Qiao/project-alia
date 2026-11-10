use super::deliveries::{DeliveryErrorCode, DeliveryErrorDetail, DeliveryErrorResponse};
use crate::{
    auth::AuthUser,
    modules::embodiment::motion_delivery::{
        MotionDeliveryRecord, MotionDeliveryRepository, MotionLookupError,
    },
    state::AppState,
};
use axum::{
    Json, Router,
    extract::{Path, State, rejection::PathRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use uuid::Uuid;

pub struct MotionApiError(anyhow::Error);
impl From<anyhow::Error> for MotionApiError {
    fn from(value: anyhow::Error) -> Self {
        Self(value)
    }
}
impl From<PathRejection> for MotionApiError {
    fn from(value: PathRejection) -> Self {
        Self(value.into())
    }
}
impl IntoResponse for MotionApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self.0.downcast_ref::<MotionLookupError>() {
            Some(MotionLookupError::NotFound) => (
                StatusCode::NOT_FOUND,
                DeliveryErrorCode::NotFound,
                "Motion delivery or response not found.",
            ),
            Some(MotionLookupError::Unauthorized) => (
                StatusCode::FORBIDDEN,
                DeliveryErrorCode::Unauthorized,
                "Motion owner is unavailable.",
            ),
            None if self.0.is::<PathRejection>() => (
                StatusCode::BAD_REQUEST,
                DeliveryErrorCode::InvalidInput,
                "Invalid motion delivery or response identity.",
            ),
            None => {
                tracing::error!(error=%self.0, "motion delivery query unavailable");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    DeliveryErrorCode::DeliveryUnavailable,
                    "Motion evidence unavailable. Query the original delivery; do not replay the physical command.",
                )
            }
        };
        (
            status,
            Json(DeliveryErrorResponse {
                error: DeliveryErrorDetail {
                    code,
                    message: message.into(),
                },
            }),
        )
            .into_response()
    }
}

#[utoipa::path(operation_id = "list_response_motion_deliveries", get, path="/api/v1/responses/{id}/motion-deliveries", tag="delivery", security(("bearer"=[])), params(("id"=Uuid, Path)), responses(
    (status=200, description="Owner's motion delivery facts; at most 32, software submission is not position confirmation", body=[MotionDeliveryRecord]),
    (status=400, description="Invalid UUID", body=DeliveryErrorResponse), (status=401, description="Authentication required"),
    (status=403, description="Motion owner is unavailable", body=DeliveryErrorResponse),
    (status=404, description="Response absent or owned by another account", body=DeliveryErrorResponse), (status=503, description="Ledger unavailable", body=DeliveryErrorResponse)
))]
pub async fn for_response(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<Vec<MotionDeliveryRecord>>, MotionApiError> {
    let Path(id) = id?;
    Ok(Json(
        MotionDeliveryRepository::new(state.db.clone())
            .for_response(auth.id, id)
            .await?,
    ))
}

#[utoipa::path(operation_id = "get_motion_delivery", get, path="/api/v1/motion-deliveries/{id}", tag="delivery", security(("bearer"=[])), params(("id"=Uuid, Path)), responses(
    (status=200, description="Owner's immutable motion command and durable device evidence", body=MotionDeliveryRecord),
    (status=400, description="Invalid UUID", body=DeliveryErrorResponse), (status=401, description="Authentication required"),
    (status=403, description="Motion owner is unavailable", body=DeliveryErrorResponse),
    (status=404, description="Motion delivery absent or owned by another account", body=DeliveryErrorResponse), (status=503, description="Ledger unavailable", body=DeliveryErrorResponse)
))]
pub async fn get_delivery(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<MotionDeliveryRecord>, MotionApiError> {
    let Path(id) = id?;
    Ok(Json(
        MotionDeliveryRepository::new(state.db.clone())
            .get(auth.id, id)
            .await?,
    ))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/responses/{id}/motion-deliveries", get(for_response))
        .route("/motion-deliveries/{id}", get(get_delivery))
}
