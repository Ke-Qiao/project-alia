use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct WsParams {
    pub token: String,
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

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut event_rx = state.event_bus.subscribe();

    // Spawn task to forward events to WebSocket
    let send_task = tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    let msg = serde_json::to_string(&event).unwrap_or_default();
                    if sender.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "WebSocket subscriber lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Handle incoming messages from client
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(_text) => {
                // TODO: handle subscribe/unsubscribe messages
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    send_task.abort();
}
