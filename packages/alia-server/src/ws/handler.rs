use std::collections::HashSet;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::event::ServerEvent;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct WsParams {
    pub token: String,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum WsClientMessage {
    Subscribe { events: Vec<String> },
    Unsubscribe { events: Vec<String> },
    Ping,
}

const SEND_DEADLINE: Duration = Duration::from_secs(1);
const AUTH_RECHECK: Duration = Duration::from_secs(30);
const EVENT_TAGS: &[&str] = &[
    "device",
    "plugin",
    "voice",
    "rule",
    "insights",
    "scheduler",
    "stream",
    "other",
];

async fn authorize_diagnostics(state: &AppState, token: &str) -> Result<AuthUser, AppError> {
    let user = AuthUser::from_token(state, token).await?;
    // This bus includes global device, tool and conversation events. A scoped
    // end-user feed needs a separate contract in the later frontend round.
    if !user.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(user)
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(params): Query<WsParams>,
) -> Response {
    if state.cancel_token.is_cancelled() {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if let Err(error) = authorize_diagnostics(&state, &params.token).await {
        return error.into_response();
    }
    ws.max_message_size(16 * 1024)
        .max_frame_size(16 * 1024)
        .on_upgrade(move |socket| async move {
            let tasks = state.runtime_tasks.clone();
            // If shutdown sealed admission during the handshake, dropping the
            // rejected future also drops the upgraded socket.
            let _ = tasks.try_spawn("websocket", handle_socket(socket, state, params.token));
        })
}

fn event_type_tag(event: &ServerEvent) -> &'static str {
    match event {
        ServerEvent::DeviceOnline { .. }
        | ServerEvent::DeviceOffline { .. }
        | ServerEvent::DeviceDiscovered { .. } => "device",
        ServerEvent::PluginEvent { .. } => "plugin",
        ServerEvent::VoiceStateChanged { .. } => "voice",
        ServerEvent::RuleTriggered { .. } => "rule",
        ServerEvent::ConversationTurnCompleted { .. }
        | ServerEvent::InsightStarted { .. }
        | ServerEvent::InsightCompleted { .. } => "insights",
        ServerEvent::TaskScheduled { .. }
        | ServerEvent::TaskTriggered { .. }
        | ServerEvent::TaskCompleted { .. }
        | ServerEvent::TaskFailed { .. } => "scheduler",
        ServerEvent::StreamChunk { .. } | ServerEvent::StreamEnd { .. } => "stream",
        _ => "other",
    }
}

fn valid_tags(events: &[String]) -> bool {
    events.len() <= EVENT_TAGS.len() && events.iter().all(|tag| EVENT_TAGS.contains(&tag.as_str()))
}

/// A single tracked actor owns both halves. Socket writes have a deadline, so
/// a slow peer cannot prevent shutdown or require an unowned forwarding task.
async fn handle_socket(
    mut socket: WebSocket,
    state: AppState,
    token: String,
) -> anyhow::Result<()> {
    let mut event_rx = state.event_bus.subscribe();
    let mut subscriptions: Option<HashSet<String>> = None;
    let mut recheck = tokio::time::interval(AUTH_RECHECK);
    loop {
        let outgoing = tokio::select! {
            biased;
            _ = state.cancel_token.cancelled() => break,
            _ = recheck.tick() => {
                let authorized = tokio::select! {
                    _ = state.cancel_token.cancelled() => break,
                    auth = authorize_diagnostics(&state, &token) => auth.is_ok(),
                };
                if !authorized { break; }
                continue;
            }
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    let Ok(message) = serde_json::from_str::<WsClientMessage>(&text) else { break; };
                    match message {
                        WsClientMessage::Subscribe { events } => {
                            if !valid_tags(&events) { break; }
                            subscriptions.get_or_insert_with(HashSet::new).extend(events);
                            continue;
                        }
                        WsClientMessage::Unsubscribe { events } => {
                            if !valid_tags(&events) { break; }
                            // Unsubscribing from the implicit all-events filter
                            // means all known categories except these categories.
                            let filter = subscriptions.get_or_insert_with(|| {
                                EVENT_TAGS.iter().map(|tag| (*tag).to_owned()).collect()
                            });
                            for event in events { filter.remove(&event); }
                            continue;
                        }
                        WsClientMessage::Ping => Message::Text("{\"type\":\"pong\"}".into()),
                    }
                }
                Some(Ok(Message::Ping(bytes))) => Message::Pong(bytes),
                Some(Ok(Message::Pong(_))) => continue,
                _ => break,
            },
            event = event_rx.recv() => match event {
                Ok(event) => {
                    if subscriptions.as_ref().is_some_and(|filter| !filter.contains(event_type_tag(&event))) {
                        continue;
                    }
                    Message::Text(serde_json::to_string(&event)?.into())
                }
                // A diagnostics consumer that loses events must reconnect;
                // silently continuing would hide the gap in its observations.
                Err(broadcast::error::RecvError::Lagged(_))
                | Err(broadcast::error::RecvError::Closed) => break,
            },
        };
        let sent = tokio::select! {
            _ = state.cancel_token.cancelled() => break,
            result = tokio::time::timeout(SEND_DEADLINE, socket.send(outgoing)) => result,
        };
        if !matches!(sent, Ok(Ok(()))) {
            break;
        }
    }
    Ok(())
}
