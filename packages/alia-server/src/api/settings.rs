use std::collections::BTreeMap;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

#[derive(Deserialize, Default)]
struct ListQuery {
    /// Optional namespace prefix filter (e.g. "core.ai", "plugin.zigbee")
    #[serde(default)]
    prefix: Option<String>,
}

async fn get_settings(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<ListQuery>,
) -> Result<Json<serde_json::Value>> {
    let entries = match query.prefix.as_deref() {
        Some(prefix) => state.settings.list(prefix).await,
        None => state.settings.all().await,
    };

    Ok(Json(serde_json::to_value(entries).unwrap_or_default()))
}

async fn update_settings(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<BTreeMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>> {
    let changed = state.settings.batch_update(&body).await;

    if !changed.is_empty() {
        if let Err(e) = state.settings.save().await {
            tracing::warn!(error = %e, "failed to persist settings");
        }
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "changed": changed,
    })))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings", get(get_settings).patch(update_settings))
}
