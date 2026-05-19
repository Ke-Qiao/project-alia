use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::auth::AuthUser;
use crate::error::{AppError, Result};
use crate::expression::{RelationshipClearResult, RelationshipProfile, RelationshipPruneResult};
use crate::state::AppState;

async fn get_relationship_profile(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<RelationshipProfile>> {
    let profile = state
        .modules
        .expression
        .relationship_profile(&auth.id.to_string());
    if !profile.privacy.user_visible {
        return Err(AppError::Forbidden);
    }
    Ok(Json(profile))
}

async fn clear_relationship_profile(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<RelationshipClearResult>> {
    let profile = state
        .modules
        .expression
        .relationship_profile(&auth.id.to_string());
    if !profile.privacy.clearable {
        return Err(AppError::Forbidden);
    }
    let result = state
        .modules
        .expression
        .clear_relationship_data(&auth.id.to_string())
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(result))
}

async fn prune_relationship_profiles(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<RelationshipPruneResult>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(Json(state.modules.expression.prune_relationship_profiles()))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/expression/relationship",
            get(get_relationship_profile).delete(clear_relationship_profile),
        )
        .route(
            "/expression/relationships/prune",
            post(prune_relationship_profiles),
        )
}
