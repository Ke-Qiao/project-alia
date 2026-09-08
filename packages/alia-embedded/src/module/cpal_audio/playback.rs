//! Local playback ordering. Control never waits for media capacity or buffer drain.
//!
//! A stop advances the generation and retires queued and in-flight media. This is
//! a local barrier: callers must still reject late network frames from old turns.
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::Duration;

use crate::module::playback_session::{AudioSubmission, SubmissionWrite};
use alia_core::delivery::{ObservationEnd, PlaybackIdentity, SubmissionBackend};
use anyhow::{Context, Result, bail, ensure};

mod resampler;

#[derive(Debug)]
pub(super) struct AudioBlock {
    pcm: Arc<[i16]>,
    range: Range<usize>,
    pub sample_rate: u32,
    pub generation: u64,
    submission: Option<Arc<AudioSubmission>>,
    end: bool,
}

impl AudioBlock {
    fn samples(&self) -> &[i16] {
        &self.pcm[self.range.clone()]
    }

    fn is_live(&self) -> bool {
        self.submission.as_ref().is_none_or(|s| s.is_live())
    }
}

#[derive(PartialEq, Eq)]
enum RenderKey {
    Submission(PlaybackIdentity),
    Legacy(usize),
}

#[derive(Default)]
pub(super) struct Renderer {
    current: Option<(RenderKey, u64, resampler::PcmResampler)>,
}

impl Renderer {
    pub fn clear(&mut self) {
        self.current = None;
    }

    pub fn render(
        &mut self,
        block: AudioBlock,
        rate: u32,
        channels: usize,
    ) -> Result<RenderedBlock> {
        let key = block.submission.as_ref().map_or_else(
            || RenderKey::Legacy(block.pcm.as_ptr() as usize),
            |s| RenderKey::Submission(s.identity),
        );
        if !self
            .current
            .as_ref()
            .is_some_and(|(old, generation, _)| *old == key && *generation == block.generation)
        {
            self.current = Some((
                key,
                block.generation,
                resampler::PcmResampler::new(block.sample_rate, rate)?,
            ));
        }
        let converter = &mut self.current.as_mut().expect("initialized").2;
        let input: Vec<f64> = block
            .samples()
            .iter()
            .map(|&sample| f64::from(sample))
            .collect();
        let mut mono = converter.push(&input)?;
        if block.end {
            mono.extend(converter.finish()?);
            self.current = None;
        }
        let mono: Vec<i16> = mono
            .into_iter()
            .map(|sample| sample.round().clamp(-32768.0, 32767.0) as i16)
            .collect();
        Ok(RenderedBlock {
            samples: super::interleave_mono(&mono, channels),
            cursor: 0,
            channels,
            generation: block.generation,
            submission: block.submission,
            output_write: None,
        })
    }
}

struct State {
    media: VecDeque<AudioBlock>,
    stop_pending: bool,
    closed: bool,
    input: Option<PlaybackIdentity>,
    submissions: HashMap<PlaybackIdentity, Weak<AudioSubmission>>,
    backend: Option<SubmissionBackend>,
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
        self.retire(
            close,
            expected,
            ObservationEnd::Cancelled {
                reason: "local software playback fence applied".into(),
            },
        )
    }

    fn retire(&self, close: bool, expected: Option<u64>, outcome: ObservationEnd) -> Option<u64> {
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
            state.input = None;
            for submission in state.submissions.values().filter_map(Weak::upgrade) {
                submission.terminate(outcome.clone());
            }
            state.submissions.clear();
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
            input: None,
            submissions: HashMap::new(),
            backend: None,
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
            submission: None,
            end: true,
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

    pub fn open_submission(
        &self,
        identity: PlaybackIdentity,
        rate: u32,
        generation: u64,
    ) -> Result<Arc<AudioSubmission>> {
        let shared = &self.0.0;
        let mut state = shared.lock();
        ensure!(
            !state.closed && shared.generation.load(Ordering::Acquire) == generation,
            "playback generation is closed"
        );
        ensure!(
            state.input.is_none(),
            "previous playback input is still open"
        );
        state
            .submissions
            .retain(|_, s| s.upgrade().is_some_and(|s| s.is_live()));
        ensure!(
            state.submissions.len() < 32,
            "active playback observation capacity reached"
        );
        ensure!(
            !state.submissions.contains_key(&identity),
            "playback identity already registered"
        );
        let backend = state
            .backend
            .clone()
            .context("playback backend is not ready")?;
        let submission = AudioSubmission::new(identity, generation, rate, backend)?;
        state
            .submissions
            .insert(identity, Arc::downgrade(&submission));
        state.input = Some(identity);
        Ok(submission)
    }

    pub fn submit(
        &self,
        submission: &Arc<AudioSubmission>,
        offset: u64,
        pcm: &[i16],
        end: bool,
    ) -> Result<bool> {
        let shared = &self.0.0;
        let block = AudioBlock {
            pcm: pcm.into(),
            range: 0..pcm.len(),
            sample_rate: submission.source_sample_rate,
            generation: submission.generation,
            submission: Some(submission.clone()),
            end,
        };
        let mut state = shared.lock();
        while !state.closed
            && submission.is_live()
            && shared.generation.load(Ordering::Acquire) == submission.generation
            && state.media.len() == shared.capacity
        {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
        if state.closed
            || !submission.is_live()
            || shared.generation.load(Ordering::Acquire) != submission.generation
        {
            return Ok(false);
        }
        ensure!(
            state.input == Some(submission.identity),
            "playback input lease is stale"
        );
        if end {
            submission.accept_end(offset)?;
            state.input = None;
        } else {
            submission.accept(offset, pcm.len() as u64)?;
        }
        state.media.push_back(block);
        drop(state);
        shared.changed.notify_all();
        Ok(true)
    }

    pub fn cancel_submission(&self, submission: &Arc<AudioSubmission>, reason: &str) -> bool {
        let shared = &self.0.0;
        let mut state = shared.lock();
        if state.closed || shared.generation.load(Ordering::Acquire) != submission.generation {
            return false;
        }
        let Some(stored) = state
            .submissions
            .get(&submission.identity)
            .and_then(Weak::upgrade)
        else {
            return false;
        };
        if !Arc::ptr_eq(&stored, submission) {
            return false;
        }
        submission.terminate(ObservationEnd::Cancelled {
            reason: reason.into(),
        });
        state.media.retain(|block| {
            block
                .submission
                .as_ref()
                .is_none_or(|s| !Arc::ptr_eq(s, submission))
        });
        if state.input == Some(submission.identity) {
            state.input = None;
        }
        drop(state);
        shared.changed.notify_all();
        true
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
    pub fn set_backend(&self, backend: SubmissionBackend) {
        self.0.lock().backend = Some(backend);
    }
    pub fn generation(&self) -> Arc<AtomicU64> {
        self.0.generation.clone()
    }

    pub fn device_failed(&self) -> impl Fn() + Send + 'static {
        let shared = self.0.clone();
        move || {
            shared.retire(
                true,
                None,
                ObservationEnd::Unknown {
                    reason: "audio backend failed; presentation is unknown".into(),
                },
            );
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
        while state.media.front().is_some_and(|block| !block.is_live()) {
            state.media.pop_front();
        }
        let block = if accept_media {
            state.media.front_mut().map(|queued| {
                let count = (queued.sample_rate as usize / 50).clamp(1, 4096);
                let end = (queued.range.start + count).min(queued.pcm.len());
                let block = AudioBlock {
                    pcm: queued.pcm.clone(),
                    range: queued.range.start..end,
                    sample_rate: queued.sample_rate,
                    generation: queued.generation,
                    submission: queued.submission.clone(),
                    end: queued.end && end == queued.pcm.len(),
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
        self.0.retire(
            true,
            None,
            ObservationEnd::Unknown {
                reason: "audio worker closed; final device state is unknown".into(),
            },
        );
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

/// Every rendered segment retains its source identity until the actual backend
/// accepts it. Fixed output format makes same-epoch segments safe to append FIFO.
pub(super) struct RenderedBlock {
    pub samples: Vec<i16>,
    cursor: usize,
    channels: usize,
    generation: u64,
    submission: Option<Arc<AudioSubmission>>,
    output_write: Option<SubmissionWrite>,
}

pub(super) struct PlaybackBuffer {
    blocks: VecDeque<RenderedBlock>,
    // The callback moves consumed allocations here. The worker frees them; no
    // PCM buffer allocation/deallocation is required on the realtime callback.
    retired: VecDeque<RenderedBlock>,
    current: Arc<AtomicU64>,
}

const MAX_RENDERED_BLOCKS: usize = 512;

impl PlaybackBuffer {
    pub fn new(current: Arc<AtomicU64>) -> Self {
        Self {
            blocks: VecDeque::with_capacity(MAX_RENDERED_BLOCKS),
            retired: VecDeque::with_capacity(MAX_RENDERED_BLOCKS),
            current,
        }
    }

    pub fn is_current(&self, generation: u64) -> bool {
        generation == self.current.load(Ordering::Acquire)
    }

    fn valid(&self, block: &RenderedBlock) -> bool {
        self.is_current(block.generation) && block.submission.as_ref().is_none_or(|s| s.is_live())
    }

    pub fn len(&self) -> usize {
        self.blocks
            .iter()
            .filter(|b| self.valid(b))
            .map(|b| b.samples.len() - b.cursor)
            .sum()
    }

    pub fn append_rendered(&mut self, block: RenderedBlock) -> Result<bool> {
        self.discard_invalid();
        self.retired.clear(); // Worker thread only.
        if !self.valid(&block) {
            return Ok(false);
        }
        if block.samples.is_empty() {
            return Ok(true);
        }
        ensure!(
            block.channels > 0 && block.samples.len() % block.channels == 0,
            "incomplete rendered frame"
        );
        ensure!(
            self.blocks.len() < MAX_RENDERED_BLOCKS,
            "rendered playback queue capacity exceeded"
        );
        self.blocks.push_back(block);
        Ok(true)
    }

    #[cfg(test)]
    pub fn append(&mut self, generation: u64, samples: Vec<i16>) -> bool {
        self.append_rendered(RenderedBlock {
            samples,
            cursor: 0,
            channels: 1,
            generation,
            submission: None,
            output_write: None,
        })
        .unwrap()
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
        self.retired.clear();
    }

    fn discard_invalid(&mut self) {
        while self
            .blocks
            .front()
            .is_some_and(|b| b.output_write.is_none() && !self.valid(b))
        {
            self.retired
                .push_back(self.blocks.pop_front().expect("front checked"));
        }
    }

    pub fn write_next(&mut self, write: impl FnOnce(i16)) {
        loop {
            self.discard_invalid();
            let Some(block) = self.blocks.front_mut() else {
                write(0);
                return;
            };
            if block.cursor % block.channels == 0 {
                if let Some(submission) = &block.submission {
                    let Some(lease) = submission.begin_output() else {
                        self.retired
                            .push_back(self.blocks.pop_front().expect("front checked"));
                        continue;
                    };
                    block.output_write = Some(lease);
                }
            }
            // Finish an already acquired interleaved frame even if a fence is
            // applied between its channels. The next identity starts aligned.
            write(block.samples[block.cursor]);
            block.cursor += 1;
            if block.cursor % block.channels == 0 {
                if let Some(lease) = block.output_write.take() {
                    lease.finish(1);
                }
            }
            if block.cursor == block.samples.len() {
                self.retired
                    .push_back(self.blocks.pop_front().expect("front checked"));
            }
            return;
        }
    }

    #[cfg(test)]
    pub fn next_sample(&mut self) -> i16 {
        let mut value = 0;
        self.write_next(|sample| value = sample);
        value
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn alsa_chunk(&mut self, max_frames: usize) -> (Vec<i16>, Option<SubmissionWrite>) {
        self.retired.clear();
        self.discard_invalid();
        let Some(block) = self.blocks.front() else {
            return (Vec::new(), None);
        };
        let lease = if let Some(submission) = &block.submission {
            let Some(lease) = submission.begin_output() else {
                return (Vec::new(), None);
            };
            Some(lease)
        } else {
            None
        };
        let count = (block.samples.len() - block.cursor).min(max_frames * block.channels);
        (
            block.samples[block.cursor..block.cursor + count].to_vec(),
            lease,
        )
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn alsa_submitted(&mut self, frames: usize) {
        let Some(block) = self.blocks.front_mut() else {
            return;
        };
        let count = frames * block.channels;
        assert!(count <= block.samples.len() - block.cursor);
        block.cursor += count;
        if block.cursor == block.samples.len() {
            self.blocks.pop_front();
        }
    }
}

#[cfg(test)]
mod tests;
