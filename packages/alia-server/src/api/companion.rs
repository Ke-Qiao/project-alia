//! Owner-scoped bounded control. Request bodies cannot supply authority or opportunities.
use std::time::Duration;

use axum::extract::{Path, State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, Result};
use crate::modules::companion::repository::RepoError;
use crate::modules::companion::types::{CompanionMode, CompanionPolicy};
use crate::state::AppState;

const REQUEST_BUDGET: Duration = Duration::from_secs(2);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyRequest {
    expected_revision: i64,
    policy: CompanionPolicy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShadowRequest {
    operation_id: Uuid,
    appraisal_ids: Vec<Uuid>,
}

async fn policy(State(state): State<AppState>, auth: AuthUser) -> Result<Response> {
    let policy = tokio::time::timeout(REQUEST_BUDGET, state.modules.companion.policy(auth.id))
        .await
        .map_err(|_| AppError::Unavailable("companion policy read timed out"))?
        .map_err(repo_error)?;
    Ok(Json(policy).into_response())
}

async fn update_policy(
    State(state): State<AppState>,
    auth: AuthUser,
    body: std::result::Result<Json<PolicyRequest>, JsonRejection>,
) -> Result<Response> {
    let Json(body) =
        body.map_err(|_| AppError::Validation("expected_revision and policy are required".into()))?;
    if body.expected_revision <= 0 {
        return Err(AppError::Validation(
            "expected_revision must be positive".into(),
        ));
    }
    if body.policy.mode == CompanionMode::Enabled && !state.modules.companion.live_ready() {
        return Err(AppError::Validation(
            "live companion admission is not available yet; use shadow or disabled".into(),
        ));
    }
    body.policy
        .validate()
        .map_err(|_| AppError::Validation("invalid companion policy".into()))?;
    let result = tokio::time::timeout(
        REQUEST_BUDGET,
        state
            .modules
            .companion
            .replace_policy(auth.id, body.expected_revision, body.policy),
    )
    .await;
    match result {
        Ok(Ok(policy)) => Ok(Json(policy).into_response()),
        Ok(Err(error)) if !matches!(error, RepoError::Unavailable(_)) => Err(repo_error(error)),
        _ => Ok((StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": {
            "code": "COMPANION_POLICY_UNCONFIRMED",
            "message": "policy completion is unconfirmed; read current policy before retrying the original revision",
            "expected_revision": body.expected_revision
        }}))).into_response()),
    }
}

async fn shadow(
    State(state): State<AppState>,
    auth: AuthUser,
    body: std::result::Result<Json<ShadowRequest>, JsonRejection>,
) -> Result<Response> {
    let Json(body) = body
        .map_err(|_| AppError::Validation("operation_id and appraisal_ids are required".into()))?;
    if body.operation_id.is_nil()
        || !(1..=32).contains(&body.appraisal_ids.len())
        || body.appraisal_ids.iter().any(Uuid::is_nil)
        || body
            .appraisal_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != body.appraisal_ids.len()
    {
        return Err(AppError::Validation(
            "supply a nonzero operation_id and 1..32 distinct appraisal_ids".into(),
        ));
    }
    let operation = body.operation_id;
    let result = tokio::time::timeout(
        REQUEST_BUDGET,
        state
            .modules
            .companion
            .evaluate(auth.id, operation, body.appraisal_ids),
    )
    .await;
    match result {
        Ok(Ok(report)) => Ok(Json(report).into_response()),
        Ok(Err(RepoError::Erased)) => Ok(erased(operation)),
        Ok(Err(error)) if !matches!(error, RepoError::Unavailable(_)) => Err(repo_error(error)),
        _ => Ok((StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": {
            "code": "COMPANION_SHADOW_UNCONFIRMED",
            "message": "completion is unconfirmed; retry the same operation_id and appraisal_ids",
            "operation_id": operation
        }}))).into_response()),
    }
}

async fn replay(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(operation): Path<Uuid>,
) -> Result<Response> {
    if operation.is_nil() {
        return Err(AppError::Validation("operation_id must be nonzero".into()));
    }
    match tokio::time::timeout(
        REQUEST_BUDGET,
        state.modules.companion.replay(auth.id, operation),
    )
    .await
    {
        Ok(Ok(report)) => Ok(Json(report).into_response()),
        Ok(Err(RepoError::Erased)) => Ok(erased(operation)),
        Ok(Err(error)) => Err(repo_error(error)),
        Err(_) => Err(AppError::Unavailable("companion replay read timed out")),
    }
}

fn erased(operation: Uuid) -> Response {
    (
        StatusCode::GONE,
        Json(json!({"error": {
            "code": "COMPANION_EVIDENCE_ERASED",
            "message": "private decision evidence was erased; it cannot be replayed",
            "operation_id": operation
        }})),
    )
        .into_response()
}

fn repo_error(error: RepoError) -> AppError {
    match error {
        RepoError::InvalidInput(message) => AppError::Validation(message),
        RepoError::Conflict => {
            AppError::Conflict("companion identity or policy revision changed".into())
        }
        RepoError::Forbidden | RepoError::Erased => AppError::Forbidden,
        RepoError::Unavailable(_) => AppError::Unavailable("companion storage unavailable"),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/companion/policy", get(policy).put(update_policy))
        .route("/companion/shadow", post(shadow))
        .route("/companion/replay/{operation_id}", get(replay))
}
