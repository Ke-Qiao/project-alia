//! Owner-scoped bounded control. Request bodies cannot supply authority or opportunities.
use std::time::Duration;

use axum::extract::{Path, State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, ErrorResponse, Result};
use crate::modules::companion::repository::{RepoError, ShadowReport};
use crate::modules::companion::types::{CompanionMode, CompanionPolicy, PolicySnapshot};
use crate::state::AppState;

const REQUEST_BUDGET: Duration = Duration::from_secs(2);

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyRequest {
    /// Positive revision returned by GET; retries retain this original revision and policy.
    #[schema(minimum = 1)]
    expected_revision: i64,
    /// Full replacement; all seven policy fields are required.
    policy: CompanionPolicy,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShadowRequest {
    /// Nonzero caller-generated identity. Retain it with the same Appraisal set on retry.
    operation_id: Uuid,
    /// 1–32 distinct nonzero canonical Appraisal IDs belonging to this owner.
    #[schema(min_items = 1, max_items = 32)]
    appraisal_ids: Vec<Uuid>,
}

/// Companion mutation and erasure receipts require their original operation identity.
#[derive(Serialize, ToSchema)]
pub struct CompanionErrorResponse {
    pub error: CompanionErrorDetail,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "code", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CompanionErrorDetail {
    CompanionPolicyUnconfirmed {
        message: String,
        expected_revision: i64,
    },
    CompanionShadowUnconfirmed {
        message: String,
        operation_id: Uuid,
    },
    CompanionEvidenceErased {
        message: String,
        operation_id: Uuid,
    },
}

/// Read the authenticated owner's current policy. The account is rechecked in
/// SQL; admin roles do not select another owner. An absent policy is seeded as shadow.
#[utoipa::path(get, path = "/api/v1/companion/policy", operation_id = "get_companion_policy", tag = "companion", security(("bearer" = [])), responses(
    (status = 200, description = "Current owner policy and positive CAS revision", body = PolicySnapshot),
    (status = 401, description = "AUTH_INVALID_CREDENTIALS: missing/invalid JWT, inactive account", body = ErrorResponse),
    (status = 403, description = "AUTH_FORBIDDEN: current owner authority unavailable", body = ErrorResponse),
    (status = 503, description = "SERVICE_UNAVAILABLE: read/storage unavailable; this is not a mutation receipt", body = ErrorResponse),
    (status = 500, description = "INTERNAL_DB_ERROR: account lookup database failure", body = ErrorResponse)
))]
pub async fn policy(State(state): State<AppState>, auth: AuthUser) -> Result<Response> {
    let policy = tokio::time::timeout(REQUEST_BUDGET, state.modules.companion.policy(auth.id))
        .await
        .map_err(|_| AppError::Unavailable("companion policy read timed out"))?
        .map_err(repo_error)?;
    Ok(Json(policy).into_response())
}

/// Replace all policy fields using compare-and-swap. Unknown fields are rejected.
/// Enabled requires an installed live runner; it does not prove a device is ready.
/// On 503, GET and compare the original intent before retrying that exact revision
/// and policy. Never silently advance expected_revision to overwrite another change.
#[utoipa::path(put, path = "/api/v1/companion/policy", operation_id = "update_companion_policy", tag = "companion", security(("bearer" = [])),
    request_body = PolicyRequest, responses(
    (status = 200, description = "Committed replacement with incremented revision", body = PolicySnapshot),
    (status = 400, description = "VALIDATION_ERROR: malformed/unknown fields, invalid bounds, or live runner unavailable for enabled mode", body = ErrorResponse),
    (status = 401, description = "AUTH_INVALID_CREDENTIALS: missing/invalid JWT, inactive account", body = ErrorResponse),
    (status = 403, description = "AUTH_FORBIDDEN: current owner authority unavailable", body = ErrorResponse),
    (status = 409, description = "VERSION_CONFLICT: original revision is no longer current; read current policy and decide explicitly", body = ErrorResponse),
    (status = 503, description = "COMPANION_POLICY_UNCONFIRMED: completion unknown, with original expected_revision; read current policy before retrying the same intent", body = CompanionErrorResponse),
    (status = 500, description = "INTERNAL_DB_ERROR: account lookup database failure", body = ErrorResponse)
))]
pub async fn update_policy(
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
        _ => Ok(unconfirmed_policy(body.expected_revision)),
    }
}

/// Evaluate existing canonical Appraisals without generating output or consuming
/// live admission budget. Request fields cannot supply owner, opportunity or authority.
/// Repeating the same operation and Appraisal set reads its original frozen receipt;
/// changing that intent conflicts. A new shadow operation while enabled is unavailable.
#[utoipa::path(post, path = "/api/v1/companion/shadow", operation_id = "run_companion_shadow", tag = "companion", security(("bearer" = [])),
    request_body = ShadowRequest, responses(
    (status = 200, description = "Bounded frozen report; up to 32 examined sources and 4 decisions, never output authority", body = ShadowReport),
    (status = 400, description = "VALIDATION_ERROR: malformed/unknown fields, nil IDs, duplicate IDs or batch outside 1–32", body = ErrorResponse),
    (status = 401, description = "AUTH_INVALID_CREDENTIALS: missing/invalid JWT, inactive account", body = ErrorResponse),
    (status = 403, description = "AUTH_FORBIDDEN: canonical sources or current private-read authority unavailable", body = ErrorResponse),
    (status = 409, description = "VERSION_CONFLICT: operation identity/worklist or canonical policy authority changed", body = ErrorResponse),
    (status = 410, description = "COMPANION_EVIDENCE_ERASED with operation_id: original evidence erased, expired or revoked; it cannot be reconstructed by retry", body = CompanionErrorResponse),
    (status = 503, description = "COMPANION_SHADOW_UNCONFIRMED with operation_id: completion unknown; retry exactly the same operation_id and appraisal_ids", body = CompanionErrorResponse),
    (status = 500, description = "INTERNAL_DB_ERROR: account lookup database failure", body = ErrorResponse)
))]
pub async fn shadow(
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
        _ => Ok(unconfirmed_shadow(operation)),
    }
}

/// Read and deterministically replay this owner's original frozen report after
/// rechecking current source/owner authority. This never retries audio or grants live output.
#[utoipa::path(get, path = "/api/v1/companion/replay/{operation_id}", operation_id = "get_companion_replay", tag = "companion", security(("bearer" = [])),
    params(("operation_id" = Uuid, Path, description = "Original nonzero shadow operation identity")), responses(
    (status = 200, description = "Original frozen inputs and pure decision after current authority recheck", body = ShadowReport),
    (status = 400, description = "Nil UUID: JSON VALIDATION_ERROR; malformed UUID: Axum plain-text path rejection", content((ErrorResponse = "application/json"), (String = "text/plain"))),
    (status = 401, description = "AUTH_INVALID_CREDENTIALS: missing/invalid JWT, inactive account", body = ErrorResponse),
    (status = 403, description = "AUTH_FORBIDDEN: missing/other-owner operation or current private-read authority unavailable; no existence disclosure", body = ErrorResponse),
    (status = 409, description = "VERSION_CONFLICT: canonical policy authority changed", body = ErrorResponse),
    (status = 410, description = "COMPANION_EVIDENCE_ERASED with operation_id: private evidence erased, expired or revoked; replay is unavailable", body = CompanionErrorResponse),
    (status = 503, description = "SERVICE_UNAVAILABLE: read/storage unavailable; retry this same read", body = ErrorResponse),
    (status = 500, description = "INTERNAL_DB_ERROR: account lookup database failure", body = ErrorResponse)
))]
pub async fn replay(
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

pub(crate) fn unconfirmed_policy(expected_revision: i64) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(CompanionErrorResponse {
            error: CompanionErrorDetail::CompanionPolicyUnconfirmed {
                message: "policy completion is unconfirmed; read current policy before retrying the original revision".into(),
                expected_revision,
            },
        }),
    ).into_response()
}

pub(crate) fn unconfirmed_shadow(operation_id: Uuid) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(CompanionErrorResponse {
            error: CompanionErrorDetail::CompanionShadowUnconfirmed {
                message: "completion is unconfirmed; retry the same operation_id and appraisal_ids"
                    .into(),
                operation_id,
            },
        }),
    )
        .into_response()
}

pub(crate) fn erased(operation_id: Uuid) -> Response {
    (
        StatusCode::GONE,
        Json(CompanionErrorResponse {
            error: CompanionErrorDetail::CompanionEvidenceErased {
                message: "private decision evidence was erased; it cannot be replayed".into(),
                operation_id,
            },
        }),
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
