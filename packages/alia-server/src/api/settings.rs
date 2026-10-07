use std::collections::BTreeMap;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::error::{AppError, Result};
use crate::modules::memory::context::{ContextError, PolicySnapshot};
use crate::settings::context_policy;
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
    context_policy_version: i64,
    changed: Vec<String>,
    applied: Vec<String>,
    requires_restart: Vec<String>,
    ignored_runtime: Vec<String>,
    module_reports: Vec<crate::bootstrap::registry::ModuleSettingsApplyReport>,
    errors: Vec<String>,
}

async fn get_settings(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ListQuery>,
) -> Result<Response> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    let _update = state.settings.lock_updates().await;
    let policy = current_policy(&state).await?;
    state
        .settings
        .batch_update(&context_policy::projection(&policy.policy))
        .await;
    let entries = match query.prefix.as_deref() {
        Some(prefix) => state.settings.list(prefix).await,
        None => state.settings.all().await,
    };

    let mut response = Json(entries).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{}\"", policy.version)).expect("numeric policy ETag"),
    );
    Ok(response)
}

async fn update_settings(
    State(state): State<AppState>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(body): Json<BTreeMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    let _update = state.settings.lock_updates().await;
    let current = current_policy(&state).await?;
    let expected = if let Some(value) = headers.get(header::IF_MATCH) {
        value
            .to_str()
            .ok()
            .and_then(|s| s.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|v| *v > 0)
            .ok_or_else(|| {
                AppError::Validation(
                    "If-Match must contain the quoted context policy version".into(),
                )
            })?
    } else {
        current.version
    };
    if expected != current.version {
        return Err(AppError::Conflict(
            "context policy changed; reload settings before retrying".into(),
        ));
    }
    let mut next = context_policy::patched(&current.policy, &body)
        .map_err(|error| AppError::Validation(error.to_string()))?;
    let current_profile = state
        .modules
        .embedding
        .preview_profile_settings(&BTreeMap::new())
        .await
        .map_err(|error| AppError::Validation(error.to_string()))?;
    let next_profile = state
        .modules
        .embedding
        .preview_profile_settings(&body)
        .await
        .map_err(|error| AppError::Validation(error.to_string()))?;
    let role = &state.config.llm.roles.embedding;
    let profile_reaffirmed = body.contains_key(&format!("core.profiles.llm.{role}.model"))
        || body.contains_key(&format!("core.profiles.llm.{role}.dimensions"))
        || next_profile.as_ref().is_some_and(|profile| {
            body.contains_key(&format!("core.providers.llm.{}.base_url", profile.provider))
        });
    // A restart may load a new deployment profile while the database still
    // protects the old identity. Explicitly reapplying its model/dimensions or
    // endpoint is a reviewed transition; credential rotation is not one.
    let embedding_changed = next_profile != current_profile || profile_reaffirmed;
    if embedding_changed {
        next.cache_embedding_profile_id = next_profile
            .as_ref()
            .map(|p| p.id())
            .transpose()
            .map_err(|error| AppError::Validation(error.to_string()))?;
        next.active_embedding_profile = next.cache_embedding_profile_id.clone();
    }
    let policy = if body.keys().any(|key| context_policy::owns(key)) || embedding_changed {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            state
                .modules
                .context
                .policy_store()
                .replace_for_actor_with_embedding(
                    expected,
                    next,
                    auth.id,
                    if embedding_changed {
                        next_profile.as_ref()
                    } else {
                        None
                    },
                ),
        )
        .await
        .map_err(|_| {
            AppError::Unavailable(
                "context policy acknowledgement timed out; reload before retrying",
            )
        })?
        .map_err(policy_error)?
    } else {
        current
    };
    // No process-local settings change precedes the durable policy CAS. A
    // second server's unrelated fields cannot be overwritten from this RAM.
    let requested_keys = body.keys().cloned().collect::<Vec<_>>();
    let mut effective_updates = body;
    if embedding_changed && let Some(profile) = next_profile {
        // Persist the provider's complete preview, including an implicit
        // dimension change caused by choosing a different model. Otherwise a
        // restart could combine the new model with an old stored dimension.
        effective_updates.insert(
            format!("core.profiles.llm.{role}.model"),
            serde_json::json!(profile.model),
        );
        effective_updates.insert(
            format!("core.profiles.llm.{role}.dimensions"),
            serde_json::json!(profile.dimensions),
        );
        effective_updates.insert(
            format!("core.providers.llm.{}.base_url", profile.provider),
            serde_json::json!(profile.endpoint),
        );
    }
    effective_updates.extend(context_policy::projection(&policy.policy));
    let changed = state.settings.batch_update(&effective_updates).await;

    let mut requires_restart = Vec::new();
    let mut hot_updates = BTreeMap::new();
    // The registry represents desired settings. A previous apply or file write
    // may have failed after publication, so explicit retries must reach the
    // modules even when the desired value itself is unchanged.
    let mut apply_keys = changed.clone();
    apply_keys.extend(requested_keys.iter().cloned());
    apply_keys.sort();
    apply_keys.dedup();
    for key in &apply_keys {
        let value = effective_updates
            .get(key)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let requires = !context_policy::owns(key)
            && state
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
    // These fields already govern every new Context/Appraisal from committed
    // SQL policy. A legacy registry restart label cannot postpone that gate.
    let mut applied = hot_updates
        .keys()
        .filter(|key| context_policy::owns(key))
        .cloned()
        .collect::<Vec<_>>();
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
            .interaction_policies
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

    if !changed.is_empty() || !requested_keys.is_empty() {
        if let Err(e) = state.settings.save().await {
            tracing::warn!(error = %e, "failed to persist settings");
            errors.push(
                "non-policy settings could not be saved; the context policy is already committed"
                    .into(),
            );
        }
    }

    Ok(Json(
        serde_json::to_value(SettingsUpdateResponse {
            success: errors.is_empty(),
            context_policy_version: policy.version,
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

async fn current_policy(state: &AppState) -> Result<PolicySnapshot> {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        state.modules.context.policy_store().current(),
    )
    .await
    .map_err(|_| AppError::Unavailable("context policy store timed out"))?
    .map_err(policy_error)
}

fn policy_error(error: anyhow::Error) -> AppError {
    match error.downcast_ref::<ContextError>() {
        Some(ContextError::Forbidden) => AppError::Forbidden,
        Some(ContextError::PolicyChanged) => {
            AppError::Conflict("context policy changed; reload settings before retrying".into())
        }
        _ => {
            tracing::error!(%error, "context policy operation failed");
            AppError::Unavailable("context policy store unavailable; reload before retrying")
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings", get(get_settings).patch(update_settings))
}
