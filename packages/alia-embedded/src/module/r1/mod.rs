//! Existing R1 firmware supplies wake events over UART; PCM still uses the audio slot.
mod initializer;
#[cfg(any(target_os = "linux", test))]
mod protocol;
#[cfg(target_os = "linux")]
mod serial;
#[cfg(test)]
mod tests;

pub use initializer::InitializationReport;
pub(crate) use initializer::PreparedInitialization;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
#[cfg(target_os = "linux")]
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use tokio_util::sync::CancellationToken;

use super::context::ModuleContext;
use super::lifecycle::{ModuleHealth, ModuleLifecycle};
#[cfg(any(target_os = "linux", test))]
use super::traits::WakeWordSource;
use super::traits::{WakeWordEngine, WakeWordEvent};
use crate::config::R1Config;

const EVENT_TTL: Duration = Duration::from_secs(1);
#[cfg(target_os = "linux")]
const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const PARTIAL_TIMEOUT: Duration = Duration::from_millis(250);

struct Pending {
    event: WakeWordEvent,
    received: Instant,
}

#[derive(Default)]
struct State {
    generation: u64,
    #[cfg(all(target_os = "linux", any(test, feature = "test-support")))]
    serial_generation: Option<u64>,
    version: Option<String>,
    pending: Option<Pending>,
    previous: Option<(u16, Vec<u8>, Instant)>,
    last_error: Option<String>,
}

impl State {
    fn reset(&mut self) -> Result<()> {
        self.pending = None;
        self.generation = self
            .generation
            .checked_add(1)
            .context("R1 reset generation exhausted")?;
        Ok(())
    }

    fn disconnect(&mut self, error: Option<String>) {
        #[cfg(all(target_os = "linux", any(test, feature = "test-support")))]
        {
            self.serial_generation = None;
        }
        self.version = None;
        self.pending = None;
        self.previous = None;
        self.last_error = error;
    }

    #[cfg(any(target_os = "linux", test))]
    fn receive(
        &mut self,
        generation: u64,
        frame: &protocol::Frame,
        now: Instant,
        timestamp_us: u64,
    ) {
        if self.generation != generation || self.version.is_none() {
            return;
        }
        let Some(wake) = frame.wakeup() else { return };
        if self
            .previous
            .as_ref()
            .is_some_and(|(id, payload, received)| {
                *id == frame.id
                    && payload == &frame.payload
                    && now.duration_since(*received) < EVENT_TTL
            })
        {
            return;
        }
        self.previous = Some((frame.id, frame.payload.clone(), now));
        tracing::debug!(
            message_id = frame.id,
            angle_deg = ?wake.angle_deg,
            beam = ?wake.beam,
            "R1 UART wake event accepted"
        );
        self.pending = Some(Pending {
            event: WakeWordEvent {
                keyword: wake.keyword,
                timestamp_us,
                source: WakeWordSource::R1 {
                    message_id: frame.id,
                    angle_deg: wake.angle_deg,
                    beam: wake.beam,
                },
            },
            received: now,
        });
    }

    fn detect(&mut self, now: Instant) -> Option<WakeWordEvent> {
        let pending = self.pending.take()?;
        let age = now.duration_since(pending.received);
        let fresh = self.version.is_some() && age < EVENT_TTL;
        if fresh {
            tracing::debug!(
                source = ?pending.event.source,
                age_ms = age.as_millis() as u64,
                "R1 UART wake event consumed by speech"
            );
        }
        fresh.then_some(pending.event)
    }
}

pub struct R1WakeWord {
    #[cfg(target_os = "linux")]
    config: R1Config,
    state: Arc<Mutex<State>>,
    cancel: CancellationToken,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl R1WakeWord {
    /// Test synchronization after the UART owner has discarded the previous generation.
    #[cfg(all(target_os = "linux", any(test, feature = "test-support")))]
    pub fn test_reset_applied(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.serial_generation == Some(state.generation)
    }

    pub fn new(config: &R1Config) -> Result<Self> {
        ensure!(
            cfg!(target_os = "linux"),
            "R1 UART wakeword currently requires Linux"
        );
        config.validate()?;
        Ok(Self {
            #[cfg(target_os = "linux")]
            config: config.clone(),
            state: Arc::new(Mutex::new(State::default())),
            cancel: CancellationToken::new(),
            worker: Mutex::new(None),
        })
    }

    fn stop_worker(&self) -> Result<()> {
        self.cancel.cancel();
        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let result = worker.take().map(|worker| worker.join());
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .disconnect(None);
        ensure!(!matches!(result, Some(Err(_))), "R1 UART worker panicked");
        Ok(())
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for R1WakeWord {
    fn name(&self) -> &'static str {
        "r1_uart"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> Result<()> {
        self.cancel = ctx.cancel_token.child_token();
        Ok(())
    }

    async fn start(&self) -> Result<()> {
        ensure!(!self.cancel.is_cancelled(), "R1 UART owner is stopped");
        let worker = self
            .worker
            .lock()
            .map_err(|_| anyhow::anyhow!("R1 worker lock poisoned"))?;
        ensure!(!self.cancel.is_cancelled(), "R1 UART owner is stopped");
        if let Some(worker) = worker.as_ref() {
            ensure!(!worker.is_finished(), "R1 UART worker exited");
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        {
            let mut worker = worker;
            let (config, state, cancel) =
                (self.config.clone(), self.state.clone(), self.cancel.clone());
            *worker = Some(
                thread::Builder::new()
                    .name("r1-uart".into())
                    .spawn(move || run(config, state, cancel))
                    .context("spawning R1 UART worker")?,
            );
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        anyhow::bail!("R1 UART wakeword currently requires Linux")
    }

    async fn stop(&self) -> Result<()> {
        self.stop_worker()
    }

    async fn health_check(&self) -> ModuleHealth {
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if worker.as_ref().is_some_and(thread::JoinHandle::is_finished) {
            return ModuleHealth::Unhealthy;
        }
        if !self.cancel.is_cancelled()
            && self
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .version
                .is_some()
        {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Degraded
        }
    }
}

impl WakeWordEngine for R1WakeWord {
    fn reset(&self) -> Result<()> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("R1 state lock poisoned"))?
            .reset()
    }

    fn detect(&self, pcm: &[i16]) -> Option<WakeWordEvent> {
        if pcm.is_empty() || self.cancel.is_cancelled() {
            return None;
        }
        self.state.lock().ok()?.detect(Instant::now())
    }
}

impl Drop for R1WakeWord {
    fn drop(&mut self) {
        if let Err(error) = self.stop_worker() {
            tracing::error!(%error, "R1 UART cleanup failed");
        }
    }
}

/// A simulated UART for cross-crate integration tests; never opens a hardware device.
#[cfg(all(any(test, feature = "test-support"), target_os = "linux"))]
pub fn test_pty_engine() -> (std::fs::File, R1WakeWord) {
    let (master, serial, _) = serial::pty();
    let owner = R1WakeWord::new(&R1Config {
        serial_path: "/test-r1-pty".into(),
    })
    .unwrap();
    let (state, cancel) = (owner.state.clone(), owner.cancel.clone());
    *owner.worker.lock().unwrap() = Some(thread::spawn(move || {
        let result = session(serial, &state, &cancel, 0x7ffe);
        state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .disconnect(result.err().map(|error| format!("{error:#}")));
    }));
    (master, owner)
}

#[cfg(target_os = "linux")]
fn run(config: R1Config, state: Arc<Mutex<State>>, cancel: CancellationToken) {
    while !cancel.is_cancelled() {
        let result = serial::Serial::open(&config.serial_path)
            .and_then(|serial| session(serial, &state, &cancel, rand::random()));
        let error = result
            .err()
            .filter(|_| !cancel.is_cancelled())
            .map(|error| format!("{error:#}"));
        {
            let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
            if error != state.last_error
                && let Some(error) = &error
            {
                tracing::warn!(path = %config.serial_path.display(), %error, "R1 UART unavailable; retrying");
            }
            state.disconnect(error);
        }
        let retry = Instant::now() + Duration::from_secs(1);
        while !cancel.is_cancelled() && Instant::now() < retry {
            thread::sleep(
                serial::POLL_INTERVAL.min(retry.saturating_duration_since(Instant::now())),
            );
        }
    }
    state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .disconnect(None);
}

#[cfg(target_os = "linux")]
fn session(
    mut serial: serial::Serial,
    state: &Mutex<State>,
    cancel: &CancellationToken,
    mut version_id: u16,
) -> Result<()> {
    let mut decoder = protocol::Decoder::default();
    let mut generation = state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .generation;
    #[cfg(all(target_os = "linux", any(test, feature = "test-support")))]
    {
        state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .serial_generation = Some(generation);
    }
    let mut partial_deadline = None;
    serial.write(&protocol::version_query(version_id), cancel)?;
    tracing::debug!(version_id, generation, "R1 UART version query sent");
    let version_deadline = Instant::now() + VERSION_TIMEOUT;
    let mut verified = false;
    let mut bytes = [0u8; 1024];
    while !cancel.is_cancelled() {
        let next_generation = state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .generation;
        if generation != next_generation {
            generation = next_generation;
            decoder.clear();
            partial_deadline = None;
            serial.clear_input()?;
            #[cfg(all(target_os = "linux", any(test, feature = "test-support")))]
            {
                state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .serial_generation = Some(generation);
            }
            if !verified {
                ensure!(
                    Instant::now() < version_deadline,
                    "R1 version response timed out"
                );
                // Reset discards the old transaction's response along with stale wakes.
                version_id = version_id.wrapping_add(1);
                serial.write(&protocol::version_query(version_id), cancel)?;
                tracing::debug!(
                    version_id,
                    generation,
                    "R1 UART version query replaced after capture reset"
                );
            }
        }
        if !verified {
            ensure!(
                Instant::now() < version_deadline,
                "R1 version response timed out"
            );
        }
        if partial_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            decoder.clear();
            partial_deadline = None;
        }
        let count = serial.read(&mut bytes)?;
        let received = Instant::now();
        let timestamp_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros()
            .min(u64::MAX as u128) as u64;
        if count == 0 {
            continue;
        }
        if generation
            != state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .generation
        {
            continue;
        }
        if partial_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            decoder.clear();
            partial_deadline = None;
        }
        let frames = decoder.push(&bytes[..count]);
        if decoder.is_empty() {
            partial_deadline = None;
        } else if partial_deadline.is_none() || !frames.is_empty() {
            partial_deadline = Some(Instant::now() + PARTIAL_TIMEOUT);
        }
        for frame in frames {
            if !verified {
                ensure!(
                    Instant::now() < version_deadline,
                    "R1 version response timed out"
                );
            }
            if let Some(ack) = frame.handshake_ack() {
                tracing::debug!(
                    message_id = frame.id,
                    verified,
                    "R1 UART handshake received"
                );
                if verified {
                    state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .disconnect(None);
                }
                serial.write(&ack, cancel)?;
                ensure!(
                    !verified,
                    "R1 restarted its UART protocol; version must be verified again"
                );
            }
            if !verified && let Some(version) = frame.version_response(version_id) {
                let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
                if state.generation == generation {
                    tracing::info!(firmware = %version, "R1 UART protocol ready");
                    state.version = Some(version);
                    state.last_error = None;
                    verified = true;
                }
            }
            state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .receive(generation, &frame, received, timestamp_us);
        }
    }
    Ok(())
}
