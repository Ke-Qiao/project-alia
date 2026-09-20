//! Bounded diagnostic/reference handoff. Producers never wait for consumers.
//! This is a single-consumer stream: a future AEC worker owns that consumer and
//! must reset on loss or discontinuity, rather than treating missing PCM as zero.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alia_core::audio_clock::*;
use alia_core::delivery::SubmissionBoundary;
use uuid::Uuid;

pub const REFERENCE_BLOCK_SAMPLES: usize = 1024;
const REFERENCE_BLOCK_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceContent {
    /// Includes real zero-valued media and software mute after volume processing.
    Media,
    /// CPAL actually received a zero-filled callback buffer at this boundary.
    ZeroFill,
    Mixed,
}

#[derive(Clone, Debug)]
pub struct AudioReferenceBlock {
    pub timing: AudioTiming,
    pub boundary: Option<SubmissionBoundary>,
    pub content: ReferenceContent,
    pub discontinuity: Option<AudioDiscontinuity>,
    pub sequence: u64,
    pub lost_blocks_before: u64,
    pub lost_frames_before: u64,
    /// Normalized samples copied from the actual native output representation.
    /// Storage is fixed and frame-aligned; only samples[..sample_count] is valid.
    pub samples: [f64; REFERENCE_BLOCK_SAMPLES],
    pub sample_count: usize,
}

#[derive(Clone, Debug)]
pub struct AudioStreamSnapshot {
    pub stream: AudioStreamId,
    pub format: AudioFormat,
    pub running: bool,
    pub closing: bool,
    pub closed: bool,
    pub cursor: u64,
    pub continuity: u64,
    pub last_discontinuity: Option<AudioDiscontinuity>,
    pub last_fault: Option<AudioDiscontinuity>,
    pub attempted_blocks: u64,
    pub lost_blocks: u64,
    pub lost_frames: u64,
}

struct StreamState {
    stream: AudioStreamId,
    format: AudioFormat,
    // 0: preparing, 1: active, 2: closing, 3: final statistics published.
    phase: AtomicU8,
    cursor: AtomicU64,
    continuity: AtomicU64,
    reason: AtomicU8,
    fault: AtomicU8,
    sequence: AtomicU64,
    lost_blocks: AtomicU64,
    lost_frames: AtomicU64,
}

struct ReferenceInner {
    boot_id: Uuid,
    boot_origin: Instant,
    queue: Mutex<VecDeque<AudioReferenceBlock>>,
    capacity: usize,
    streams: Mutex<[Option<Arc<StreamState>>; 2]>,
}

#[derive(Clone)]
pub struct AudioReferenceBus(Arc<ReferenceInner>);
impl AudioReferenceBus {
    pub(crate) fn new(boot_id: Uuid) -> Self {
        Self::with_capacity(boot_id, REFERENCE_BLOCK_CAPACITY)
    }
    fn with_capacity(boot_id: Uuid, capacity: usize) -> Self {
        assert!(!boot_id.is_nil() && capacity > 0);
        Self(Arc::new(ReferenceInner {
            boot_id,
            boot_origin: Instant::now(),
            queue: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            streams: Mutex::new([None, None]),
        }))
    }
    /// Consumer side only. Never call from an audio callback.
    pub fn try_read(&self) -> Option<AudioReferenceBlock> {
        self.0
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }
    /// Independent of the PCM queue, so final overflow and a dropped close marker
    /// remain observable even when no later callback or packet exists.
    pub fn snapshot(&self, direction: AudioDirection) -> Option<AudioStreamSnapshot> {
        self.0.streams.lock().unwrap_or_else(|e| e.into_inner())[index(direction)]
            .as_ref()
            .map(|s| snapshot(s))
    }
    pub(crate) fn open_stream(
        &self,
        direction: AudioDirection,
        format: AudioFormat,
    ) -> AudioStreamClock {
        assert!(format.is_valid());
        let state = Arc::new(StreamState {
            stream: AudioStreamId {
                boot_id: self.0.boot_id,
                stream_id: Uuid::new_v4(),
                direction,
            },
            format,
            phase: 0.into(),
            cursor: 0.into(),
            continuity: 0.into(),
            reason: 255.into(),
            fault: 255.into(),
            sequence: 0.into(),
            lost_blocks: 0.into(),
            lost_frames: 0.into(),
        });
        self.0.streams.lock().unwrap_or_else(|e| e.into_inner())[index(direction)] =
            Some(state.clone());
        let probe = AudioStreamProbe {
            bus: self.clone(),
            state,
        };
        AudioStreamClock {
            probe,
            last_callback_ns: None,
        }
    }
}
fn index(direction: AudioDirection) -> usize {
    match direction {
        AudioDirection::Capture => 0,
        AudioDirection::Playback => 1,
    }
}
fn snapshot(s: &StreamState) -> AudioStreamSnapshot {
    let phase = s.phase.load(Ordering::Acquire);
    AudioStreamSnapshot {
        stream: s.stream,
        format: s.format,
        running: phase == 1,
        closing: phase == 2,
        closed: phase == 3,
        cursor: s.cursor.load(Ordering::Acquire),
        continuity: s.continuity.load(Ordering::Acquire),
        last_discontinuity: match s.reason.load(Ordering::Acquire) {
            255 => None,
            reason => Some(decode_reason(reason)),
        },
        last_fault: match s.fault.load(Ordering::Acquire) {
            255 => None,
            reason => Some(decode_reason(reason)),
        },
        attempted_blocks: s.sequence.load(Ordering::Acquire),
        lost_blocks: s.lost_blocks.load(Ordering::Acquire),
        lost_frames: s.lost_frames.load(Ordering::Acquire),
    }
}

#[derive(Clone)]
pub(crate) struct AudioStreamProbe {
    bus: AudioReferenceBus,
    state: Arc<StreamState>,
}
impl AudioStreamProbe {
    pub fn activate(&self) {
        if self
            .state
            .phase
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.mark(AudioDiscontinuity::StreamStarted);
        }
    }
    pub fn stream_id(&self) -> Uuid {
        self.state.stream.stream_id
    }
    pub fn format(&self) -> AudioFormat {
        self.state.format
    }
    pub fn continuity(&self) -> u64 {
        self.state.continuity.load(Ordering::Acquire)
    }
    pub fn unknown_clock(&self) -> AudioClockReading {
        AudioClockReading {
            clock_id: self.stream_id(),
            callback_ns: None,
            sample_ns: None,
            queued_frames: None,
            quality: AudioTimeQuality::Unknown,
        }
    }
    pub fn observed_boot_ns(&self) -> u64 {
        self.bus
            .0
            .boot_origin
            .elapsed()
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64
    }
    pub fn mark(&self, reason: AudioDiscontinuity) {
        if !matches!(
            reason,
            AudioDiscontinuity::StreamStarted
                | AudioDiscontinuity::StreamStopped
                | AudioDiscontinuity::Cancelled
        ) {
            self.state
                .fault
                .store(encode_reason(reason), Ordering::Release);
        }
        self.state
            .reason
            .store(encode_reason(reason), Ordering::Release);
        self.state.continuity.fetch_add(1, Ordering::AcqRel);
        let timing = AudioTiming {
            stream: self.state.stream,
            format: self.state.format,
            start_frame: self.state.cursor.load(Ordering::Acquire),
            frames: 0,
            continuity: self.continuity(),
            clock: self.unknown_clock(),
            observed_boot_ns: self.observed_boot_ns(),
        };
        self.publish(AudioReferenceBlock {
            timing,
            boundary: None,
            content: ReferenceContent::ZeroFill,
            discontinuity: Some(reason),
            sequence: 0,
            lost_blocks_before: 0,
            lost_frames_before: 0,
            samples: [0.0; REFERENCE_BLOCK_SAMPLES],
            sample_count: 0,
        });
    }
    pub fn close(&self) {
        if self
            .state
            .phase
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |phase| {
                (phase < 2).then_some(2)
            })
            .is_ok()
        {
            self.mark(AudioDiscontinuity::StreamStopped);
            // Readers which acquire Closed must also observe the final marker
            // attempt and any overflow it caused, even without later PCM.
            self.state.phase.store(3, Ordering::Release);
        }
    }
    pub fn publish(&self, mut block: AudioReferenceBlock) {
        block.sequence = self.state.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        block.lost_blocks_before = self.state.lost_blocks.load(Ordering::Acquire);
        block.lost_frames_before = self.state.lost_frames.load(Ordering::Acquire);
        if let Ok(mut queue) = self.bus.0.queue.try_lock() {
            if queue.len() < self.bus.0.capacity {
                queue.push_back(block);
                return;
            }
        }
        self.state.lost_blocks.fetch_add(1, Ordering::AcqRel);
        self.state
            .lost_frames
            .fetch_add(block.timing.frames, Ordering::AcqRel);
    }
}

pub(crate) struct AudioStreamOwner(pub AudioStreamProbe);
impl Drop for AudioStreamOwner {
    fn drop(&mut self) {
        self.0.close();
    }
}

pub(crate) struct AudioStreamClock {
    pub probe: AudioStreamProbe,
    last_callback_ns: Option<i64>,
}
impl AudioStreamClock {
    pub fn observe(&mut self, frames: u64, mut clock: AudioClockReading) -> AudioTiming {
        self.probe.activate();
        if clock.clock_id != self.probe.stream_id()
            || clock
                .callback_ns
                .zip(self.last_callback_ns)
                .is_some_and(|(new, old)| new < old)
        {
            self.probe.mark(AudioDiscontinuity::ClockRegression);
            self.last_callback_ns = clock.callback_ns;
            clock = self.probe.unknown_clock();
        } else {
            self.last_callback_ns = clock.callback_ns.or(self.last_callback_ns);
        }
        let start = self.probe.state.cursor.fetch_add(frames, Ordering::AcqRel);
        AudioTiming {
            stream: self.probe.state.stream,
            format: self.probe.state.format,
            start_frame: start,
            frames,
            continuity: self.probe.continuity(),
            clock,
            observed_boot_ns: self.probe.observed_boot_ns(),
        }
    }
}

fn encode_reason(reason: AudioDiscontinuity) -> u8 {
    match reason {
        AudioDiscontinuity::StreamStarted => 0,
        AudioDiscontinuity::StreamStopped => 1,
        AudioDiscontinuity::Cancelled => 2,
        AudioDiscontinuity::BackendError => 3,
        AudioDiscontinuity::Underrun => 4,
        AudioDiscontinuity::Suspended => 5,
        AudioDiscontinuity::ClockRegression => 6,
        AudioDiscontinuity::CaptureConversionReset => 7,
        AudioDiscontinuity::CallbackBufferUnavailable => 8,
        AudioDiscontinuity::Overrun => 9,
    }
}
fn decode_reason(reason: u8) -> AudioDiscontinuity {
    match reason {
        1 => AudioDiscontinuity::StreamStopped,
        2 => AudioDiscontinuity::Cancelled,
        3 => AudioDiscontinuity::BackendError,
        4 => AudioDiscontinuity::Underrun,
        5 => AudioDiscontinuity::Suspended,
        6 => AudioDiscontinuity::ClockRegression,
        7 => AudioDiscontinuity::CaptureConversionReset,
        8 => AudioDiscontinuity::CallbackBufferUnavailable,
        9 => AudioDiscontinuity::Overrun,
        _ => AudioDiscontinuity::StreamStarted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format() -> AudioFormat {
        AudioFormat {
            sample_rate_hz: 48000,
            channels: 2,
        }
    }
    fn block(clock: &mut AudioStreamClock, frames: u64) -> AudioReferenceBlock {
        let timing = clock.observe(frames, clock.probe.unknown_clock());
        AudioReferenceBlock {
            timing,
            boundary: Some(SubmissionBoundary::CpalCallbackBuffer),
            content: ReferenceContent::Media,
            discontinuity: None,
            sequence: 0,
            lost_blocks_before: 0,
            lost_frames_before: 0,
            samples: [0.25; REFERENCE_BLOCK_SAMPLES],
            sample_count: frames as usize * 2,
        }
    }
    #[test]
    fn full_reference_queue_and_lost_close_remain_visible_without_later_pcm() {
        let bus = AudioReferenceBus::with_capacity(Uuid::new_v4(), 1);
        let mut clock = bus.open_stream(AudioDirection::Playback, format());
        clock.probe.activate();
        bus.try_read().unwrap(); // Started
        let first = block(&mut clock, 4);
        clock.probe.publish(first);
        let lost = block(&mut clock, 3);
        clock.probe.publish(lost);
        clock.probe.mark(AudioDiscontinuity::BackendError);
        clock.probe.mark(AudioDiscontinuity::Cancelled); // Cleanup must retain the actual fault.
        clock.probe.close();
        let state = bus.snapshot(AudioDirection::Playback).unwrap();
        assert!(!state.running);
        assert_eq!(state.cursor, 7);
        assert_eq!(state.lost_blocks, 4); // PCM + error + cleanup cancellation + close
        assert_eq!(state.lost_frames, 3);
        assert_eq!(state.last_fault, Some(AudioDiscontinuity::BackendError));
        assert_eq!(
            state.last_discontinuity,
            Some(AudioDiscontinuity::StreamStopped)
        );
        assert_eq!(bus.try_read().unwrap().timing.frames, 4);
        assert!(bus.try_read().is_none());
    }
    #[test]
    fn resumed_reference_reports_the_missing_cursor_range_without_inventing_silence() {
        let bus = AudioReferenceBus::with_capacity(Uuid::new_v4(), 2);
        let mut clock = bus.open_stream(AudioDirection::Playback, format());
        clock.probe.activate();
        bus.try_read();
        for frames in [10, 5, 7] {
            let next = block(&mut clock, frames);
            clock.probe.publish(next);
        }
        assert_eq!(bus.try_read().unwrap().timing.frames, 10);
        let next = block(&mut clock, 2);
        clock.probe.publish(next);
        let before_gap = bus.try_read().unwrap();
        let after_gap = bus.try_read().unwrap();
        assert_eq!(before_gap.timing.start_frame + before_gap.timing.frames, 15);
        assert_eq!(after_gap.timing.start_frame, 22);
        assert_eq!(after_gap.sequence, before_gap.sequence + 2);
        assert_eq!(after_gap.lost_blocks_before, 1);
        assert_eq!(after_gap.lost_frames_before, 7);
        assert_eq!(after_gap.timing.frames, 2);
    }
    #[test]
    fn contended_reference_consumer_never_blocks_native_producer_or_close() {
        let bus = AudioReferenceBus::with_capacity(Uuid::new_v4(), 2);
        let mut clock = bus.open_stream(AudioDirection::Playback, format());
        clock.probe.activate();
        bus.try_read();
        let lock = bus.0.queue.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let lost = block(&mut clock, 2);
            clock.probe.publish(lost);
            clock.probe.close();
            tx.send(()).unwrap();
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(1));
        drop(lock);
        worker.join().unwrap();
        result.expect("producer waited for reference queue lock");
        let state = bus.snapshot(AudioDirection::Playback).unwrap();
        assert_eq!(state.lost_frames, 2);
        assert_eq!(state.lost_blocks, 2);
        assert!(!state.running);
    }
    #[test]
    fn capture_playback_and_restart_have_distinct_clock_and_sample_domains() {
        let boot = Uuid::new_v4();
        let bus = AudioReferenceBus::new(boot);
        let mut capture = bus.open_stream(AudioDirection::Capture, format());
        let mut playback = bus.open_stream(AudioDirection::Playback, format());
        let a = capture.observe(24, capture.probe.unknown_clock());
        let b = playback.observe(24, playback.probe.unknown_clock());
        assert_eq!(a.stream.boot_id, boot);
        assert_eq!(b.stream.boot_id, boot);
        assert_ne!(a.stream.stream_id, b.stream.stream_id);
        assert_ne!(a.clock.clock_id, b.clock.clock_id);
        assert_eq!((a.start_frame, b.start_frame), (0, 0));
        let owner = AudioStreamOwner(playback.probe.clone());
        drop(owner);
        assert!(!bus.snapshot(AudioDirection::Playback).unwrap().running);
        let mut fresh = bus.open_stream(AudioDirection::Playback, format());
        let next = fresh.observe(1, fresh.probe.unknown_clock());
        assert_ne!(next.stream.stream_id, b.stream.stream_id);
        assert_eq!(next.start_frame, 0);
        assert_eq!(next.clock.quality, AudioTimeQuality::Unknown);
    }
    #[test]
    fn failed_native_start_closes_the_owner_without_a_started_claim_or_reactivation() {
        let bus = AudioReferenceBus::new(Uuid::new_v4());
        let clock = bus.open_stream(AudioDirection::Playback, format());
        assert!(!bus.snapshot(AudioDirection::Playback).unwrap().running);
        assert_eq!(
            bus.snapshot(AudioDirection::Playback)
                .unwrap()
                .last_discontinuity,
            None
        );
        assert!(bus.try_read().is_none());
        drop(AudioStreamOwner(clock.probe.clone()));
        clock.probe.activate();
        let state = bus.snapshot(AudioDirection::Playback).unwrap();
        assert!(!state.running && state.closed);
        assert_eq!(state.cursor, 0);
        assert_eq!(
            bus.try_read().unwrap().discontinuity,
            Some(AudioDiscontinuity::StreamStopped)
        );
        assert!(bus.try_read().is_none());
    }
    #[test]
    fn final_close_snapshot_waits_for_native_owner_and_includes_lost_terminal_marker() {
        struct NativeOwner {
            entered: std::sync::mpsc::Sender<()>,
            release: std::sync::mpsc::Receiver<()>,
        }
        impl Drop for NativeOwner {
            fn drop(&mut self) {
                self.entered.send(()).unwrap();
                self.release.recv().unwrap();
            }
        }
        // Same field destruction order as CpalCaptureRuntime/CpalPlaybackSink.
        struct RuntimeOwner {
            _native: NativeOwner,
            _clock: AudioStreamOwner,
        }
        let bus = AudioReferenceBus::with_capacity(Uuid::new_v4(), 1);
        let clock = bus.open_stream(AudioDirection::Playback, format());
        clock.probe.activate(); // Fills the bounded queue.
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let owner = RuntimeOwner {
            _native: NativeOwner {
                entered: entered_tx,
                release: release_rx,
            },
            _clock: AudioStreamOwner(clock.probe.clone()),
        };
        let worker = std::thread::spawn(move || drop(owner));
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let during_native_drop = bus.snapshot(AudioDirection::Playback).unwrap();
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(during_native_drop.running && !during_native_drop.closed);
        let done = bus.snapshot(AudioDirection::Playback).unwrap();
        assert!(done.closed && !done.closing && !done.running);
        assert_eq!(done.lost_blocks, 1);
        assert_eq!(
            done.last_discontinuity,
            Some(AudioDiscontinuity::StreamStopped)
        );
    }
    #[test]
    fn backwards_backend_time_changes_continuity_without_rewinding_frames() {
        let bus = AudioReferenceBus::new(Uuid::new_v4());
        let mut clock = bus.open_stream(AudioDirection::Capture, format());
        let reading = |ns| AudioClockReading {
            clock_id: clock.probe.stream_id(),
            callback_ns: Some(ns),
            sample_ns: Some(ns - 10),
            queued_frames: None,
            quality: AudioTimeQuality::BackendEstimate,
        };
        let (first_read, old_read, next_read) = (reading(100), reading(50), reading(60));
        let first = clock.observe(20, first_read);
        let old = clock.observe(10, old_read);
        assert!(old.continuity > first.continuity);
        assert_eq!(old.start_frame, 20);
        assert_eq!(old.clock.quality, AudioTimeQuality::Unknown);
        let next = clock.observe(5, next_read);
        assert_eq!(next.continuity, old.continuity);
        assert_eq!(next.start_frame, 30);
        assert_eq!(next.clock.sample_ns, Some(50));
    }
}
