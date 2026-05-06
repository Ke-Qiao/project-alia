use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

use crate::event::EventBus;

#[derive(Debug, Clone)]
pub enum ConnectionStatus {
    Connected,
    Disconnected { since: DateTime<Utc>, reason: String },
    Reconnecting { attempt: u32 },
}

/// gRPC client wrapper for communicating with alia-embedded
#[derive(Clone)]
pub struct EmbeddedClient {
    endpoint: String,
    status: Arc<RwLock<ConnectionStatus>>,
    event_bus: EventBus,
}

impl EmbeddedClient {
    pub fn new(endpoint: &str, event_bus: EventBus) -> Self {
        let status = if endpoint.is_empty() {
            ConnectionStatus::Disconnected {
                since: Utc::now(),
                reason: "no endpoint configured".into(),
            }
        } else {
            ConnectionStatus::Reconnecting { attempt: 0 }
        };

        Self {
            endpoint: endpoint.to_string(),
            status: Arc::new(RwLock::new(status)),
            event_bus,
        }
    }

    pub async fn connection_status(&self) -> ConnectionStatus {
        self.status.read().await.clone()
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}
