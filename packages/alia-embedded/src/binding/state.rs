use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

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
    pub bound_since: Option<String>,
    #[serde(skip)]
    pub handoff_token: Option<Vec<u8>>,
    #[serde(skip)]
    pub handoff_expires_at: Option<u64>,
}

impl Default for BindingData {
    fn default() -> Self {
        Self {
            state: BindingState::Unbound,
            server_id: None,
            server_name: None,
            server_endpoint: None,
            bound_since: None,
            handoff_token: None,
            handoff_expires_at: None,
        }
    }
}

pub struct BindingManager {
    data: RwLock<BindingData>,
    persist_path: PathBuf,
    failed_attempts: RwLock<u32>,
    lockout_until: RwLock<Option<Instant>>,
    max_attempts: u32,
    lockout_duration_secs: u64,
}

impl BindingManager {
    pub fn new(persist_path: PathBuf, max_attempts: u32, lockout_duration_secs: u64) -> Self {
        Self {
            data: RwLock::new(BindingData::default()),
            persist_path,
            failed_attempts: RwLock::new(0),
            lockout_until: RwLock::new(None),
            max_attempts,
            lockout_duration_secs,
        }
    }

    /// Load binding state from disk, or start fresh.
    pub async fn load(&self) -> Result<()> {
        if self.persist_path.exists() {
            let content = tokio::fs::read_to_string(&self.persist_path)
                .await
                .with_context(|| {
                    format!("reading binding state: {}", self.persist_path.display())
                })?;
            let data: BindingData = serde_json::from_str(&content)?;
            *self.data.write().await = data;
        }
        Ok(())
    }

    async fn persist(&self) -> Result<()> {
        let data = self.data.read().await;
        if let Some(parent) = self.persist_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let json = serde_json::to_string_pretty(&*data)?;
        tokio::fs::write(&self.persist_path, json).await?;
        Ok(())
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
        _auth_token: &[u8],
    ) -> Result<()> {
        self.check_lockout().await?;

        let mut data = self.data.write().await;
        if data.state == BindingState::Bound {
            self.record_failure().await;
            bail!(
                "already bound to server {}",
                data.server_id.as_deref().unwrap_or("unknown")
            );
        }

        // TODO: validate auth_token against physical confirmation
        data.state = BindingState::Bound;
        data.server_id = Some(server_id.to_string());
        data.server_name = Some(server_name.to_string());
        data.server_endpoint = Some(server_endpoint.to_string());
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
        _new_server_id: &str,
        _new_endpoint: &str,
    ) -> Result<(Vec<u8>, u64)> {
        let mut data = self.data.write().await;

        if data.state != BindingState::Bound {
            bail!("not bound");
        }
        if data.server_id.as_deref() != Some(current_server_id) {
            bail!("server_id mismatch");
        }

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
    ) -> Result<()> {
        let mut data = self.data.write().await;

        if data.state != BindingState::HandoffPending {
            bail!("no handoff pending");
        }

        // Validate token
        let stored_token = data.handoff_token.as_deref().unwrap_or_default();
        if stored_token != token {
            bail!("invalid handoff token");
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

        data.state = BindingState::Bound;
        data.server_id = Some(new_server_id.to_string());
        data.server_name = Some(new_server_name.to_string());
        data.server_endpoint = Some(new_endpoint.to_string());
        data.bound_since = Some(chrono::Utc::now().to_rfc3339());
        data.handoff_token = None;
        data.handoff_expires_at = None;

        drop(data);
        self.persist().await?;

        info!(new_server_id, "handoff completed");
        Ok(())
    }

    /// Get current binding status.
    pub async fn status(&self) -> BindingData {
        self.data.read().await.clone()
    }
}
