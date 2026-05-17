use std::collections::HashSet;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::event::ServerEvent;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct WsParams {
    pub token: String,
}

/// Client → Server control messages
#[derive(Deserialize)]
#[serde(tag = "action")]
enum WsClientMessage {
    #[serde(rename = "subscribe")]
    Subscribe { events: Vec<String> },
    #[serde(rename = "unsubscribe")]
    Unsubscribe { events: Vec<String> },
    #[serde(rename = "ping")]
    Ping,
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(params): Query<WsParams>,
) -> Response {
    // Validate JWT from query parameter
    let claims = match state.jwt_keys.decode(&params.token) {
        Ok(c) => c,
        Err(_) => {
            return Response::builder()
                .status(401)
                .body(axum::body::Body::empty())
                .unwrap();
        }
    };

    tracing::info!(user_id = %claims.sub, "WebSocket connection established");
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Max number of pending messages before dropping
const BACKPRESSURE_LIMIT: usize = 64;

/// Extract the event type tag from a ServerEvent for filtering
fn event_type_tag(event: &ServerEvent) -> &'static str {
    match event {
        ServerEvent::DeviceOnline { .. }
        | ServerEvent::DeviceOffline { .. }
        | ServerEvent::DeviceDiscovered { .. } => "device",
        ServerEvent::PluginEvent { .. } => "plugin",
        ServerEvent::ProcessOutput { .. } | ServerEvent::ProcessStateChanged { .. } => "process",
        ServerEvent::VoiceStateChanged { .. } => "voice",
        ServerEvent::RuleTriggered { .. } => "rule",
        ServerEvent::InsightStarted { .. } | ServerEvent::InsightCompleted { .. } => "insights",
        ServerEvent::TaskScheduled { .. }
        | ServerEvent::TaskTriggered { .. }
        | ServerEvent::TaskCompleted { .. }
        | ServerEvent::TaskFailed { .. } => "scheduler",
        ServerEvent::StreamChunk { .. } | ServerEvent::StreamEnd { .. } => "stream",
        _ => "other",
    }
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut event_rx = state.event_bus.subscribe();

    // Subscription filter: None = all events, Some(set) = filtered
    let subscriptions = std::sync::Arc::new(tokio::sync::RwLock::new(None::<HashSet<String>>));
    let sub_for_send = subscriptions.clone();

    // Spawn task to forward events to WebSocket with backpressure
    let send_task = tokio::spawn(async move {
        let mut pending = 0usize;

        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    // Check subscription filter
                    let subs = sub_for_send.read().await;
                    if let Some(ref filter) = *subs {
                        if !filter.contains(event_type_tag(&event)) {
                            continue;
                        }
                    }
                    drop(subs);

                    // Backpressure: drop if too many pending
                    if pending >= BACKPRESSURE_LIMIT {
                        tracing::warn!(pending, "WebSocket backpressure: dropping event");
                        continue;
                    }

                    let msg = serde_json::to_string(&event).unwrap_or_default();
                    pending += 1;
                    if sender.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                    pending = pending.saturating_sub(1);
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "WebSocket subscriber lagged");
                    // Reset pending count after lag
                    pending = 0;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Handle incoming messages from client (subscribe/unsubscribe/ping)
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(client_msg) = serde_json::from_str::<WsClientMessage>(&text) {
                    match client_msg {
                        WsClientMessage::Subscribe { events } => {
                            let mut subs = subscriptions.write().await;
                            let filter = subs.get_or_insert_with(HashSet::new);
                            filter.extend(events);
                        }
                        WsClientMessage::Unsubscribe { events } => {
                            let mut subs = subscriptions.write().await;
                            if let Some(ref mut filter) = *subs {
                                for e in &events {
                                    filter.remove(e);
                                }
                                if filter.is_empty() {
                                    *subs = None;
                                }
                            }
                        }
                        WsClientMessage::Ping => {
                            // Client-side keepalive, no action needed
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    send_task.abort();
}
