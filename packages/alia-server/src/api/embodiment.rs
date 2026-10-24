//! Owner-scoped physical evidence. Mutations use the existing Execution API.

use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
    routing::get,
};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use crate::modules::embodiment::operations::OperationRecord;
use crate::{auth::AuthUser, error::AppError, state::AppState};

#[derive(Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    #[param(default = 25, minimum = 1, maximum = 100)]
    pub limit: u64,
}

fn default_limit() -> u64 {
    25
}

fn unavailable(error: anyhow::Error) -> AppError {
    tracing::error!(%error, "embodiment evidence query failed");
    AppError::Unavailable(
        "Physical operation evidence is unavailable. Query the original operation; do not resubmit it.",
    )
}

#[utoipa::path(get, path = "/api/v1/embodiment/operations", tag = "embodiment", security(("bearer" = [])),
    params(ListQuery), responses(
        (status = 200, description = "Owner's physical operations, newest first", body = [serde_json::Value]),
        (status = 400, description = "Invalid query"), (status = 401, description = "Authentication required"),
        (status = 503, description = "Operation evidence unavailable")
    ))]
pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Vec<OperationRecord>>, AppError> {
    let Query(query) = query.map_err(|e| AppError::Validation(e.body_text()))?;
    if !(1..=100).contains(&query.limit) {
        return Err(AppError::Validation(
            "limit must be between 1 and 100".into(),
        ));
    }
    state
        .embodiment
        .list(auth.id, query.limit)
        .await
        .map(Json)
        .map_err(unavailable)
}

#[utoipa::path(get, path = "/api/v1/embodiment/operations/{id}", tag = "embodiment", security(("bearer" = [])),
    params(("id" = Uuid, Path)), responses(
        (status = 200, description = "Durable operation status and separate physical evidence", body = serde_json::Value),
        (status = 400, description = "Invalid identity"), (status = 401, description = "Authentication required"),
        (status = 404, description = "Operation absent or owned by another account"),
        (status = 503, description = "Operation evidence unavailable")
    ))]
pub async fn get_operation(
    State(state): State<AppState>,
    auth: AuthUser,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<OperationRecord>, AppError> {
    let Path(id) = id.map_err(|e| AppError::Validation(e.body_text()))?;
    state
        .embodiment
        .get(auth.id, id)
        .await
        .map_err(unavailable)?
        .map(Json)
        .ok_or_else(|| AppError::NotFound("Physical operation not found".into()))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/embodiment/operations", get(list))
        .route("/embodiment/operations/{id}", get(get_operation))
}
