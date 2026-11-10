use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::entity::{chat_message, chat_session, conversation_event};
use crate::error::{AppError, ErrorResponse, Result};
use crate::expression::{ExpressionTrace, ExpressionTraceEvent, expression_traces_from_events};
use crate::state::AppState;

#[derive(Serialize, ToSchema)]
pub struct Conversation {
    pub id: Uuid,
    pub channel_id: String,
    pub creator_id: Option<String>,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct Message {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub user_id: Option<String>,
    pub created_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct AgentEvent {
    pub id: Uuid,
    pub turn_id: Uuid,
    pub sequence: i32,
    pub event_type: String,
    pub actor: String,
    pub summary: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Maximum records to return; zero is accepted. The endpoint clamps larger values.
    limit: Option<u64>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TraceQuery {
    /// Maximum source events; defaults to 1000 and clamps to 5000. Zero is accepted.
    limit: Option<u64>,
    turn_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
pub struct DeleteConversationResult {
    pub success: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/conversations",
    operation_id = "list_conversations",
    tag = "conversations",
    security(("bearer" = [])),
    params(ListQuery),
    responses(
        (status = 200, description = "Conversations ordered by most recently updated; administrators see all, others only their own. limit defaults to 100 and clamps to 500.", body = [Conversation]),
        (status = 400, description = "Malformed UUID or query: Axum plain-text rejection", body = String, content_type = "text/plain"),
        (status = 500, description = "Account or conversation database query failed", body = ErrorResponse)
    )
)]
pub async fn list_conversations(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Conversation>>> {
    let mut select = chat_session::Entity::find().order_by_desc(chat_session::Column::UpdatedAt);
    if !auth.is_admin() {
        select = select.filter(chat_session::Column::CreatorId.eq(auth.id.to_string()));
    }

    let sessions = select
        .limit(query.limit.unwrap_or(100).min(500))
        .all(&state.db)
        .await
        .map_err(AppError::Database)?;

    Ok(Json(
        sessions.into_iter().map(conversation_response).collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/conversations/{id}",
    operation_id = "get_conversation",
    tag = "conversations",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Conversation identity")),
    responses(
        (status = 200, description = "Conversation owned by this user, or any conversation for an administrator.", body = Conversation),
        (status = 400, description = "Malformed UUID or query: Axum plain-text rejection", body = String, content_type = "text/plain"),
        (status = 403, description = "Conversation belongs to another user; administrators are allowed", body = ErrorResponse),
        (status = 404, description = "Conversation does not exist", body = ErrorResponse),
        (status = 500, description = "Account or conversation database query failed", body = ErrorResponse)
    )
)]
pub async fn get_conversation(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Conversation>> {
    let session = load_authorized_session(&state, &auth, id).await?;
    Ok(Json(conversation_response(session)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/conversations/{id}",
    operation_id = "delete_conversation",
    tag = "conversations",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Conversation identity")),
    responses(
        (status = 200, description = "Conversation deleted after owner or administrator authorization.", body = DeleteConversationResult),
        (status = 400, description = "Malformed UUID or query: Axum plain-text rejection", body = String, content_type = "text/plain"),
        (status = 403, description = "Conversation belongs to another user; administrators are allowed", body = ErrorResponse),
        (status = 404, description = "Conversation does not exist", body = ErrorResponse),
        (status = 500, description = "Account or conversation database query failed", body = ErrorResponse)
    )
)]
pub async fn delete_conversation(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<DeleteConversationResult>> {
    let _ = load_authorized_session(&state, &auth, id).await?;
    chat_session::Entity::delete_by_id(id)
        .exec(&state.db)
        .await
        .map_err(AppError::Database)?;
    Ok(Json(DeleteConversationResult { success: true }))
}

#[utoipa::path(
    get,
    path = "/api/v1/conversations/{id}/messages",
    operation_id = "list_conversation_messages",
    tag = "conversations",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Conversation identity"), ListQuery),
    responses(
        (status = 200, description = "Messages ordered by creation time ascending. limit defaults to 1000 and clamps to 5000.", body = [Message]),
        (status = 400, description = "Malformed UUID or query: Axum plain-text rejection", body = String, content_type = "text/plain"),
        (status = 403, description = "Conversation belongs to another user; administrators are allowed", body = ErrorResponse),
        (status = 404, description = "Conversation does not exist", body = ErrorResponse),
        (status = 500, description = "Account or conversation database query failed", body = ErrorResponse)
    )
)]
pub async fn get_messages(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Message>>> {
    let _ = load_authorized_session(&state, &auth, id).await?;
    let messages = chat_message::Entity::find()
        .filter(chat_message::Column::SessionId.eq(id))
        .order_by_asc(chat_message::Column::CreatedAt)
        .limit(query.limit.unwrap_or(1000).min(5000))
        .all(&state.db)
        .await
        .map_err(AppError::Database)?;

    Ok(Json(messages.into_iter().map(message_response).collect()))
}

#[utoipa::path(
    get,
    path = "/api/v1/conversations/{id}/events",
    operation_id = "list_conversation_events",
    tag = "conversations",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Conversation identity"), ListQuery),
    responses(
        (status = 200, description = "Events ordered by creation time and sequence ascending. limit defaults to 1000 and clamps to 5000.", body = [AgentEvent]),
        (status = 400, description = "Malformed UUID or query: Axum plain-text rejection", body = String, content_type = "text/plain"),
        (status = 403, description = "Conversation belongs to another user; administrators are allowed", body = ErrorResponse),
        (status = 404, description = "Conversation does not exist", body = ErrorResponse),
        (status = 500, description = "Account or conversation database query failed", body = ErrorResponse)
    )
)]
pub async fn get_events(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<AgentEvent>>> {
    let _ = load_authorized_session(&state, &auth, id).await?;
    let events = conversation_event::Entity::find()
        .filter(conversation_event::Column::SessionId.eq(id))
        .order_by_asc(conversation_event::Column::CreatedAt)
        .order_by_asc(conversation_event::Column::Sequence)
        .limit(query.limit.unwrap_or(1000).min(5000))
        .all(&state.db)
        .await
        .map_err(AppError::Database)?;

    Ok(Json(events.into_iter().map(agent_event_response).collect()))
}

#[utoipa::path(
    get,
    path = "/api/v1/conversations/{id}/expression-traces",
    operation_id = "list_conversation_expression_traces",
    tag = "conversations",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Conversation identity"), TraceQuery),
    responses(
        (status = 200, description = "Expression traces grouped and ordered by turn UUID, with source events ordered by sequence. limit applies to source events, not traces.", body = [ExpressionTrace]),
        (status = 400, description = "Malformed UUID or query: Axum plain-text rejection", body = String, content_type = "text/plain"),
        (status = 403, description = "Conversation belongs to another user; administrators are allowed", body = ErrorResponse),
        (status = 404, description = "Conversation does not exist", body = ErrorResponse),
        (status = 500, description = "Account or conversation database query failed", body = ErrorResponse)
    )
)]
pub async fn get_expression_traces(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<TraceQuery>,
) -> Result<Json<Vec<ExpressionTrace>>> {
    let _ = load_authorized_session(&state, &auth, id).await?;
    let mut select = conversation_event::Entity::find()
        .filter(conversation_event::Column::SessionId.eq(id))
        .filter(conversation_event::Column::EventType.is_in(expression_trace_event_types()));
    if let Some(turn_id) = query.turn_id {
        select = select.filter(conversation_event::Column::TurnId.eq(turn_id));
    }
    let events = select
        .order_by_asc(conversation_event::Column::CreatedAt)
        .order_by_asc(conversation_event::Column::Sequence)
        .limit(query.limit.unwrap_or(1000).min(5000))
        .all(&state.db)
        .await
        .map_err(AppError::Database)?
        .into_iter()
        .map(expression_trace_event)
        .collect();

    Ok(Json(expression_traces_from_events(events)))
}

async fn load_authorized_session(
    state: &AppState,
    auth: &AuthUser,
    id: Uuid,
) -> Result<chat_session::Model> {
    let session = chat_session::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(AppError::Database)?
        .ok_or_else(|| AppError::NotFound("Conversation not found".into()))?;

    if auth.is_admin() || session.creator_id.as_deref() == Some(auth.id.to_string().as_str()) {
        Ok(session)
    } else {
        Err(AppError::Forbidden)
    }
}

fn conversation_response(session: chat_session::Model) -> Conversation {
    Conversation {
        id: session.id,
        channel_id: session.channel_id,
        creator_id: session.creator_id,
        title: session.name.unwrap_or_else(|| "New Chat".to_string()),
        created_at: session.created_at.to_rfc3339(),
        updated_at: session.updated_at.to_rfc3339(),
    }
}

fn message_response(message: chat_message::Model) -> Message {
    Message {
        id: message.id,
        role: message.role,
        content: message.content,
        user_id: message.user_id,
        created_at: message.created_at.to_rfc3339(),
    }
}

fn agent_event_response(event: conversation_event::Model) -> AgentEvent {
    AgentEvent {
        id: event.id,
        turn_id: event.turn_id,
        sequence: event.sequence,
        event_type: event.event_type,
        actor: event.actor,
        summary: event.summary,
        payload: event.payload,
        created_at: event.created_at.to_rfc3339(),
    }
}

fn expression_trace_event(event: conversation_event::Model) -> ExpressionTraceEvent {
    ExpressionTraceEvent {
        session_id: Some(event.session_id),
        turn_id: event.turn_id,
        sequence: event.sequence,
        event_type: event.event_type,
        actor: event.actor,
        summary: event.summary,
        payload: event.payload,
        created_at: Some(event.created_at.to_rfc3339()),
    }
}

fn expression_trace_event_types() -> [&'static str; 2] {
    ["tts_provider_adjustments", "sentence_expression_llm"]
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/conversations", get(list_conversations))
        .route(
            "/conversations/{id}",
            get(get_conversation).delete(delete_conversation),
        )
        .route("/conversations/{id}/messages", get(get_messages))
        .route("/conversations/{id}/events", get(get_events))
        .route(
            "/conversations/{id}/expression-traces",
            get(get_expression_traces),
        )
}
