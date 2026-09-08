use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use futures::FutureExt;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::module::lifecycle::ModuleHealth;

use super::adapter::{ChannelAdapter, ChannelTarget};

/// A registry retains retired entries until shutdown has joined their cleanup.
/// An old Arc cannot make a stopped entry start again through this registry.
pub struct ChannelRegistry {
    channels: RwLock<HashMap<String, Arc<ChannelEntry>>>,
    disabled: RwLock<HashMap<String, DisabledChannelInfo>>,
    admission: CancellationToken,
    admission_gate: std::sync::Mutex<()>,
    restart_gates: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

struct ChannelEntry {
    channel: Arc<dyn ChannelAdapter>,
    cancel_start: CancellationToken,
    retired: AtomicBool,
    work: Mutex<ChannelWork>,
}

#[derive(Default)]
struct ChannelWork {
    start_attempted: bool,
    starting: Option<JoinHandle<anyhow::Result<()>>>,
    stopping: Option<JoinHandle<anyhow::Result<()>>>,
    stopped: Option<Result<(), String>>,
}

impl ChannelEntry {
    async fn start(&self) -> anyhow::Result<()> {
        let mut work = self.work.lock().await;
        anyhow::ensure!(
            !self.retired.load(Ordering::Acquire) && !self.cancel_start.is_cancelled(),
            "channel is stopping"
        );
        anyhow::ensure!(
            !work.start_attempted,
            "channel start has already been attempted"
        );
        work.start_attempted = true;
        let channel = self.channel.clone();
        let cancel = self.cancel_start.clone();
        work.starting = Some(tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Ok(()),
                result = channel.start() => result,
            }
        }));
        let result = work.starting.as_mut().unwrap().await;
        work.starting.take();
        drop(work);
        match result
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
        {
            Ok(()) if !self.cancel_start.is_cancelled() => Ok(()),
            Ok(()) => {
                self.stop().await?;
                anyhow::bail!("channel startup cancelled");
            }
            Err(error) => match self.stop().await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error.context(format!("channel startup cleanup: {cleanup:#}"))),
            },
        }
    }

    fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        self.cancel_start.cancel();
    }

    async fn request_stop(&self) {
        self.retire();
        let mut work = self.work.lock().await;
        if work.stopping.is_none() && work.stopped.is_none() {
            // Move an abandoned startup handle directly into its owned cleanup
            // task. No await occurs between moving it and retaining the new handle.
            let starting = work.starting.take();
            let channel = self.channel.clone();
            work.stopping = Some(tokio::spawn(async move {
                let mut errors = Vec::new();
                if let Some(starting) = starting {
                    match starting.await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => errors.push(format!("startup: {error:#}")),
                        Err(error) => errors.push(format!("startup task: {error}")),
                    }
                }
                let mut stopping = channel.stop();
                match std::panic::AssertUnwindSafe(stopping.as_mut())
                    .catch_unwind()
                    .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => errors.push(format!("stop: {error:#}")),
                    Err(_) => {
                        tracing::error!(channel_id = %channel.channel_id(),
                            "channel stop panicked; completion is unproven, retaining owner and dependencies");
                        // Never repoll a panicked future, claim a successful join,
                        // or release its channel owner. The runtime's grace budget
                        // reports Incomplete while retaining this cleanup task.
                        std::future::pending::<()>().await;
                    }
                }
                // Explicitly keep both captures live across the quarantine await.
                drop(stopping);
                drop(channel);
                anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
                Ok(())
            }));
        }
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.request_stop().await;
        let mut work = self.work.lock().await;
        if let Some(stopping) = work.stopping.as_mut() {
            let result = stopping
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result);
            work.stopping.take();
            work.stopped = Some(result.map_err(|error| format!("{error:#}")));
        }
        match work
            .stopped
            .as_ref()
            .expect("channel cleanup result retained")
        {
            Ok(()) => Ok(()),
            Err(error) => anyhow::bail!("{error}"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DisabledChannelInfo {
    pub id: String,
    pub channel_type: String,
    pub reason: String,
    pub disabled_at: DateTime<Utc>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            disabled: RwLock::new(HashMap::new()),
            admission: CancellationToken::new(),
            admission_gate: std::sync::Mutex::new(()),
            restart_gates: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Linearization point: no new registrations or startup may pass this gate.
    /// Existing outbound use remains available until the later shutdown phase.
    pub fn close_admission(&self) {
        let _gate = self
            .admission_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.admission.cancel();
    }

    pub fn is_closing(&self) -> bool {
        self.admission.is_cancelled()
    }

    /// Hold for an entire API disable/init/register/start/rollback operation.
    /// Internal methods do not reacquire it. Gates survive entry replacement so
    /// an older request cannot accidentally start or disable its successor.
    pub async fn restart_guard(
        &self,
        id: &str,
    ) -> anyhow::Result<tokio::sync::OwnedMutexGuard<()>> {
        let id = normalize_channel_identifier(id, "channel id")?;
        let gate = {
            let _admission = self
                .admission_gate
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            anyhow::ensure!(!self.is_closing(), "channel registry is stopping");
            self.restart_gates
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .entry(id)
                .or_default()
                .clone()
        };
        let guard = tokio::select! {
            biased;
            _ = self.admission.cancelled() => anyhow::bail!("channel registry is stopping"),
            guard = gate.lock_owned() => guard,
        };
        let _admission = self
            .admission_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        anyhow::ensure!(!self.is_closing(), "channel registry is stopping");
        Ok(guard)
    }

    pub async fn get(&self, id: &str) -> Option<Arc<dyn ChannelAdapter>> {
        self.channels
            .read()
            .await
            .get(id)
            .filter(|entry| !entry.retired.load(Ordering::Acquire))
            .map(|entry| entry.channel.clone())
    }

    pub async fn resolve_id_or_type(&self, id_or_type: &str) -> anyhow::Result<Option<String>> {
        let requested = id_or_type.trim();
        if requested.is_empty() {
            return Ok(None);
        }
        let channels = self.channels.read().await;
        if channels
            .get(requested)
            .is_some_and(|entry| !entry.retired.load(Ordering::Acquire))
        {
            return Ok(Some(requested.to_string()));
        }
        let matches = channels
            .values()
            .filter(|entry| {
                !entry.retired.load(Ordering::Acquire) && entry.channel.channel_type() == requested
            })
            .map(|entry| entry.channel.channel_id().to_string())
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(None),
            [id] => Ok(Some(id.clone())),
            _ => anyhow::bail!(
                "channel id '{}' matched multiple channels by type; use a concrete channel id",
                requested
            ),
        }
    }

    pub async fn get_resolved(
        &self,
        id_or_type: &str,
    ) -> anyhow::Result<Option<(String, Arc<dyn ChannelAdapter>)>> {
        let Some(id) = self.resolve_id_or_type(id_or_type).await? else {
            return Ok(None);
        };
        Ok(self.get(&id).await.map(|channel| (id, channel)))
    }

    pub async fn list(&self) -> Vec<Arc<dyn ChannelAdapter>> {
        self.channels
            .read()
            .await
            .values()
            .filter(|entry| !entry.retired.load(Ordering::Acquire))
            .map(|entry| entry.channel.clone())
            .collect()
    }

    pub async fn disabled(&self) -> Vec<DisabledChannelInfo> {
        self.disabled.read().await.values().cloned().collect()
    }

    pub async fn register(&self, channel: Arc<dyn ChannelAdapter>) -> anyhow::Result<()> {
        let id = normalize_channel_identifier(channel.channel_id(), "channel id")?;
        let channel_type = normalize_channel_identifier(channel.channel_type(), "channel type")?;
        {
            let mut map = self.channels.write().await;
            let _gate = self
                .admission_gate
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            anyhow::ensure!(!self.is_closing(), "channel registry is stopping");
            if let Some(previous) = map.get(&id) {
                let joined = previous.retired.load(Ordering::Acquire)
                    && previous
                        .work
                        .try_lock()
                        .is_ok_and(|work| matches!(work.stopped, Some(Ok(()))));
                anyhow::ensure!(
                    joined,
                    "channel '{}' still active or cleanup not confirmed",
                    id
                );
            }
            let active = map
                .iter()
                .filter(|(_, entry)| !entry.retired.load(Ordering::Acquire))
                .map(|(id, entry)| (id.clone(), entry.channel.clone()))
                .collect();
            validate_channel_identity_not_ambiguous(&active, &id, &channel_type)?;
            map.insert(
                id.clone(),
                Arc::new(ChannelEntry {
                    channel,
                    cancel_start: self.admission.child_token(),
                    retired: AtomicBool::new(false),
                    work: Mutex::new(ChannelWork::default()),
                }),
            );
        }
        self.disabled.write().await.remove(&id);
        Ok(())
    }

    /// Start only through the registry so close_admission also fences API races.
    pub async fn start(&self, id: &str) -> anyhow::Result<()> {
        anyhow::ensure!(!self.is_closing(), "channel registry is stopping");
        let entry = self
            .channels
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("channel '{id}' not found"))?;
        entry.start().await
    }

    pub async fn mark_disabled(&self, id: &str, channel_type: &str, reason: impl ToString) {
        self.disabled.write().await.insert(
            id.to_string(),
            DisabledChannelInfo {
                id: id.into(),
                channel_type: channel_type.into(),
                reason: reason.to_string(),
                disabled_at: Utc::now(),
            },
        );
    }

    pub async fn disable(&self, id: &str, reason: impl ToString) -> anyhow::Result<()> {
        let entry = self
            .channels
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("channel '{id}' not found"))?;
        entry.retire();
        self.mark_disabled(id, entry.channel.channel_type(), reason)
            .await;
        entry.stop().await
    }

    pub async fn unregister(&self, id: &str) -> anyhow::Result<()> {
        self.disable(id, "unregistered").await
    }

    pub async fn broadcast(&self, msg: &str) -> anyhow::Result<()> {
        anyhow::ensure!(!self.is_closing(), "channel registry is stopping");
        let mut errors = Vec::new();
        for channel in self.list().await {
            if let Err(error) = channel
                .send_message(
                    &ChannelTarget {
                        voice: None,
                        user_id: String::new(),
                    },
                    msg,
                )
                .await
            {
                errors.push(format!("{}: {error:#}", channel.channel_id()));
            }
        }
        anyhow::ensure!(errors.is_empty(), "broadcast failed: {}", errors.join("; "));
        Ok(())
    }

    /// Every stop is requested before awaiting results, so one stalled channel
    /// cannot prevent another channel's cancellation. Waiters never own handles.
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.close_admission();
        let channels = self
            .channels
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for entry in &channels {
            entry.retire();
        }
        futures::future::join_all(channels.iter().map(|entry| entry.request_stop())).await;
        let mut errors = Vec::new();
        for entry in channels {
            if let Err(error) = entry.stop().await {
                errors.push(format!("{}: {error:#}", entry.channel.channel_id()));
            }
        }
        anyhow::ensure!(
            errors.is_empty(),
            "channel shutdown incomplete: {}",
            errors.join("; ")
        );
        Ok(())
    }

    pub async fn health(&self) -> HashMap<String, ModuleHealth> {
        let mut results = HashMap::new();
        for channel in self.list().await {
            results.insert(
                channel.channel_id().to_string(),
                channel.health_check().await,
            );
        }
        results
    }
}

impl Default for ChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_channel_identifier(value: &str, label: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("{label} must not be empty");
    }
    Ok(value.to_string())
}

fn validate_channel_identity_not_ambiguous(
    channels: &HashMap<String, Arc<dyn ChannelAdapter>>,
    new_id: &str,
    new_type: &str,
) -> anyhow::Result<()> {
    for existing in channels.values() {
        let existing_id = existing.channel_id();
        let existing_type = existing.channel_type();
        if new_id == existing_type && new_type != existing_type {
            anyhow::bail!(
                "channel id '{}' conflicts with existing channel type '{}'; use a concrete instance id that is not another channel type",
                new_id,
                existing_type
            );
        }
        if new_type == existing_id && new_id != existing_id {
            anyhow::bail!(
                "channel type '{}' conflicts with existing channel id '{}'; use a channel id that does not shadow a channel type",
                new_type,
                existing_id
            );
        }
    }
    Ok(())
}
