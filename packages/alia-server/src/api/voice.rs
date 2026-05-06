use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct VoiceChatRequest {
    pub text: String,
}

#[derive(Serialize)]
pub struct VoiceChatResponse {
    pub text: String,
    pub conversation_id: Option<String>,
}

async fn voice_chat(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Json(_req): Json<VoiceChatRequest>,
) -> Result<Json<VoiceChatResponse>> {
    // TODO: implement full voice pipeline (router → cache → memory → persona → LLM)
    Ok(Json(VoiceChatResponse {
        text: "Voice chat not yet implemented".into(),
        conversation_id: None,
    }))
}

async fn transcribe(
    State(_state): State<AppState>,
    _auth: AuthUser,
    _body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>> {
    // TODO: forward to STT adapter
    Ok(Json(serde_json::json!({"text": ""})))
}

async fn synthesize(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Json(_body): Json<serde_json::Value>,
) -> Result<axum::body::Bytes> {
    // TODO: forward to TTS adapter
    Ok(axum::body::Bytes::new())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/voice/chat", post(voice_chat))
        .route("/voice/transcribe", post(transcribe))
        .route("/voice/synthesize", post(synthesize))
}
