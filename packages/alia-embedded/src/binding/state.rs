use std::time::Instant;

use alia_transport::{
    DEFAULT_AUTH_MAX_SKEW, bind_payload, ensure_recent_timestamp, handoff_completion_payload,
    handoff_intent_payload, verify_auth_token, verify_payload,
};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::module::traits::DiscoveryMetadata;
use crate::storage::EmbeddedStore;

const BINDING_STATE_KEY: &str = "binding_state";

/// Binding state machine: UNBOUND → BOUND, BOUND → HANDOFF_PENDING → BOUND (new server).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingState {
    Unbound,
    Bound,
    HandoffPending,
}

/// Persistent binding data written to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingData {
    pub state: BindingState,
    pub server_id: Option<String>,
    pub server_name: Option<String>,
    pub server_endpoint: Option<String>,
    pub server_public_key: Option<Vec<u8>>,
    pub bound_since: Option<String>,
    #[serde(skip)]
    pub dev_skip_auth: bool,
    #[serde(skip)]
    pub handoff_token: Option<Vec<u8>>,
    #[serde(skip)]
    pub handoff_expires_at: Option<u64>,
    #[serde(skip)]
    pub pending_server_id: Option<String>,
    #[serde(skip)]
    pub pending_server_name: Option<String>,
    #[serde(skip)]
    pub pending_server_endpoint: Option<String>,
    #[serde(skip)]
    pub pending_server_public_key: Option<Vec<u8>>,
}

impl BindingData {
    pub fn discovery_metadata(&self) -> DiscoveryMetadata {
        let status = match self.state {
            BindingState::Unbound => "unbound",
            BindingState::Bound => "bound",
            BindingState::HandoffPending => "handoff_pending",
        };

        DiscoveryMetadata {
            status: status.to_string(),
            server_id: self.server_id.clone(),
            capabilities: Vec::new(),
            attributes: HashMap::new(),
        }
    }
}

impl Default for BindingData {
    fn default() -> Self {
        Self {
            state: BindingState::Unbound,
            server_id: None,
            server_name: None,
            server_endpoint: None,
            server_public_key: None,
            bound_since: None,
            dev_skip_auth: false,
            handoff_token: None,
            handoff_expires_at: None,
            pending_server_id: None,
            pending_server_name: None,
            pending_server_endpoint: None,
            pending_server_public_key: None,
        }
    }
}

pub struct BindingManager {
    data: RwLock<BindingData>,
    store: EmbeddedStore,
    failed_attempts: RwLock<u32>,
    lockout_until: RwLock<Option<Instant>>,
    max_attempts: u32,
    lockout_duration_secs: u64,
}

impl BindingManager {
    pub fn new(store: EmbeddedStore, max_attempts: u32, lockout_duration_secs: u64) -> Self {
        Self {
            data: RwLock::new(BindingData::default()),
            store,
            failed_attempts: RwLock::new(0),
            lockout_until: RwLock::new(None),
            max_attempts,
            lockout_duration_secs,
        }
    }

    /// Load binding state from SQLite, or start fresh if no state exists.
    pub async fn load(&self) -> Result<()> {
        if let Some(data) = self
            .store
            .load_json::<BindingData>(BINDING_STATE_KEY)
            .await?
        {
            *self.data.write().await = data;
        }

        Ok(())
    }

    async fn persist(&self) -> Result<()> {
        let data = self.data.read().await.clone();
        self.store.save_json(BINDING_STATE_KEY, &data).await
    }

    /// Check if currently locked out due to too many failed attempts.
    async fn check_lockout(&self) -> Result<()> {
        let lockout = self.lockout_until.read().await;
        if let Some(until) = *lockout {
            if Instant::now() < until {
                bail!("too many failed bind attempts, locked out");
            }
        }
        Ok(())
    }

    async fn record_failure(&self) {
        let mut attempts = self.failed_attempts.write().await;
        *attempts += 1;
        if *attempts >= self.max_attempts {
            let mut lockout = self.lockout_until.write().await;
            *lockout =
                Some(Instant::now() + std::time::Duration::from_secs(self.lockout_duration_secs));
            *attempts = 0;
            warn!("bind lockout activated for {}s", self.lockout_duration_secs);
        }
    }

    async fn reset_failures(&self) {
        *self.failed_attempts.write().await = 0;
        *self.lockout_until.write().await = None;
    }

    /// Bind to a server.
    pub async fn bind(
        &self,
        server_id: &str,
        server_name: &str,
        server_endpoint: &str,
        server_public_key: &[u8],
        auth_token: &[u8],
        issued_at: u64,
    ) -> Result<()> {
        self.check_lockout().await?;
        ensure_recent_timestamp(issued_at, DEFAULT_AUTH_MAX_SKEW)?;

        let payload = bind_payload(
            server_id,
            server_name,
            server_endpoint,
            server_public_key,
            issued_at,
        );
        verify_payload(server_public_key, &payload, auth_token)?;

        let mut data = self.data.write().await;
        if data.state == BindingState::Bound {
            self.record_failure().await;
            bail!(
                "already bound to server {}",
                data.server_id.as_deref().unwrap_or("unknown")
            );
        }

        data.state = BindingState::Bound;
        data.server_id = Some(server_id.to_string());
        data.server_name = Some(server_name.to_string());
        data.server_endpoint = Some(server_endpoint.to_string());
        data.server_public_key = Some(server_public_key.to_vec());
        data.bound_since = Some(chrono::Utc::now().to_rfc3339());

        drop(data);
        self.persist().await?;
        self.reset_failures().await;

        info!(server_id, "device bound");
        Ok(())
    }

    /// Unbind from the current server.
    pub async fn unbind(&self, server_id: &str) -> Result<()> {
        let mut data = self.data.write().await;

        match data.state {
            BindingState::Bound => {
                if data.server_id.as_deref() != Some(server_id) {
                    bail!("server_id mismatch");
                }
            }
            _ => bail!("not bound"),
        }

        *data = BindingData::default();
        drop(data);
        self.persist().await?;

        info!(server_id, "device unbound");
        Ok(())
    }

    /// Prepare handoff to a new server. Returns a handoff token.
    pub async fn prepare_handoff(
        &self,
        current_server_id: &str,
        new_server_id: &str,
        new_server_name: &str,
        new_endpoint: &str,
        new_server_public_key: &[u8],
        authorization: &[u8],
        issued_at: u64,
    ) -> Result<(Vec<u8>, u64)> {
        let mut data = self.data.write().await;
        ensure_recent_timestamp(issued_at, DEFAULT_AUTH_MAX_SKEW)?;

        if data.state != BindingState::Bound {
            bail!("not bound");
        }
        if data.server_id.as_deref() != Some(current_server_id) {
            bail!("server_id mismatch");
        }
        let current_server_public_key = data
            .server_public_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("missing bound server public key"))?;
        let payload = handoff_intent_payload(
            current_server_id,
            new_server_id,
            new_server_name,
            new_endpoint,
            new_server_public_key,
            issued_at,
        );
        verify_payload(current_server_public_key, &payload, authorization)?;

        // Generate a random token (in production, sign with device key)
        let token: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 300; // 5 minutes

        data.state = BindingState::HandoffPending;
        data.handoff_token = Some(token.clone());
        data.handoff_expires_at = Some(expires_at);
        data.pending_server_id = Some(new_server_id.to_string());
        data.pending_server_name = Some(new_server_name.to_string());
        data.pending_server_endpoint = Some(new_endpoint.to_string());
        data.pending_server_public_key = Some(new_server_public_key.to_vec());

        drop(data);
        self.persist().await?;

        info!("handoff prepared, token valid for 300s");
        Ok((token, expires_at))
    }

    /// Complete handoff with a valid token.
    pub async fn complete_handoff(
        &self,
        token: &[u8],
        new_server_id: &str,
        new_server_name: &str,
        new_endpoint: &str,
        new_server_public_key: &[u8],
        authorization: &[u8],
        issued_at: u64,
    ) -> Result<()> {
        let mut data = self.data.write().await;
        ensure_recent_timestamp(issued_at, DEFAULT_AUTH_MAX_SKEW)?;

        if data.state != BindingState::HandoffPending {
            bail!("no handoff pending");
        }

        // Validate token
        let stored_token = data.handoff_token.as_deref().unwrap_or_default();
        if stored_token != token {
            bail!("invalid handoff token");
        }
        if data.pending_server_id.as_deref() != Some(new_server_id) {
            bail!("new_server_id mismatch");
        }
        if data.pending_server_name.as_deref() != Some(new_server_name) {
            bail!("new_server_name mismatch");
        }
        if data.pending_server_endpoint.as_deref() != Some(new_endpoint) {
            bail!("new_server_endpoint mismatch");
        }
        if data.pending_server_public_key.as_deref() != Some(new_server_public_key) {
            bail!("new_server_public_key mismatch");
        }

        // Check expiry
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if let Some(expires) = data.handoff_expires_at {
            if now > expires {
                data.state = BindingState::Bound; // Revert to previous bound state
                drop(data);
                self.persist().await?;
                bail!("handoff token expired");
            }
        }

        let payload = handoff_completion_payload(
            token,
            new_server_id,
            new_server_name,
            new_endpoint,
            new_server_public_key,
            issued_at,
        );
        verify_payload(new_server_public_key, &payload, authorization)?;

        data.state = BindingState::Bound;
        data.server_id = Some(new_server_id.to_string());
        data.server_name = Some(new_server_name.to_string());
        data.server_endpoint = Some(new_endpoint.to_string());
        data.server_public_key = Some(new_server_public_key.to_vec());
        data.bound_since = Some(chrono::Utc::now().to_rfc3339());
        data.handoff_token = None;
        data.handoff_expires_at = None;
        data.pending_server_id = None;
        data.pending_server_name = None;
        data.pending_server_endpoint = None;
        data.pending_server_public_key = None;

        drop(data);
        self.persist().await?;

        info!(new_server_id, "handoff completed");
        Ok(())
    }

    /// Get current binding status.
    pub async fn status(&self) -> BindingData {
        self.data.read().await.clone()
    }

    /// Seed a local development binding without cryptographic handshaking.
    /// This is intentionally explicit and config-gated for same-machine testing.
    pub async fn seed_dev_binding(
        &self,
        server_id: &str,
        server_name: &str,
        server_endpoint: &str,
    ) -> Result<()> {
        let mut data = self.data.write().await;
        data.state = BindingState::Bound;
        data.server_id = Some(server_id.to_string());
        data.server_name = Some(server_name.to_string());
        data.server_endpoint = Some(server_endpoint.to_string());
        data.server_public_key = None;
        data.bound_since = Some(chrono::Utc::now().to_rfc3339());
        data.dev_skip_auth = true;

        drop(data);
        self.persist().await?;
        info!(server_id, server_endpoint, "dev binding seeded");
        Ok(())
    }

    pub async fn verify_grpc_auth(&self, token: &str, required_scope: &str) -> Result<()> {
        let data = self.data.read().await;
        if data.dev_skip_auth {
            warn!(required_scope, "skipping gRPC auth for local dev binding");
            return Ok(());
        }
        let server_id = data
            .server_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("device is not bound to a server"))?;
        let server_public_key = data
            .server_public_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("missing bound server public key"))?;

        verify_auth_token(
            token,
            server_public_key,
            server_id,
            required_scope,
            DEFAULT_AUTH_MAX_SKEW,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_manager(dir: &tempfile::TempDir) -> BindingManager {
        let store = EmbeddedStore::open(dir.path().join("embedded-state.sqlite3"))
            .await
            .unwrap();
        BindingManager::new(store, 3, 60)
    }

    #[tokio::test]
    async fn persists_binding_state_to_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let manager = test_manager(&dir).await;

        manager.data.write().await.state = BindingState::Bound;
        manager.data.write().await.server_id = Some("server-1".to_string());
        manager.persist().await.unwrap();

        let reloaded = test_manager(&dir).await;
        reloaded.load().await.unwrap();
        let status = reloaded.status().await;

        assert_eq!(status.state, BindingState::Bound);
        assert_eq!(status.server_id.as_deref(), Some("server-1"));
    }
}
