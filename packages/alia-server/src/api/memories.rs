use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

#[derive(Serialize)]
pub struct MemoryInfo {
    pub id: Uuid,
    pub memory_layer: String,
    pub node_type: String,
    pub content: String,
    pub importance: f32,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct GraphData {
    pub nodes: Vec<serde_json::Value>,
    pub edges: Vec<serde_json::Value>,
}

async fn list_memories(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<MemoryInfo>>> {
    Ok(Json(Vec::new()))
}

async fn get_memory(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<MemoryInfo>> {
    Err(crate::error::AppError::NotFound("Memory not found".into()))
}

async fn update_memory(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_id): Path<Uuid>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<MemoryInfo>> {
    Err(crate::error::AppError::NotFound("Memory not found".into()))
}

async fn delete_memory(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({"success": true})))
}

async fn get_graph(State(_state): State<AppState>, _auth: AuthUser) -> Result<Json<GraphData>> {
    Ok(Json(GraphData {
        nodes: Vec::new(),
        edges: Vec::new(),
    }))
}

async fn search_memories(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<Vec<MemoryInfo>>> {
    Ok(Json(Vec::new()))
}

async fn clear_memories(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({"success": true})))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/memories", get(list_memories).delete(clear_memories))
        .route(
            "/memories/{id}",
            get(get_memory).patch(update_memory).delete(delete_memory),
        )
        .route("/memories/graph", get(get_graph))
        .route("/memories/search", post(search_memories))
}
