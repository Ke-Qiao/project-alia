use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::error::{AppError, Result};
use crate::plugins::storage::validate_plugin_name;
use crate::plugins::{PluginRecord, PluginStorageUsage};
use crate::state::AppState;

#[derive(Serialize)]
pub struct ServerPluginInfo {
    pub name: String,
    pub state: String,
    pub artifact_counts: BTreeMap<String, i64>,
    pub storage: PluginStorageUsage,
    pub capabilities: Vec<String>,
}

#[derive(Serialize)]
pub struct PluginStorageExport {
    pub plugin_name: String,
    pub exported_at: String,
    pub records: Vec<PluginRecord>,
}

#[derive(Serialize)]
pub struct PluginStoragePurgeResponse {
    pub plugin_name: String,
    pub collection: Option<String>,
    pub deleted_records: u64,
}

#[derive(Deserialize, Default)]
struct StoragePurgeQuery {
    #[serde(default)]
    collection: Option<String>,
}

async fn list_plugins(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<ServerPluginInfo>>> {
    let mut plugins = list_artifact_owners(&state)
        .await
        .map_err(AppError::Internal)?;
    for usage in state
        .plugin_storage
        .list_plugin_usage()
        .await
        .map_err(AppError::Internal)?
    {
        plugins
            .entry(usage.plugin_name.clone())
            .or_insert_with(|| ServerPluginInfo {
                name: usage.plugin_name.clone(),
                state: "registered".to_string(),
                artifact_counts: BTreeMap::new(),
                storage: usage.clone(),
                capabilities: Vec::new(),
            })
            .storage = usage.clone();
    }

    let mut plugins = plugins.into_values().collect::<Vec<_>>();
    for plugin in &mut plugins {
        plugin.capabilities = plugin_capabilities(plugin);
    }
    plugins.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Json(plugins))
}

async fn get_plugin_storage(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<PluginStorageUsage>> {
    validate_plugin_name(&name).map_err(|error| AppError::Validation(error.to_string()))?;
    Ok(Json(
        state
            .plugin_storage
            .usage_for_plugin(&name)
            .await
            .map_err(AppError::Internal)?,
    ))
}

async fn export_plugin_storage(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<PluginStorageExport>> {
    validate_plugin_name(&name).map_err(|error| AppError::Validation(error.to_string()))?;
    let records = state
        .plugin_storage
        .export_plugin_records(&name)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(PluginStorageExport {
        plugin_name: name,
        exported_at: Utc::now().to_rfc3339(),
        records,
    }))
}

async fn purge_plugin_storage(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(name): Path<String>,
    Query(query): Query<StoragePurgeQuery>,
) -> Result<Json<PluginStoragePurgeResponse>> {
    validate_plugin_name(&name).map_err(|error| AppError::Validation(error.to_string()))?;
    let deleted_records = state
        .plugin_storage
        .purge_plugin(&name, query.collection.as_deref())
        .await
        .map_err(|error| AppError::Validation(error.to_string()))?;
    Ok(Json(PluginStoragePurgeResponse {
        plugin_name: name,
        collection: query.collection,
        deleted_records,
    }))
}

async fn list_artifact_owners(
    state: &AppState,
) -> anyhow::Result<BTreeMap<String, ServerPluginInfo>> {
    let rows = state
        .db
        .query_all(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT owner_name, artifact_kind, COUNT(*)::bigint AS count
            FROM registry_artifacts
            WHERE owner_kind = 'plugin'
            GROUP BY owner_name, artifact_kind
            ORDER BY owner_name, artifact_kind
            "#
            .to_string(),
        ))
        .await?;

    let mut plugins = BTreeMap::new();
    for row in rows {
        let name: String = row.try_get("", "owner_name")?;
        let kind: String = row.try_get("", "artifact_kind")?;
        let count: i64 = row.try_get("", "count")?;
        plugins
            .entry(name.clone())
            .or_insert_with(|| ServerPluginInfo {
                name: name.clone(),
                state: "registered".to_string(),
                artifact_counts: BTreeMap::new(),
                storage: PluginStorageUsage {
                    plugin_name: name.clone(),
                    record_count: 0,
                    value_bytes: 0,
                    expired_record_count: 0,
                },
                capabilities: Vec::new(),
            })
            .artifact_counts
            .insert(kind, count);
    }
    Ok(plugins)
}

fn plugin_capabilities(plugin: &ServerPluginInfo) -> Vec<String> {
    let mut capabilities = plugin
        .artifact_counts
        .keys()
        .map(|kind| format!("artifact.{kind}"))
        .collect::<Vec<_>>();
    if plugin.storage.record_count > 0 {
        capabilities.push("storage.records".to_string());
    }
    capabilities.sort();
    capabilities
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/plugins", get(list_plugins))
        .route(
            "/plugins/{name}/storage",
            get(get_plugin_storage).delete(purge_plugin_storage),
        )
        .route("/plugins/{name}/storage/export", get(export_plugin_storage))
}
