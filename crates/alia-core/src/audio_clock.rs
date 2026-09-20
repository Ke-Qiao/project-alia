//! Local audio evidence. Frame counts and backend clock estimates are not proof
//! of DAC presentation, acoustic capture alignment, or a shared sampling clock.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioDirection {
    Capture,
    Playback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioStreamId {
    pub boot_id: Uuid,
    /// A new identity for every native stream open, including device restart.
    pub stream_id: Uuid,
    pub direction: AudioDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    pub sample_rate_hz: u32,
    pub channels: u16,
}
impl AudioFormat {
    pub fn is_valid(self) -> bool {
        (8000..=384000).contains(&self.sample_rate_hz) && (1..=32).contains(&self.channels)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioTimeQuality {
    Unknown,
    /// Provider timestamps or a timestamp plus queued-frame delay. Neither is
    /// a measured correspondence between the capture and playback devices.
    BackendEstimate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioClockReading {
    /// Origin is local to this stream. Equal sample rates never identify clocks.
    pub clock_id: Uuid,
    pub callback_ns: Option<i64>,
    pub sample_ns: Option<i64>,
    /// ALSA's observed queued frames, kept separate from a submission timestamp.
    pub queued_frames: Option<i64>,
    pub quality: AudioTimeQuality,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTiming {
    pub stream: AudioStreamId,
    pub format: AudioFormat,
    /// Counts only frames actually observed/accepted at the declared boundary.
    /// Unknown frames lost at an xrun are not invented or added to this cursor.
    pub start_frame: u64,
    pub frames: u64,
    /// Changes on discontinuity; cursors across segments cannot prove continuity.
    pub continuity: u64,
    pub clock: AudioClockReading,
    /// Actual host observation from the embedded runtime's monotonic boot origin.
    /// This is an observation time, not the sample's capture/presentation time.
    pub observed_boot_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureTiming {
    /// Native input batch contributing to this converted batch. With resampling,
    /// it does not claim the first converted sample equals the first native one.
    pub native: AudioTiming,
    pub converted_format: AudioFormat,
    pub converted_start_frame: u64,
    pub converted_frames: u64,
    pub resampled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioDiscontinuity {
    StreamStarted,
    StreamStopped,
    Cancelled,
    BackendError,
    Underrun,
    Overrun,
    Suspended,
    ClockRegression,
    CaptureConversionReset,
    CallbackBufferUnavailable,
}
