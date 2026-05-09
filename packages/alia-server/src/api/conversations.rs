use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

#[derive(Serialize)]
pub struct Conversation {
    pub id: Uuid,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct Message {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

async fn list_conversations(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<Conversation>>> {
    Ok(Json(Vec::new()))
}

async fn get_conversation(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<Conversation>> {
    Err(crate::error::AppError::NotFound(
        "Conversation not found".into(),
    ))
}

async fn delete_conversation(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({"success": true})))
}

async fn get_messages(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_id): Path<Uuid>,
) -> Result<Json<Vec<Message>>> {
    Ok(Json(Vec::new()))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/conversations", get(list_conversations))
        .route(
            "/conversations/{id}",
            get(get_conversation).delete(delete_conversation),
        )
        .route("/conversations/{id}/messages", get(get_messages))
}
