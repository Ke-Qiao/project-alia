//! Local playback ordering. Control never waits for media capacity or buffer drain.
//!
//! A stop advances the generation and retires queued and in-flight media. This is
//! a local barrier: callers must still reject late network frames from old turns.
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Result, bail};

#[derive(Debug)]
pub(super) struct AudioBlock {
    pcm: Arc<[i16]>,
    range: Range<usize>,
    pub sample_rate: u32,
    pub generation: u64,
}

impl AudioBlock {
    fn samples(&self) -> &[i16] {
        &self.pcm[self.range.clone()]
    }

    pub fn render(&self, sample_rate: u32, channels: usize) -> Vec<i16> {
        let mono = if self.sample_rate == sample_rate || self.pcm.len() < 2 {
            self.samples().to_vec()
        } else {
            // Use the original clip's coordinates across worker blocks. Rounding
            // each block separately loses samples and resets interpolation phase.
            let output_start =
                self.range.start as u64 * sample_rate as u64 / self.sample_rate as u64;
            let mut output_end =
                self.range.end as u64 * sample_rate as u64 / self.sample_rate as u64;
            if self.range.end == self.pcm.len() {
                output_end = output_end.max(1);
            }
            let step = self.sample_rate as f64 / sample_rate as f64;
            (output_start..output_end)
                .map(|index| {
                    let position = index as f64 * step;
                    let left = position.floor() as usize;
                    let right = (left + 1).min(self.pcm.len() - 1);
                    let fraction = position - left as f64;
                    (self.pcm[left] as f64
                        + (self.pcm[right] as f64 - self.pcm[left] as f64) * fraction)
                        .round() as i16
                })
                .collect()
        };
        super::interleave_mono(&mono, channels)
    }
}

struct State {
    media: VecDeque<AudioBlock>,
    stop_pending: bool,
    closed: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    generation: Arc<AtomicU64>,
    capacity: usize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn stop(&self, close: bool, expected: Option<u64>) -> Option<u64> {
        let (retired, generation) = {
            let mut state = self.lock();
            if state.closed
                || expected.is_some_and(|epoch| epoch != self.generation.load(Ordering::Acquire))
            {
                return None;
            }
            let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
            state.stop_pending = true;
            state.closed |= close;
            (std::mem::take(&mut state.media), generation)
        };
        self.changed.notify_all();
        // Deallocate retired audio outside the lock used by control and producers.
        drop(retired);
        Some(generation)
    }
}

struct Producer(Arc<Shared>);

impl Drop for Producer {
    fn drop(&mut self) {
        self.0.stop(true, None);
    }
}

#[derive(Clone)]
pub(super) struct PlaybackSender(Arc<Producer>);

pub(super) struct PlaybackReceiver(Arc<Shared>);

pub(super) enum PlaybackAction {
    Play(AudioBlock),
    Stop,
    Shutdown,
}

#[cfg(test)]
pub(super) fn channel(capacity: usize) -> (PlaybackSender, PlaybackReceiver) {
    channel_with_generation(capacity, Arc::new(AtomicU64::new(0)))
}

pub(super) fn channel_with_generation(
    capacity: usize,
    generation: Arc<AtomicU64>,
) -> (PlaybackSender, PlaybackReceiver) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            media: VecDeque::new(),
            stop_pending: false,
            closed: false,
        }),
        changed: Condvar::new(),
        generation,
        capacity: capacity.max(1),
    });
    (
        PlaybackSender(Arc::new(Producer(shared.clone()))),
        PlaybackReceiver(shared),
    )
}

impl PlaybackSender {
    pub fn play(&self, pcm: &[i16], sample_rate: u32) -> Result<()> {
        let generation = self.0.0.generation.load(Ordering::Acquire);
        self.play_if_current(pcm, sample_rate, generation)
            .map(|_| ())
    }

    pub fn play_if_current(&self, pcm: &[i16], sample_rate: u32, generation: u64) -> Result<bool> {
        if sample_rate == 0 {
            bail!("playback sample rate must be positive");
        }
        let shared = &self.0.0;
        let block = AudioBlock {
            pcm: pcm.into(),
            range: 0..pcm.len(),
            sample_rate,
            generation,
        };
        let mut state = shared.lock();
        while !state.closed
            && shared.generation.load(Ordering::Acquire) == generation
            && state.media.len() == shared.capacity
        {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        if state.closed {
            bail!("playback runtime is closed");
        }
        if shared.generation.load(Ordering::Acquire) != generation {
            return Ok(false);
        }
        state.media.push_back(block);
        drop(state);
        shared.changed.notify_all();
        Ok(true)
    }

    pub fn stop(&self) {
        self.0.0.stop(false, None);
    }

    pub fn begin(&self) -> Result<u64> {
        self.0
            .0
            .stop(false, None)
            .ok_or_else(|| anyhow::anyhow!("playback runtime is closed"))
    }

    pub fn stop_if_current(&self, generation: u64) -> bool {
        self.0.0.stop(false, Some(generation)).is_some()
    }

    pub fn shutdown(&self) {
        self.0.0.stop(true, None);
    }
}

impl PlaybackReceiver {
    pub fn generation(&self) -> Arc<AtomicU64> {
        self.0.generation.clone()
    }

    pub fn device_failed(&self) -> impl Fn() + Send + 'static {
        let shared = self.0.clone();
        move || {
            shared.stop(true, None);
        }
    }

    fn take(&self, accept_media: bool) -> Option<PlaybackAction> {
        let mut state = self.0.lock();
        if state.closed {
            return Some(PlaybackAction::Shutdown);
        }
        if state.stop_pending {
            state.stop_pending = false;
            return Some(PlaybackAction::Stop);
        }
        // Bound worker conversion work without making a whole WAV submission
        // wait for its playback duration. Control can interrupt between blocks.
        let mut retired = None;
        let block = if accept_media {
            state.media.front_mut().map(|queued| {
                let count = (queued.sample_rate as usize / 50).clamp(1, 4096);
                let end = (queued.range.start + count).min(queued.pcm.len());
                let block = AudioBlock {
                    pcm: queued.pcm.clone(),
                    range: queued.range.start..end,
                    sample_rate: queued.sample_rate,
                    generation: queued.generation,
                };
                queued.range.start = end;
                block
            })
        } else {
            None
        };
        if state
            .media
            .front()
            .is_some_and(|queued| queued.range.is_empty())
        {
            retired = state.media.pop_front();
        }
        drop(state);
        if retired.is_some() {
            self.0.changed.notify_all();
        }
        drop(retired);
        block.map(PlaybackAction::Play)
    }

    fn wait(&self, accept_media: bool, idle: bool) {
        let state = self.0.lock();
        let waiting = |state: &mut State| {
            !state.closed && !state.stop_pending && (!accept_media || state.media.is_empty())
        };
        if idle {
            drop(self.0.changed.wait_while(state, waiting));
        } else {
            // Device consumption has no portable stdlib wake handle. Control and
            // new media wake this wait immediately; only device progress polls.
            drop(
                self.0
                    .changed
                    .wait_timeout_while(state, Duration::from_millis(2), waiting),
            );
        }
    }
}

impl Drop for PlaybackReceiver {
    fn drop(&mut self) {
        // Release blocked producers on initialization, device, or worker failure.
        self.0.stop(true, None);
    }
}

/// The same control loop drives both callback audio and nonblocking ALSA writes.
pub(super) trait PlaybackSink {
    fn buffered_ms(&self) -> u64;
    fn enqueue(&mut self, block: AudioBlock) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn pump(&mut self) -> Result<()>;
}

pub(super) fn run(
    receiver: PlaybackReceiver,
    sink: &mut impl PlaybackSink,
    target_buffer_ms: u64,
) -> Result<()> {
    let result = (|| {
        loop {
            let buffered_ms = sink.buffered_ms();
            match receiver.take(buffered_ms < target_buffer_ms.max(1)) {
                Some(PlaybackAction::Shutdown) => return Ok(()),
                Some(PlaybackAction::Stop) => sink.stop()?,
                Some(PlaybackAction::Play(block)) => sink.enqueue(block)?,
                None => {
                    sink.pump()?;
                    let buffered_ms = sink.buffered_ms();
                    receiver.wait(buffered_ms < target_buffer_ms.max(1), buffered_ms == 0);
                }
            }
        }
    })();
    // Closing or a device error must retire output, not drain an old response.
    let stopped = sink.stop();
    result.and(stopped)
}

/// Rendered audio retains its generation until the device consumes it.
pub(super) struct PlaybackBuffer {
    pub samples: VecDeque<i16>,
    generation: u64,
    current: Arc<AtomicU64>,
}

impl PlaybackBuffer {
    pub fn new(current: Arc<AtomicU64>) -> Self {
        Self {
            samples: VecDeque::new(),
            generation: current.load(Ordering::Acquire),
            current,
        }
    }

    pub fn is_current(&self, generation: u64) -> bool {
        generation == self.current.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        if self.is_current(self.generation) {
            self.samples.len()
        } else {
            0
        }
    }

    pub fn append(&mut self, generation: u64, samples: Vec<i16>) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        if self.generation != generation {
            self.samples.clear();
            // Retain the block's generation even if a stop races with append.
            self.generation = generation;
        }
        self.samples.extend(samples);
        true
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.generation = self.current.load(Ordering::Acquire);
    }

    pub fn next_sample(&mut self) -> i16 {
        if !self.is_current(self.generation) {
            self.clear();
            return 0;
        }
        self.samples.pop_front().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests;
