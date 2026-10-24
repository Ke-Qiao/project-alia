use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::config::EmbeddedConfig;
use crate::module::sensor::SensorRegistry;
use crate::storage::EmbeddedStore;

mod audio_reference;
pub use audio_reference::{
    AudioReferenceBlock, AudioReferenceBus, AudioReferenceReader, AudioSampleBoundary,
    AudioStreamSnapshot, NativeSampleFormat, REFERENCE_BLOCK_SAMPLES, ReferenceContent,
};
pub(crate) use audio_reference::{AudioStreamClock, AudioStreamOwner, AudioStreamProbe};

/// Audio bus for zero-copy PCM distribution between modules.
///
/// Uses a broadcast channel to fan-out audio frames to multiple consumers
/// (wake word engine, edge STT, upstream upload).
#[derive(Clone)]
pub struct AudioBus {
    tx: broadcast::Sender<AudioFrame>,
    reference: AudioReferenceBus,
    capture_stopped: Arc<Mutex<Option<CancellationToken>>>,
}

/// A single audio frame on the bus.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub pcm: Vec<i16>,
    pub sample_rate: u32,
    /// Converted sample cursor only, not a wall clock or playback alignment.
    pub timestamp_us: u64,
    /// None is explicitly unproven (e.g. injected test audio).
    pub timing: Option<alia_core::audio_clock::CaptureTiming>,
}

impl AudioBus {
    pub fn new(capacity: usize) -> Self {
        Self::with_boot(capacity, uuid::Uuid::new_v4())
    }

    pub fn with_boot(capacity: usize, boot_id: uuid::Uuid) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            reference: AudioReferenceBus::new(boot_id),
            capture_stopped: Arc::new(Mutex::new(None)),
        }
    }

    pub fn reference(&self) -> AudioReferenceBus {
        self.reference.clone()
    }

    pub fn publish(&self, frame: AudioFrame) {
        // Ignore send errors (no subscribers is OK)
        let _ = self.tx.send(frame);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AudioFrame> {
        self.tx.subscribe()
    }

    pub(crate) fn begin_capture(&self) -> CancellationToken {
        let stopped = CancellationToken::new();
        *self
            .capture_stopped
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(stopped.clone());
        stopped
    }

    pub(crate) fn capture_stopped(&self) -> Option<CancellationToken> {
        self.capture_stopped
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Event bus for internal module-to-module signaling.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<ModuleEvent>,
}

/// Events that modules can emit for cross-module coordination.
#[derive(Debug, Clone)]
pub enum ModuleEvent {
    WakeWordDetected(super::traits::WakeWordEvent),
    SpeechStart,
    SpeechEnd,
    PlaybackStarted { source: String },
    PlaybackFinished { source: String },
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn publish(&self, event: ModuleEvent) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ModuleEvent> {
        self.tx.subscribe()
    }
}

/// Shared context passed to all modules during initialization.
///
/// Lighter-weight than the server-side equivalent—tailored for Pi 5 resource constraints.
pub struct ModuleContext {
    pub config: Arc<EmbeddedConfig>,
    pub state_store: EmbeddedStore,
    pub event_bus: EventBus,
    pub cancel_token: CancellationToken,
    pub audio_bus: AudioBus,
    /// Shared sensor provider registry. Concrete sensor modules register their
    /// provider handles here; server-facing gRPC reads route through it.
    pub sensors: Arc<SensorRegistry>,
}
