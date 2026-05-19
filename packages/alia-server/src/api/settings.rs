use std::collections::BTreeMap;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::config::MemoryCaptureMode;
use crate::error::{AppError, Result};
use crate::state::AppState;

#[derive(Deserialize, Default)]
struct ListQuery {
    /// Optional namespace prefix filter (e.g. "core.ai", "plugin.zigbee")
    #[serde(default)]
    prefix: Option<String>,
}

#[derive(Debug, Serialize)]
struct SettingsUpdateResponse {
    success: bool,
    changed: Vec<String>,
    applied: Vec<String>,
    requires_restart: Vec<String>,
    ignored_runtime: Vec<String>,
    module_reports: Vec<crate::module::registry::ModuleSettingsApplyReport>,
    errors: Vec<String>,
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
    if body
        .get("core.policies.memory.capture_mode")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| matches!(value, "provider_infer_all" | "provider-infer-all"))
    {
        let capabilities = state.modules.memory.memory_capabilities().await;
        if !capabilities.supports_conversation_ingest || !capabilities.supports_provider_infer {
            return Err(AppError::Validation(format!(
                "capture_mode provider_infer_all is not supported by memory backend '{}'",
                capabilities.provider
            )));
        }
    }
    if body
        .get("core.policies.memory.capture_mode")
        .is_some_and(|value| value == &serde_json::json!(MemoryCaptureMode::ProviderInferAll))
    {
        let capabilities = state.modules.memory.memory_capabilities().await;
        if !capabilities.supports_conversation_ingest || !capabilities.supports_provider_infer {
            return Err(AppError::Validation(format!(
                "capture_mode provider_infer_all is not supported by memory backend '{}'",
                capabilities.provider
            )));
        }
    }

    let changed = state.settings.batch_update(&body).await;

    let mut requires_restart = Vec::new();
    let mut hot_updates = BTreeMap::new();
    for key in &changed {
        let value = body.get(key).cloned().unwrap_or(serde_json::Value::Null);
        let requires = state
            .settings
            .get_entry(key)
            .await
            .and_then(|entry| entry.schema)
            .is_some_and(|schema| schema.requires_restart);
        if requires {
            requires_restart.push(key.clone());
        } else {
            hot_updates.insert(key.clone(), value);
        }
    }

    let mut module_reports = Vec::new();
    let mut applied = Vec::new();
    let mut errors = Vec::new();
    if !hot_updates.is_empty() {
        module_reports = state.modules.apply_settings(&hot_updates).await;
        for report in &module_reports {
            applied.extend(report.applied.iter().cloned());
            if let Some(error) = &report.error {
                errors.push(format!("{}: {}", report.module, error));
            }
        }

        let pipeline_applied = state
            .pipeline_policies
            .write()
            .await
            .apply_settings(&hot_updates);
        applied.extend(pipeline_applied);
        applied.sort();
        applied.dedup();
    }

    let ignored_runtime = hot_updates
        .keys()
        .filter(|key| !applied.contains(key))
        .cloned()
        .collect::<Vec<_>>();

    if !changed.is_empty() {
        if let Err(e) = state.settings.save().await {
            tracing::warn!(error = %e, "failed to persist settings");
        }
    }

    Ok(Json(
        serde_json::to_value(SettingsUpdateResponse {
            success: errors.is_empty(),
            changed,
            applied,
            requires_restart,
            ignored_runtime,
            module_reports,
            errors,
        })
        .unwrap_or_default(),
    ))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings", get(get_settings).patch(update_settings))
}
