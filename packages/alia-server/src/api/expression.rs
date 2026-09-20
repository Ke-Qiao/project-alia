use std::time::Duration;

use axum::extract::{State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, Result};
use crate::expression::{RelationshipProfile, RelationshipPruneResult};
use crate::modules::memory::context::ContextError;
use crate::state::AppState;

const REQUEST_BUDGET: Duration = Duration::from_secs(2);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClearRequest {
    operation_id: Uuid,
    expected_epoch: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PruneRequest {
    limit: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RebuildRequest {
    operation_id: Uuid,
    #[serde(default)]
    after: Option<Uuid>,
    limit: u16,
}

async fn get_relationship_profile(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<RelationshipProfile>> {
    let profile = tokio::time::timeout(
        REQUEST_BUDGET,
        state.modules.context.relationship_profile(auth.id),
    )
    .await
    .map_err(|_| AppError::Unavailable("relationship read timed out"))?
    .map_err(relationship_error)?;
    if !profile.privacy.user_visible {
        return Err(AppError::Forbidden);
    }
    Ok(Json(profile))
}

async fn clear_relationship_profile(
    State(state): State<AppState>,
    auth: AuthUser,
    body: std::result::Result<Json<ClearRequest>, JsonRejection>,
) -> Result<Response> {
    let Json(body) = body.map_err(|_| {
        AppError::Validation(
            "operation_id and expected_epoch are required for a relationship clear".into(),
        )
    })?;
    if body.operation_id.is_nil() || body.expected_epoch < 0 {
        return Err(AppError::Validation(
            "invalid relationship clear identity".into(),
        ));
    }
    let result = tokio::time::timeout(
        REQUEST_BUDGET,
        state
            .modules
            .context
            .clear_relationship(auth.id, body.operation_id, body.expected_epoch),
    )
    .await;
    match result {
        Ok(Ok(receipt)) => Ok(Json(receipt).into_response()),
        Ok(Err(error))
            if matches!(
                error.downcast_ref::<ContextError>(),
                Some(
                    ContextError::Forbidden
                        | ContextError::Revoked(_)
                        | ContextError::PolicyChanged
                        | ContextError::RelationshipConflict { .. }
                        | ContextError::RelationshipOperationConflict { .. }
                )
            ) =>
        {
            Err(relationship_error(error))
        }
        // SQL may report its own timeout before the HTTP deadline fires. Every
        // unavailable result retains the original intent, including a lost
        // commit acknowledgement; neither deadline can mint a replacement ID.
        other => {
            if let Ok(Err(error)) = other {
                tracing::error!(error=?error, "relationship clear completion unconfirmed");
            }
            Ok((StatusCode::SERVICE_UNAVAILABLE, Json(json!({
                "error": {
                    "code": "RELATIONSHIP_CLEAR_UNCONFIRMED",
                    "message": "clear completion is unconfirmed; retry the same operation_id and expected_epoch",
                    "operation_id": body.operation_id,
                    "expected_epoch": body.expected_epoch,
                }
            }))).into_response())
        }
    }
}

async fn prune_relationship_profile(
    State(state): State<AppState>,
    auth: AuthUser,
    body: std::result::Result<Json<PruneRequest>, JsonRejection>,
) -> Result<Json<RelationshipPruneResult>> {
    let Json(body) =
        body.map_err(|_| AppError::Validation("a bounded prune limit is required".into()))?;
    if !(1..=256).contains(&body.limit) {
        return Err(AppError::Validation(
            "relationship prune limit must be 1..256".into(),
        ));
    }
    // The authenticated owner can only prune that owner's expired records.
    let result = tokio::time::timeout(
        REQUEST_BUDGET,
        state
            .modules
            .context
            .prune_relationship(auth.id, body.limit),
    )
    .await
    .map_err(|_| AppError::Unavailable("relationship prune completion is unconfirmed"))?
    .map_err(relationship_error)?;
    Ok(Json(result))
}

async fn rebuild_relationship_profile(
    State(state): State<AppState>,
    auth: AuthUser,
    body: std::result::Result<Json<RebuildRequest>, JsonRejection>,
) -> Result<Response> {
    let Json(body) = body.map_err(|_| {
        AppError::Validation("operation_id and a bounded rebuild limit are required".into())
    })?;
    if body.operation_id.is_nil()
        || body.after.is_some_and(|id| id.is_nil())
        || !(1..=32).contains(&body.limit)
    {
        return Err(AppError::Validation(
            "invalid rebuild page identity or limit (1..32)".into(),
        ));
    }
    let result = tokio::time::timeout(
        REQUEST_BUDGET,
        state.modules.expression.rebuild_relationship(
            &state.modules.context,
            auth.id,
            body.operation_id,
            body.after,
            body.limit,
        ),
    )
    .await;
    match result {
        Ok(Ok(page)) => Ok((
            if page.pending == 0 {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            },
            Json(page),
        )
            .into_response()),
        Ok(Err(error))
            if matches!(
                error.downcast_ref::<ContextError>(),
                Some(
                    ContextError::Forbidden
                        | ContextError::Revoked(_)
                        | ContextError::PolicyChanged
                        | ContextError::RelationshipConflict { .. }
                        | ContextError::RelationshipOperationConflict { .. }
                )
            ) =>
        {
            Err(relationship_error(error))
        }
        other => {
            if let Ok(Err(error)) = other {
                tracing::error!(error=?error, "relationship rebuild completion unconfirmed");
            }
            Ok((StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": {
                "code":"RELATIONSHIP_REBUILD_UNCONFIRMED",
                "message":"rebuild completion is unconfirmed; retry the exact operation and page",
                "operation_id":body.operation_id,"after":body.after,"limit":body.limit
            }}))).into_response())
        }
    }
}

fn relationship_error(error: anyhow::Error) -> AppError {
    match error.downcast_ref::<ContextError>() {
        Some(ContextError::Forbidden | ContextError::Revoked(_)) => AppError::Forbidden,
        Some(
            ContextError::RelationshipConflict { .. }
            | ContextError::RelationshipOperationConflict { .. }
            | ContextError::PolicyChanged,
        ) => {
            AppError::Conflict("relationship or policy changed; reload the current profile".into())
        }
        _ => {
            tracing::error!(error=?error, "relationship operation failed");
            AppError::Unavailable("relationship operation unavailable")
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/expression/relationship",
            get(get_relationship_profile).delete(clear_relationship_profile),
        )
        .route(
            "/expression/relationship/prune",
            post(prune_relationship_profile),
        )
        .route(
            "/expression/relationship/rebuild",
            post(rebuild_relationship_profile),
        )
}
