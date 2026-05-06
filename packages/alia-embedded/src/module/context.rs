use std::sync::Arc;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::config::EmbeddedConfig;

/// Audio bus for zero-copy PCM distribution between modules.
///
/// Uses a broadcast channel to fan-out audio frames to multiple consumers
/// (wake word engine, edge STT, upstream upload).
#[derive(Clone)]
pub struct AudioBus {
    tx: broadcast::Sender<AudioFrame>,
}

/// A single audio frame on the bus.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub pcm: Vec<i16>,
    pub sample_rate: u32,
    pub timestamp_us: u64,
}

impl AudioBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn publish(&self, frame: AudioFrame) {
        // Ignore send errors (no subscribers is OK)
        let _ = self.tx.send(frame);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AudioFrame> {
        self.tx.subscribe()
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
    WakeWordDetected { keyword: String, timestamp_us: u64 },
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
    pub event_bus: EventBus,
    pub cancel_token: CancellationToken,
    pub audio_bus: AudioBus,
}
