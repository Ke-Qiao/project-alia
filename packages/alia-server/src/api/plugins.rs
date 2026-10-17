use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::error::{AppError, Result};
use crate::plugins::storage::{
    purge_plugin_storage as purge_records, validate_collection_name, validate_plugin_name,
};
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
#[serde(deny_unknown_fields)]
struct StoragePurgeQuery {
    #[serde(default)]
    collection: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StorageReadQuery {}

async fn list_plugins(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<ServerPluginInfo>>> {
    require_admin(&auth)?;
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
    require_current_actor(&state.db, &auth, true, false).await?;
    Ok(Json(plugins))
}

async fn get_plugin_storage(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<PluginStorageUsage>> {
    require_admin(&auth)?;
    validate_plugin_name(&name).map_err(|error| AppError::Validation(error.to_string()))?;
    let usage = state
        .plugin_storage
        .usage_for_plugin(&name)
        .await
        .map_err(AppError::Internal)?;
    require_current_actor(&state.db, &auth, true, false).await?;
    Ok(Json(usage))
}

async fn export_plugin_storage(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<PluginStorageExport>> {
    require_admin(&auth)?;
    validate_plugin_name(&name).map_err(|error| AppError::Validation(error.to_string()))?;
    let records = state
        .plugin_storage
        .export_plugin_records(&name)
        .await
        .map_err(AppError::Internal)?;
    require_current_actor(&state.db, &auth, true, false).await?;
    Ok(Json(PluginStorageExport {
        plugin_name: name,
        exported_at: Utc::now().to_rfc3339(),
        records,
    }))
}

async fn purge_plugin_storage(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(name): Path<String>,
    Query(query): Query<StoragePurgeQuery>,
) -> Result<Json<PluginStoragePurgeResponse>> {
    require_admin(&auth)?;
    purge_storage(&state, &auth, name, query, true).await
}

async fn get_user_storage(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(name): Path<String>,
    Query(_query): Query<StorageReadQuery>,
) -> Result<Json<PluginStorageUsage>> {
    validate_plugin_name(&name).map_err(|error| AppError::Validation(error.to_string()))?;
    let usage = state
        .plugin_storage
        .usage_for_user(&name, auth.id)
        .await
        .map_err(AppError::Internal)?;
    require_current_actor(&state.db, &auth, false, false).await?;
    Ok(Json(usage))
}

async fn export_user_storage(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(name): Path<String>,
    Query(_query): Query<StorageReadQuery>,
) -> Result<Json<PluginStorageExport>> {
    validate_plugin_name(&name).map_err(|error| AppError::Validation(error.to_string()))?;
    let records = state
        .plugin_storage
        .export_user_records(&name, auth.id)
        .await
        .map_err(AppError::Internal)?;
    require_current_actor(&state.db, &auth, false, false).await?;
    Ok(Json(PluginStorageExport {
        plugin_name: name,
        exported_at: Utc::now().to_rfc3339(),
        records,
    }))
}

async fn purge_user_storage(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(name): Path<String>,
    Query(query): Query<StoragePurgeQuery>,
) -> Result<Json<PluginStoragePurgeResponse>> {
    purge_storage(&state, &auth, name, query, false).await
}

async fn purge_storage(
    state: &AppState,
    auth: &AuthUser,
    name: String,
    query: StoragePurgeQuery,
    admin: bool,
) -> Result<Json<PluginStoragePurgeResponse>> {
    validate_plugin_name(&name).map_err(|error| AppError::Validation(error.to_string()))?;
    if let Some(collection) = &query.collection {
        validate_collection_name(collection)
            .map_err(|error| AppError::Validation(error.to_string()))?;
    }
    let tx = state.db.begin().await?;
    let result = async {
        let deleted = purge_records(
            &tx,
            &name,
            query.collection.as_deref(),
            (!admin).then_some(auth.id),
        )
        .await
        .map_err(AppError::Internal)?;
        // Recheck after DELETE's waits; keep the account locked through commit.
        require_current_actor(&tx, auth, admin, true).await?;
        Ok::<_, AppError>(deleted)
    }
    .await;
    let deleted_records = match result {
        Ok(deleted) => {
            tx.commit().await?;
            deleted
        }
        Err(error) => {
            tx.rollback().await?;
            return Err(error);
        }
    };
    Ok(Json(PluginStoragePurgeResponse {
        plugin_name: name,
        collection: query.collection,
        deleted_records,
    }))
}

fn require_admin(auth: &AuthUser) -> Result<()> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

async fn require_current_actor(
    db: &impl ConnectionTrait,
    auth: &AuthUser,
    admin: bool,
    lock_actor: bool,
) -> Result<()> {
    let lock = if lock_actor { " FOR SHARE" } else { "" };
    let actor = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        db.query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "SELECT id FROM users WHERE id=$1 AND status='active' \
                 AND (NOT $2::boolean OR role='admin'){lock}"
            ),
            vec![auth.id.into(), admin.into()],
        )),
    )
    .await
    .map_err(|_| AppError::Unavailable("plugin storage authorization timed out; retry explicitly"))?
    .map_err(|error| {
        tracing::error!(%error, "plugin storage authorization failed");
        AppError::Unavailable("plugin storage authorization unavailable; retry explicitly")
    })?;
    if actor.is_none() {
        return Err(AppError::Forbidden);
    }
    Ok(())
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
        .route(
            "/plugins/{name}/storage/me",
            get(get_user_storage).delete(purge_user_storage),
        )
        .route(
            "/plugins/{name}/storage/me/export",
            get(export_user_storage),
        )
}
