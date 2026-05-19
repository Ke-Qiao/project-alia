use axum::extract::{Path, Query, State};
use std::collections::BTreeMap;

use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::config::MemoryCaptureMode;
use crate::error::{AppError, Result};
use crate::memory_policy::{
    detect_sensitive_memory, normalize_memory_layer, normalize_memory_layers,
};
use crate::providers::adapter::{
    MemoryCapabilities, MemoryListOpts, MemoryNode, MemoryPatch, RetrievalOpts,
};
use crate::state::AppState;

#[derive(Serialize)]
pub struct MemoryInfo {
    pub id: String,
    pub memory_layer: String,
    pub node_type: String,
    pub content: String,
    pub importance: f32,
    pub metadata: serde_json::Value,
}

#[derive(Serialize)]
pub struct GraphData {
    pub nodes: Vec<serde_json::Value>,
    pub edges: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct MemoryListQuery {
    layer: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CreateMemoryRequest {
    content: String,
    #[serde(default = "default_memory_layer")]
    memory_layer: String,
    #[serde(default = "default_node_type")]
    node_type: String,
    #[serde(default)]
    metadata: serde_json::Value,
    #[serde(default = "default_importance")]
    importance: f32,
}

#[derive(Debug, Deserialize)]
struct UpdateMemoryRequest {
    content: Option<String>,
    metadata: Option<serde_json::Value>,
    importance: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct SearchMemoriesRequest {
    query: String,
    max_results: Option<u32>,
    min_similarity: Option<f32>,
    layers: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct MemoryPrivacyUpdateRequest {
    enabled: Option<bool>,
    automatic_capture_enabled: Option<bool>,
    capture_mode: Option<MemoryCaptureMode>,
}

#[derive(Debug, Serialize)]
struct MemoryPrivacyResponse {
    enabled: bool,
    capture_mode: MemoryCaptureMode,
    automatic_capture_enabled: bool,
    explicit_memory_enabled: bool,
    sensitive_auto_capture_allowed: bool,
    sensitive_direct_entry_allowed: bool,
}

fn default_memory_layer() -> String {
    "episodic".to_string()
}

fn default_node_type() -> String {
    "fact".to_string()
}

fn default_importance() -> f32 {
    0.5
}

async fn list_memories(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<MemoryListQuery>,
) -> Result<Json<Vec<MemoryInfo>>> {
    let memories = state
        .modules
        .memory
        .list(auth.id, list_opts(&query))
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(memories.into_iter().map(memory_info).collect()))
}

async fn create_memory(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<CreateMemoryRequest>,
) -> Result<Json<MemoryInfo>> {
    let content = body.content.trim();
    if content.is_empty() {
        return Err(AppError::Validation(
            "memory content cannot be empty".into(),
        ));
    }

    let sensitive = detect_sensitive_memory(content);
    let metadata = merge_direct_memory_metadata(body.metadata, sensitive.as_ref());
    let memory = MemoryNode {
        id: None,
        memory_layer: normalize_layer(&body.memory_layer),
        node_type: body.node_type.trim().to_string(),
        content: content.to_string(),
        metadata,
        importance: body.importance.clamp(0.0, 1.0),
    };
    let fallback = memory.clone();
    let id = state
        .modules
        .memory
        .store(auth.id, memory)
        .await
        .map_err(AppError::Internal)?;
    let memory = state
        .modules
        .memory
        .get(auth.id, &id)
        .await
        .map_err(AppError::Internal)?
        .unwrap_or(MemoryNode {
            id: Some(id),
            ..fallback
        });

    Ok(Json(memory_info(memory)))
}

async fn get_memory(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<MemoryInfo>> {
    let memory = state
        .modules
        .memory
        .get(auth.id, &id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound(format!("memory '{id}' not found")))?;

    Ok(Json(memory_info(memory)))
}

async fn update_memory(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateMemoryRequest>,
) -> Result<Json<MemoryInfo>> {
    if state
        .modules
        .memory
        .get(auth.id, &id)
        .await
        .map_err(AppError::Internal)?
        .is_none()
    {
        return Err(AppError::NotFound(format!("memory '{id}' not found")));
    }

    if body
        .content
        .as_ref()
        .is_some_and(|content| content.trim().is_empty())
    {
        return Err(AppError::Validation(
            "memory content cannot be empty".into(),
        ));
    }

    let sensitive = body.content.as_deref().and_then(detect_sensitive_memory);
    state
        .modules
        .memory
        .update(
            &id,
            MemoryPatch {
                content: body.content.map(|content| content.trim().to_string()),
                metadata: body
                    .metadata
                    .map(|metadata| merge_direct_memory_metadata(metadata, sensitive.as_ref())),
                importance: body.importance.map(|importance| importance.clamp(0.0, 1.0)),
            },
        )
        .await
        .map_err(AppError::Internal)?;

    let memory = state
        .modules
        .memory
        .get(auth.id, &id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound(format!("memory '{id}' not found")))?;

    Ok(Json(memory_info(memory)))
}

async fn delete_memory(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    if state
        .modules
        .memory
        .get(auth.id, &id)
        .await
        .map_err(AppError::Internal)?
        .is_none()
    {
        return Err(AppError::NotFound(format!("memory '{id}' not found")));
    }

    state
        .modules
        .memory
        .delete(&id)
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(serde_json::json!({"success": true, "id": id})))
}

async fn get_graph(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<MemoryListQuery>,
) -> Result<Json<GraphData>> {
    let graph = state
        .modules
        .memory
        .graph_for_user(auth.id, list_opts(&query))
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(GraphData {
        nodes: graph
            .nodes
            .into_iter()
            .map(|node| serde_json::json!(memory_info(node)))
            .collect(),
        edges: graph
            .edges
            .into_iter()
            .map(|edge| {
                serde_json::json!({
                    "source_id": edge.source_id,
                    "target_id": edge.target_id,
                    "relation_type": edge.relation_type,
                    "weight": edge.weight,
                })
            })
            .collect(),
    }))
}

async fn search_memories(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<SearchMemoriesRequest>,
) -> Result<Json<Vec<MemoryInfo>>> {
    let query = body.query.trim();
    if query.is_empty() {
        return Err(AppError::Validation(
            "memory search query cannot be empty".into(),
        ));
    }

    let memories = state
        .modules
        .memory
        .retrieve(
            auth.id,
            query,
            RetrievalOpts {
                max_results: Some(body.max_results.unwrap_or(10).clamp(1, 50)),
                min_similarity: body.min_similarity,
                layers: body.layers.map(normalize_layers),
            },
        )
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(memories.into_iter().map(memory_info).collect()))
}

async fn clear_memories(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>> {
    let deleted = state
        .modules
        .memory
        .clear(auth.id)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(
        serde_json::json!({"success": true, "deleted": deleted}),
    ))
}

async fn get_memory_privacy(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<MemoryPrivacyResponse>> {
    let memory = state.pipeline_policies.read().await.memory.clone();
    Ok(Json(memory_privacy_response(&memory)))
}

async fn get_memory_capabilities(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<MemoryCapabilities>> {
    Ok(Json(state.modules.memory.memory_capabilities().await))
}

async fn update_memory_privacy(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<MemoryPrivacyUpdateRequest>,
) -> Result<Json<MemoryPrivacyResponse>> {
    let mut updates = BTreeMap::new();
    if let Some(enabled) = body.enabled {
        updates.insert(
            "core.policies.memory.enabled".to_string(),
            serde_json::json!(enabled),
        );
    }
    let capture_mode = body.capture_mode.or_else(|| {
        body.automatic_capture_enabled.map(|enabled| {
            if enabled {
                MemoryCaptureMode::LlmSelected
            } else {
                MemoryCaptureMode::ExplicitToolOnly
            }
        })
    });
    if let Some(capture_mode) = capture_mode {
        updates.insert(
            "core.policies.memory.capture_mode".to_string(),
            serde_json::json!(capture_mode),
        );
    }

    if !updates.is_empty() {
        state.settings.batch_update(&updates).await;
        state
            .pipeline_policies
            .write()
            .await
            .apply_settings(&updates);
        if let Err(error) = state.settings.save().await {
            tracing::warn!(error = %error, "failed to persist memory privacy settings");
        }
    }

    let memory = state.pipeline_policies.read().await.memory.clone();
    Ok(Json(memory_privacy_response(&memory)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/memories",
            get(list_memories)
                .post(create_memory)
                .delete(clear_memories),
        )
        .route(
            "/memories/privacy",
            get(get_memory_privacy).patch(update_memory_privacy),
        )
        .route("/memories/capabilities", get(get_memory_capabilities))
        .route(
            "/memories/{id}",
            get(get_memory).patch(update_memory).delete(delete_memory),
        )
        .route("/memories/graph", get(get_graph))
        .route("/memories/search", post(search_memories))
}

fn list_opts(query: &MemoryListQuery) -> MemoryListOpts {
    MemoryListOpts {
        max_results: Some(query.limit.unwrap_or(100).clamp(1, 500)),
        layers: query
            .layer
            .as_deref()
            .map(|layer| normalize_layers(vec![layer.to_string()])),
    }
}

fn memory_info(memory: MemoryNode) -> MemoryInfo {
    MemoryInfo {
        id: memory.id.unwrap_or_default(),
        memory_layer: memory.memory_layer,
        node_type: memory.node_type,
        content: memory.content,
        importance: memory.importance,
        metadata: memory.metadata,
    }
}

fn normalize_layers(layers: Vec<String>) -> Vec<String> {
    normalize_memory_layers(layers)
}

fn normalize_layer(layer: &str) -> String {
    normalize_memory_layer(layer)
}

fn merge_direct_memory_metadata(
    metadata: serde_json::Value,
    sensitive: Option<&crate::memory_policy::SensitiveMemoryMatch>,
) -> serde_json::Value {
    let mut metadata = if metadata.is_object() {
        metadata
    } else {
        serde_json::json!({})
    };
    if let Some(object) = metadata.as_object_mut() {
        object.insert("source".to_string(), serde_json::json!("user_direct_api"));
        object.insert("user_supplied".to_string(), serde_json::json!(true));
        if let Some(sensitive) = sensitive {
            object.insert("sensitive".to_string(), serde_json::json!(true));
            object.insert(
                "sensitive_category".to_string(),
                serde_json::json!(sensitive.category),
            );
        }
    }
    metadata
}

fn memory_privacy_response(memory: &crate::config::MemorySection) -> MemoryPrivacyResponse {
    MemoryPrivacyResponse {
        enabled: memory.enabled,
        capture_mode: memory.capture_mode,
        automatic_capture_enabled: memory.enabled
            && matches!(
                memory.capture_mode,
                MemoryCaptureMode::LlmSelected | MemoryCaptureMode::ProviderInferAll
            ),
        explicit_memory_enabled: memory.enabled,
        sensitive_auto_capture_allowed: false,
        sensitive_direct_entry_allowed: true,
    }
}
