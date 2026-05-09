use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::error::{AppError, Result};
use crate::providers::adapter::AudioEncoding;
use crate::state::AppState;

// ========== Request / Response types ==========

#[derive(Deserialize)]
pub struct VoiceChatRequest {
    /// Raw text to process (text-only voice chat, skips STT).
    pub text: Option<String>,
    /// Base64-encoded audio data (if provided, STT is invoked first).
    pub audio_base64: Option<String>,
    /// Audio encoding format (required if audio_base64 is set).
    pub encoding: Option<AudioEncoding>,
}

#[derive(Serialize)]
pub struct VoiceChatResponse {
    /// AI response text.
    pub text: String,
    /// Base64-encoded TTS audio (only if the channel supports voice_output).
    pub audio_base64: Option<String>,
    pub conversation_id: Option<String>,
}

#[derive(Deserialize)]
pub struct TranscribeRequest {
    /// Audio encoding of the uploaded body.
    #[serde(default = "default_encoding")]
    pub encoding: AudioEncoding,
}

fn default_encoding() -> AudioEncoding {
    AudioEncoding::Pcm16kMono
}

#[derive(Serialize)]
pub struct TranscribeResponse {
    pub text: String,
}

#[derive(Deserialize)]
pub struct SynthesizeRequest {
    pub text: String,
}

// ========== Handlers ==========

/// Full voice pipeline: [audio → STT] → Router → LLM → [TTS → audio]
///
/// The pipeline gates STT/TTS on the target channel's capabilities.
/// If called without a channel_id, it always runs STT/TTS when audio is present.
async fn voice_chat(
    State(state): State<AppState>,
    _auth: AuthUser,
    channel_id: Option<Path<String>>,
    Json(req): Json<VoiceChatRequest>,
) -> Result<Json<VoiceChatResponse>> {
    // Resolve channel capabilities (if a channel is targeted)
    let capabilities = if let Some(Path(ref id)) = channel_id {
        let ch = state
            .channels
            .get(id)
            .await
            .ok_or_else(|| AppError::NotFound(format!("channel '{}' not found", id)))?;
        Some(ch.capabilities())
    } else {
        None
    };

    // --- STT stage: audio → text ---
    let input_text = if let Some(audio_b64) = &req.audio_base64 {
        // Check channel supports voice input (if scoped to a channel)
        if let Some(ref caps) = capabilities {
            if !caps.voice_input {
                return Err(AppError::Validation(
                    "target channel does not support voice input".into(),
                ));
            }
        }

        let encoding = req.encoding.unwrap_or(AudioEncoding::Pcm16kMono);
        let audio_bytes = base64_decode(audio_b64)?;
        let transcription = state
            .modules
            .stt
            .transcribe(&audio_bytes, encoding)
            .await
            .map_err(|e| AppError::AiService(format!("STT failed: {e}")))?;

        if transcription.is_empty() {
            return Err(AppError::Validation("no speech detected in audio".into()));
        }
        transcription
    } else if let Some(text) = req.text {
        if text.trim().is_empty() {
            return Err(AppError::Validation("text or audio_base64 required".into()));
        }
        text
    } else {
        return Err(AppError::Validation("text or audio_base64 required".into()));
    };

    // --- LLM stage: text → response ---
    let messages = vec![crate::providers::adapter::ChatMessage {
        role: "user".into(),
        content: input_text.clone(),
    }];
    let response_text = state
        .modules
        .llm
        .chat(&messages)
        .await
        .map_err(|e| AppError::AiService(format!("LLM failed: {e}")))?;

    // --- TTS stage: response → audio (only if channel supports voice output) ---
    let audio_base64 = if should_synthesize(&capabilities) {
        let audio_bytes = state
            .modules
            .tts
            .synthesize(&response_text)
            .await
            .map_err(|e| AppError::AiService(format!("TTS failed: {e}")))?;
        if audio_bytes.is_empty() {
            None
        } else {
            use base64::Engine;
            Some(base64::engine::general_purpose::STANDARD.encode(&audio_bytes))
        }
    } else {
        None
    };

    Ok(Json(VoiceChatResponse {
        text: response_text,
        audio_base64,
        conversation_id: None,
    }))
}

/// STT endpoint: raw audio body → text transcription.
///
/// Standalone transcribe — useful for channels that capture audio locally
/// and only need the text back.
async fn transcribe(
    State(state): State<AppState>,
    _auth: AuthUser,
    axum::extract::Query(params): axum::extract::Query<TranscribeRequest>,
    body: axum::body::Bytes,
) -> Result<Json<TranscribeResponse>> {
    if body.is_empty() {
        return Err(AppError::Validation("empty audio body".into()));
    }

    let text = state
        .modules
        .stt
        .transcribe(&body, params.encoding)
        .await
        .map_err(|e| AppError::AiService(format!("STT failed: {e}")))?;

    Ok(Json(TranscribeResponse { text }))
}

/// TTS endpoint: text → synthesized audio bytes.
async fn synthesize(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(req): Json<SynthesizeRequest>,
) -> Result<axum::body::Bytes> {
    if req.text.trim().is_empty() {
        return Err(AppError::Validation("text is required".into()));
    }

    let audio = state
        .modules
        .tts
        .synthesize(&req.text)
        .await
        .map_err(|e| AppError::AiService(format!("TTS failed: {e}")))?;

    Ok(axum::body::Bytes::from(audio))
}

// ========== Routes ==========

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/voice/chat", post(voice_chat_global))
        .route("/voice/chat/{channel_id}", post(voice_chat_with_channel))
        .route("/voice/transcribe", post(transcribe))
        .route("/voice/synthesize", post(synthesize))
}

/// Voice chat without targeting a specific channel (always runs full pipeline).
async fn voice_chat_global(
    state: State<AppState>,
    auth: AuthUser,
    req: Json<VoiceChatRequest>,
) -> Result<Json<VoiceChatResponse>> {
    voice_chat(state, auth, None, req).await
}

/// Voice chat targeting a specific channel (gates STT/TTS on capabilities).
async fn voice_chat_with_channel(
    state: State<AppState>,
    auth: AuthUser,
    channel_id: Path<String>,
    req: Json<VoiceChatRequest>,
) -> Result<Json<VoiceChatResponse>> {
    voice_chat(state, auth, Some(channel_id), req).await
}

// ========== Helpers ==========

/// Decide whether to run TTS based on channel capabilities.
/// If no channel is specified (direct API call), always synthesize.
fn should_synthesize(capabilities: &Option<crate::channels::adapter::ChannelCapabilities>) -> bool {
    match capabilities {
        Some(caps) => caps.voice_output,
        None => true, // direct API call: always provide audio
    }
}

fn base64_decode(input: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|e| AppError::Validation(format!("invalid base64 audio: {e}")))
}
