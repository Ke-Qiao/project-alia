use axum::extract::{Path, Query, State};
use std::collections::BTreeMap;

use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::config::MemoryCaptureMode;
use crate::error::{AppError, Result};
use crate::memory_policy::{
    detect_sensitive_memory, normalize_memory_layer, normalize_memory_layers,
};
use crate::providers::adapter::{
    MemoryCapabilities, MemoryConsolidationResult, MemoryContextPack, MemoryExplanation,
    MemoryExport, MemoryListOpts, MemoryNode, MemoryPatch, MemorySuppressedExplanation,
    RetrievalOpts,
};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct MemoryInfo {
    pub id: String,
    pub memory_layer: String,
    pub node_type: String,
    pub kind: String,
    pub scope: String,
    pub content: String,
    pub importance: f32,
    pub confidence: f32,
    pub salience: f32,
    pub visibility: String,
    pub sensitive: bool,
    pub status: String,
    pub topic_tags: Vec<String>,
    pub affect_tags: Vec<String>,
    pub evidence_summary: Option<String>,
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
    kind: Option<String>,
    #[serde(default)]
    scope: Option<String>,
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
    pause_days: Option<u32>,
    clear_pause: Option<bool>,
}

#[derive(Debug, Serialize)]
struct MemoryPrivacyResponse {
    enabled: bool,
    capture_mode: MemoryCaptureMode,
    automatic_capture_enabled: bool,
    paused_until: Option<String>,
    explicit_memory_enabled: bool,
    sensitive_auto_capture_allowed: bool,
    sensitive_direct_entry_allowed: bool,
    supported_capture_modes: Vec<MemoryCaptureMode>,
}

#[derive(Debug, Serialize)]
struct MemoryAuditResponse {
    privacy: MemoryPrivacyResponse,
    capabilities: MemoryCapabilities,
    memories: Vec<MemoryInfo>,
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
    let mut memory = MemoryNode::new(
        normalize_layer(&body.memory_layer),
        body.node_type.trim().to_string(),
        content.to_string(),
        body.importance.clamp(0.0, 1.0),
    )
    .with_metadata(metadata)
    .with_runtime_fields(
        body.kind.unwrap_or_else(|| "semantic".to_string()),
        body.scope.unwrap_or_else(|| "user".to_string()),
        1.0,
        body.importance.clamp(0.0, 1.0),
        "user_visible",
    );
    memory.sensitive = sensitive.is_some();
    memory.evidence = serde_json::json!({
        "source": "direct_user_memory_api",
        "summary": "The user directly added this memory.",
    });
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
        .unwrap_or_else(|| {
            let mut fallback = fallback;
            fallback.id = Some(id);
            fallback
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
                ..MemoryPatch::default()
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

    let pack = state
        .modules
        .memory
        .retrieve_context_pack(
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

    let memories = memory_context_pack_to_infos(pack);
    Ok(Json(memories.into_iter().map(memory_info).collect()))
}

async fn search_memory_context(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<SearchMemoriesRequest>,
) -> Result<Json<MemoryContextPack>> {
    let query = body.query.trim();
    if query.is_empty() {
        return Err(AppError::Validation(
            "memory search query cannot be empty".into(),
        ));
    }

    let pack = state
        .modules
        .memory
        .retrieve_context_pack(
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
    Ok(Json(pack))
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

async fn delete_memories_by_topic(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(topic): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let topic = topic.trim();
    if topic.is_empty() {
        return Err(AppError::Validation("memory topic cannot be empty".into()));
    }
    let deleted = state
        .modules
        .memory
        .delete_by_topic(auth.id, topic)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(
        serde_json::json!({"success": true, "topic": topic, "deleted": deleted}),
    ))
}

async fn delete_memories_by_kind(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(kind): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let kind = kind.trim();
    if kind.is_empty() {
        return Err(AppError::Validation("memory kind cannot be empty".into()));
    }
    let deleted = state
        .modules
        .memory
        .delete_by_kind(auth.id, kind)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(
        serde_json::json!({"success": true, "kind": kind, "deleted": deleted}),
    ))
}

async fn delete_hidden_memories(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>> {
    let deleted = state
        .modules
        .memory
        .delete_hidden(auth.id)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(
        serde_json::json!({"success": true, "deleted": deleted}),
    ))
}

async fn explain_suppressed_memory(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<MemorySuppressedExplanation>> {
    let explanation = state
        .modules
        .memory
        .explain_suppressed(auth.id, &id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound(format!("memory '{id}' not found")))?;
    Ok(Json(explanation))
}

async fn export_memories(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<MemoryExport>> {
    let export = state
        .modules
        .memory
        .export_user(auth.id)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(export))
}

async fn consolidate_memories(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<MemoryConsolidationResult>> {
    let result = state
        .modules
        .memory
        .consolidate_user(auth.id)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(result))
}

async fn get_memory_privacy(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<MemoryPrivacyResponse>> {
    let memory = state.pipeline_policies.read().await.memory.clone();
    let capabilities = state.modules.memory.memory_capabilities().await;
    Ok(Json(memory_privacy_response(&memory, &capabilities)))
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
    if body.clear_pause.unwrap_or(false) {
        updates.insert(
            "core.policies.memory.paused_until".to_string(),
            serde_json::json!(null),
        );
    } else if let Some(days) = body.pause_days {
        let until = chrono::Utc::now() + chrono::Duration::days(i64::from(days.clamp(1, 365)));
        updates.insert(
            "core.policies.memory.paused_until".to_string(),
            serde_json::json!(until.to_rfc3339()),
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
    let capabilities = state.modules.memory.memory_capabilities().await;
    Ok(Json(memory_privacy_response(&memory, &capabilities)))
}

async fn get_memory_audit(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<MemoryListQuery>,
) -> Result<Json<MemoryAuditResponse>> {
    let capabilities = state.modules.memory.memory_capabilities().await;
    let memory = state.pipeline_policies.read().await.memory.clone();
    let memories = state
        .modules
        .memory
        .list(auth.id, list_opts(&query))
        .await
        .map_err(AppError::Internal)?
        .into_iter()
        .map(memory_info)
        .collect();
    Ok(Json(MemoryAuditResponse {
        privacy: memory_privacy_response(&memory, &capabilities),
        capabilities,
        memories,
    }))
}

async fn explain_memory(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<MemoryExplanation>> {
    let explanation = state
        .modules
        .memory
        .explain(auth.id, &id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound(format!("memory '{id}' not found")))?;
    Ok(Json(explanation))
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
        .route("/memories/audit", get(get_memory_audit))
        .route("/memories/export", get(export_memories))
        .route("/memories/consolidate", post(consolidate_memories))
        .route("/memories/search/context", post(search_memory_context))
        .route("/memories/hidden", delete(delete_hidden_memories))
        .route("/memories/topic/{topic}", delete(delete_memories_by_topic))
        .route("/memories/kind/{kind}", delete(delete_memories_by_kind))
        .route("/memories/graph", get(get_graph))
        .route("/memories/search", post(search_memories))
        .route("/memories/{id}/explain", get(explain_memory))
        .route("/memories/{id}/suppressed", get(explain_suppressed_memory))
        .route(
            "/memories/{id}",
            get(get_memory).patch(update_memory).delete(delete_memory),
        )
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
    let evidence_summary = memory
        .evidence
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    MemoryInfo {
        id: memory.id.unwrap_or_default(),
        memory_layer: memory.memory_layer,
        node_type: memory.node_type,
        kind: memory.kind,
        scope: memory.scope,
        content: memory.content,
        importance: memory.importance,
        confidence: memory.confidence,
        salience: memory.salience,
        visibility: memory.visibility,
        sensitive: memory.sensitive,
        status: memory.status,
        topic_tags: memory.topic_tags,
        affect_tags: memory.affect_tags,
        evidence_summary,
        metadata: memory.metadata,
    }
}

fn memory_context_pack_to_infos(pack: MemoryContextPack) -> Vec<MemoryNode> {
    let mut nodes = Vec::new();
    for snippet in pack
        .user_preferences
        .into_iter()
        .chain(pack.project_context)
        .chain(pack.relevant_episodes)
        .chain(pack.affective_guidance)
        .chain(pack.boundaries)
    {
        let mut node = MemoryNode::new(
            snippet.kind.clone(),
            snippet.kind.clone(),
            snippet.content,
            0.5,
        )
        .with_runtime_fields(
            snippet.kind,
            "user",
            snippet.confidence,
            0.5,
            "user_visible",
        )
        .with_metadata(serde_json::json!({ "source": snippet.source }));
        node.id = Some(snippet.id);
        nodes.push(node);
    }
    nodes
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

fn memory_privacy_response(
    memory: &crate::config::MemorySection,
    _capabilities: &MemoryCapabilities,
) -> MemoryPrivacyResponse {
    let supported_capture_modes = vec![
        MemoryCaptureMode::None,
        MemoryCaptureMode::ExplicitToolOnly,
        MemoryCaptureMode::LlmSelected,
    ];
    MemoryPrivacyResponse {
        enabled: memory.enabled,
        capture_mode: memory.capture_mode,
        automatic_capture_enabled: memory_effectively_enabled(memory)
            && matches!(memory.capture_mode, MemoryCaptureMode::LlmSelected),
        paused_until: memory
            .paused_until
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        explicit_memory_enabled: memory_effectively_enabled(memory),
        sensitive_auto_capture_allowed: false,
        sensitive_direct_entry_allowed: true,
        supported_capture_modes,
    }
}

fn memory_effectively_enabled(memory: &crate::config::MemorySection) -> bool {
    if !memory.enabled {
        return false;
    }
    let Some(paused_until) = memory.paused_until.as_deref().map(str::trim) else {
        return true;
    };
    if paused_until.is_empty() {
        return true;
    }
    chrono::DateTime::parse_from_rfc3339(paused_until)
        .map(|until| until.with_timezone(&chrono::Utc) <= chrono::Utc::now())
        .unwrap_or(true)
}
