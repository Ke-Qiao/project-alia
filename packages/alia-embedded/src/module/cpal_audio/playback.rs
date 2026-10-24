//! Local playback ordering. Control never waits for media capacity or buffer drain.
//!
//! A stop advances the generation and retires queued and in-flight media. This is
//! a local barrier: callers must still reject late network frames from old turns.
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::Duration;

use crate::module::mixer::{MixerSource, mix_sample};
use crate::module::playback_session::{
    AudioSubmission, OutputBlocked, PlaybackControl, SubmissionWrite,
};
use alia_core::delivery::{ObservationEnd, PlaybackIdentity, SubmissionBackend, VoiceScope};
use alia_core::playback_control::{ControlCommand, ControlPhase, ControlReceipt, RetiredReason};
use anyhow::{Context, Result, ensure};

mod resampler;
const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_RENDERED_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug)]
struct PcmReservation {
    bytes: usize,
    used: Arc<AtomicUsize>,
    changed: Arc<Condvar>,
}
impl Drop for PcmReservation {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
        self.changed.notify_all();
    }
}

#[derive(Debug)]
pub(super) struct AudioBlock {
    pcm: Arc<[i16]>,
    range: Range<usize>,
    pub sample_rate: u32,
    pub generation: u64,
    submission: Arc<AudioSubmission>,
    source: MixerSource,
    end: bool,
    reservation: Option<Arc<PcmReservation>>,
}

impl AudioBlock {
    fn samples(&self) -> &[i16] {
        &self.pcm[self.range.clone()]
    }

    fn is_live(&self) -> bool {
        self.submission.is_live()
    }
}

#[derive(Default)]
pub(super) struct Renderer {
    current: [Option<(PlaybackIdentity, u64, resampler::PcmResampler)>; 2],
}

impl Renderer {
    pub fn clear(&mut self) {
        self.current = [None, None];
    }

    pub fn render(
        &mut self,
        block: AudioBlock,
        rate: u32,
        channels: usize,
    ) -> Result<RenderedBlock> {
        let key = block.submission.identity;
        let current = &mut self.current[block.source.index()];
        if !current
            .as_ref()
            .is_some_and(|(old, generation, _)| *old == key && *generation == block.generation)
        {
            *current = Some((
                key,
                block.generation,
                resampler::PcmResampler::new(block.sample_rate, rate)?,
            ));
        }
        let converter = &mut current.as_mut().expect("initialized").2;
        let input: Vec<f64> = block
            .samples()
            .iter()
            .map(|&sample| f64::from(sample))
            .collect();
        let mut mono = converter.push(&input)?;
        if block.end {
            mono.extend(converter.finish()?);
            *current = None;
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
            source: block.source,
            output_write: None,
        })
    }
}

struct State {
    media: VecDeque<AudioBlock>,
    stop_pending: bool,
    closed: bool,
    inputs: [Option<PlaybackIdentity>; 2],
    submissions: HashMap<PlaybackIdentity, (Weak<AudioSubmission>, MixerSource)>,
    backend: Option<SubmissionBackend>,
    control: Option<Arc<PlaybackControl>>,
    control_stopped: bool,
    next_source: MixerSource,
}

struct Shared {
    state: Mutex<State>,
    changed: Arc<Condvar>,
    generation: Arc<AtomicU64>,
    capacity: usize,
    source_bytes: Arc<AtomicUsize>,
}

impl Shared {
    /// Media/control admission owns the mutex, so reservations cannot race
    /// other producers. The final slice wakes producers after conversion/drop.
    fn reserve_pcm(&self, bytes: usize) -> Arc<PcmReservation> {
        self.source_bytes.fetch_add(bytes, Ordering::AcqRel);
        Arc::new(PcmReservation {
            bytes,
            used: self.source_bytes.clone(),
            changed: self.changed.clone(),
        })
    }
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
            state.inputs = [None, None];
            if let Some(control) = state.control.take() {
                control.retire(match &outcome {
                    ObservationEnd::Unknown { .. } | ObservationEnd::Failed { .. } => {
                        RetiredReason::BackendChanged
                    }
                    _ if close => RetiredReason::Closed,
                    _ => RetiredReason::Cancelled,
                });
            }
            state.control_stopped = false;
            state.next_source = MixerSource::Voice;
            for submission in state.submissions.values().filter_map(|(s, _)| s.upgrade()) {
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
            inputs: [None, None],
            submissions: HashMap::new(),
            backend: None,
            control: None,
            control_stopped: false,
            next_source: MixerSource::Voice,
        }),
        changed: Arc::new(Condvar::new()),
        generation,
        capacity: capacity.max(1),
        source_bytes: Arc::new(AtomicUsize::new(0)),
    });
    (
        PlaybackSender(Arc::new(Producer(shared.clone()))),
        PlaybackReceiver(shared),
    )
}

impl PlaybackSender {
    pub fn open_submission(
        &self,
        identity: PlaybackIdentity,
        rate: u32,
        generation: u64,
    ) -> Result<Arc<AudioSubmission>> {
        self.open_source(identity, rate, generation, MixerSource::Voice)
    }

    pub fn open_cue_submission(
        &self,
        identity: PlaybackIdentity,
        rate: u32,
        generation: u64,
    ) -> Result<Arc<AudioSubmission>> {
        self.open_source(identity, rate, generation, MixerSource::LocalCue)
    }

    fn open_source(
        &self,
        identity: PlaybackIdentity,
        rate: u32,
        generation: u64,
        source: MixerSource,
    ) -> Result<Arc<AudioSubmission>> {
        let shared = &self.0.0;
        let mut state = shared.lock();
        ensure!(
            !state.closed && shared.generation.load(Ordering::Acquire) == generation,
            "playback generation is closed"
        );
        ensure!(
            state.inputs[source.index()].is_none(),
            "previous source input is still open"
        );
        state
            .submissions
            .retain(|_, (s, _)| s.upgrade().is_some_and(|s| s.is_live()));
        ensure!(
            state.submissions.len() < 32,
            "active playback observation capacity reached"
        );
        ensure!(
            !state.submissions.contains_key(&identity),
            "playback identity already registered"
        );
        if source == MixerSource::LocalCue {
            ensure!(
                !state
                    .submissions
                    .values()
                    .any(|(s, kind)| *kind == source && s.upgrade().is_some_and(|s| s.is_live())),
                "another local cue is still active"
            );
        }
        let backend = state
            .backend
            .clone()
            .context("playback backend is not ready")?;
        let control = Self::scope_control(&mut state, identity.scope, generation)?;
        let phase = control.snapshot().phase;
        ensure!(
            phase != ControlPhase::Retired,
            "playback control scope is retired"
        );
        ensure!(
            source != MixerSource::LocalCue || phase == ControlPhase::Running,
            "local cue output is held"
        );
        let submission =
            AudioSubmission::with_control(identity, generation, rate, backend, Some(control))?;
        state
            .submissions
            .insert(identity, (Arc::downgrade(&submission), source));
        state.inputs[source.index()] = Some(identity);
        Ok(submission)
    }

    fn scope_control(
        state: &mut State,
        scope: VoiceScope,
        generation: u64,
    ) -> Result<Arc<PlaybackControl>> {
        if let Some(control) = &state.control {
            ensure!(
                control.scope == scope && control.generation == generation,
                "another scope owns native playback"
            );
            return Ok(control.clone());
        }
        let control = PlaybackControl::new(
            scope,
            generation,
            state
                .backend
                .clone()
                .context("playback backend is not ready")?,
        )?;
        state.control = Some(control.clone());
        Ok(control)
    }
    pub fn playback_control(
        &self,
        scope: VoiceScope,
        generation: u64,
    ) -> Result<Arc<PlaybackControl>> {
        let shared = &self.0.0;
        let mut state = shared.lock();
        ensure!(
            !state.closed && shared.generation.load(Ordering::Acquire) == generation,
            "original playback owner is gone"
        );
        let control = Self::scope_control(&mut state, scope, generation)?;
        drop(state);
        shared.changed.notify_all();
        Ok(control)
    }
    pub fn apply_control(
        &self,
        owner: &Arc<PlaybackControl>,
        command: &ControlCommand,
    ) -> Result<ControlReceipt> {
        let shared = &self.0.0;
        let mut state = shared.lock();
        // An original owner's immutable receipt outlives native admission.
        // This path neither installs that owner nor dispatches a new command.
        if let Some(receipt) = owner.recorded_receipt(command) {
            return Ok(receipt);
        }
        ensure!(
            !state.closed
                && shared.generation.load(Ordering::Acquire) == owner.generation
                && state
                    .control
                    .as_ref()
                    .is_some_and(|c| Arc::ptr_eq(c, owner)),
            "original playback control owner is gone"
        );
        let receipt = owner.apply(command);
        if matches!(
            receipt.snapshot.phase,
            ControlPhase::Pausing | ControlPhase::Held | ControlPhase::Retired
        ) {
            // Local cues are ephemeral. Seal their exact identities while the
            // media/control lock still orders this hold; even an immediate
            // Resume cannot replay a stale cue. Voice keeps its resumable tail.
            for (submission, source) in state.submissions.values() {
                if *source == MixerSource::LocalCue {
                    if let Some(submission) = submission.upgrade() {
                        submission.terminate(ObservationEnd::Cancelled {
                            reason: "local cue suppressed by playback hold".into(),
                        });
                    }
                }
            }
            state.inputs[MixerSource::LocalCue.index()] = None;
            state
                .media
                .retain(|block| block.source != MixerSource::LocalCue);
        }
        drop(state);
        shared.changed.notify_all();
        Ok(receipt)
    }

    pub fn submit(
        &self,
        submission: &Arc<AudioSubmission>,
        offset: u64,
        pcm: &[i16],
        end: bool,
    ) -> Result<bool> {
        let shared = &self.0.0;
        let bytes = std::mem::size_of_val(pcm);
        ensure!(bytes <= MAX_PAYLOAD_BYTES, "PCM payload exceeds 1 MiB");
        let mut state = shared.lock();
        let source = state
            .submissions
            .get(&submission.identity)
            .filter(|(stored, _)| {
                stored
                    .upgrade()
                    .is_some_and(|s| Arc::ptr_eq(&s, submission))
            })
            .map(|(_, source)| *source)
            .context("playback submission owner is unknown")?;
        while !state.closed
            && submission.is_live()
            && (source != MixerSource::LocalCue || !submission.is_held())
            && shared.generation.load(Ordering::Acquire) == submission.generation
            && (state.media.iter().filter(|b| b.source == source).count() >= shared.capacity
                || shared.source_bytes.load(Ordering::Acquire) + bytes > MAX_SOURCE_BYTES)
        {
            state = shared
                .changed
                .wait_timeout(state, Duration::from_millis(2))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if state.closed
            || !submission.is_live()
            || (source == MixerSource::LocalCue && submission.is_held())
            || shared.generation.load(Ordering::Acquire) != submission.generation
        {
            return Ok(false);
        }
        ensure!(
            state.inputs[source.index()] == Some(submission.identity),
            "playback input lease is stale"
        );
        if source == MixerSource::LocalCue {
            ensure!(
                offset.saturating_add(pcm.len() as u64)
                    <= u64::from(submission.source_sample_rate) * 5,
                "local cue exceeds five seconds"
            );
        }
        if end {
            submission.accept_end(offset)?;
            state.inputs[source.index()] = None;
        } else {
            submission.accept(offset, pcm.len() as u64)?;
        }
        state.media.push_back(AudioBlock {
            pcm: pcm.into(),
            range: 0..pcm.len(),
            sample_rate: submission.source_sample_rate,
            generation: submission.generation,
            submission: submission.clone(),
            source,
            end,
            reservation: Some(shared.reserve_pcm(bytes)),
        });
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
        let Some((stored, source)) = state
            .submissions
            .get(&submission.identity)
            .and_then(|(s, source)| s.upgrade().map(|s| (s, *source)))
        else {
            return false;
        };
        if !Arc::ptr_eq(&stored, submission) {
            return false;
        }
        submission.terminate(ObservationEnd::Cancelled {
            reason: reason.into(),
        });
        state
            .media
            .retain(|block| !Arc::ptr_eq(&block.submission, submission));
        if state.inputs[source.index()] == Some(submission.identity) {
            state.inputs[source.index()] = None;
        }
        drop(state);
        shared.changed.notify_all();
        true
    }

    #[cfg(test)]
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

    #[cfg(test)]
    fn take(&self, accept_media: bool) -> Option<PlaybackAction> {
        self.take_sources([accept_media; 2])
    }

    fn take_sources(&self, accept_media: [bool; 2]) -> Option<PlaybackAction> {
        let mut state = self.0.lock();
        if state.closed {
            return Some(PlaybackAction::Shutdown);
        }
        if state.stop_pending {
            state.stop_pending = false;
            return Some(PlaybackAction::Stop);
        }
        if let Some(control) = &state.control {
            match control.snapshot().phase {
                ControlPhase::Retired => {
                    // Keep the closed control tombstone and generation. Later
                    // Start in this scope must not reserve a fresh native owner.
                    for submission in state.submissions.values().filter_map(|(s, _)| s.upgrade()) {
                        submission.terminate(ObservationEnd::Cancelled {
                            reason: "playback hold retired or expired".into(),
                        });
                    }
                    state.inputs = [None, None];
                    let stop = !state.control_stopped;
                    state.control_stopped = true;
                    let retired = std::mem::take(&mut state.media);
                    drop(state);
                    self.0.changed.notify_all();
                    drop(retired);
                    return stop.then_some(PlaybackAction::Stop);
                }
                ControlPhase::Pausing | ControlPhase::Held => return None,
                ControlPhase::Running => {}
            }
        }
        // Bound each conversion to 20 ms. The worker fills each native lane
        // independently, so a long voice payload cannot serialize a later cue.
        state.media.retain(AudioBlock::is_live);
        let preferred = state.next_source;
        let selected = [preferred, MixerSource::ALL[1 - preferred.index()]]
            .into_iter()
            .find_map(|source| {
                accept_media[source.index()]
                    .then(|| state.media.iter().position(|b| b.source == source))
                    .flatten()
            });
        let Some(index) = selected else {
            return None;
        };
        let queued = &mut state.media[index];
        let count = (queued.sample_rate as usize / 50).clamp(1, 4096);
        let end = (queued.range.start + count).min(queued.pcm.len());
        let block = AudioBlock {
            pcm: queued.pcm.clone(),
            range: queued.range.start..end,
            sample_rate: queued.sample_rate,
            generation: queued.generation,
            submission: queued.submission.clone(),
            source: queued.source,
            end: queued.end && end == queued.pcm.len(),
            reservation: queued.reservation.clone(),
        };
        queued.range.start = end;
        let retired = queued.range.is_empty().then(|| state.media.remove(index));
        state.next_source = MixerSource::ALL[1 - block.source.index()];
        drop(state);
        if retired.is_some() {
            self.0.changed.notify_all();
        }
        drop(retired);
        Some(PlaybackAction::Play(block))
    }

    fn wait(&self, accept_media: [bool; 2], idle: bool) {
        self.wait_sources_before(accept_media, idle, || {});
    }
    #[cfg(test)]
    fn wait_before(&self, accept_media: bool, idle: bool, before_wait: impl FnOnce()) {
        self.wait_sources_before([accept_media; 2], idle, before_wait);
    }
    fn wait_sources_before(&self, accept_media: [bool; 2], idle: bool, before_wait: impl FnOnce()) {
        let state = self.0.lock();
        let held = state.control.as_ref().is_some_and(|c| c.is_held());
        let control_key = |state: &State| {
            state
                .control
                .as_ref()
                .map(|c| (Arc::as_ptr(c), c.is_held(), c.is_retired()))
        };
        let before = control_key(&state);
        let uncontrolled = state.control.is_none();
        let waiting = |state: &mut State| {
            !state.closed
                && !state.stop_pending
                && control_key(state) == before
                && (held || !state.media.iter().any(|b| accept_media[b.source.index()]))
        };
        before_wait();
        if idle && uncontrolled {
            drop(self.0.changed.wait_while(state, waiting));
        } else {
            // Device consumption has no portable stdlib wake handle. Control and
            // new media wake this wait immediately. An idle scope also checks
            // capture retirement, which does not take the media admission lock.
            drop(self.0.changed.wait_timeout_while(
                state,
                Duration::from_millis(if idle && !held { 20 } else { 2 }),
                waiting,
            ));
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
    fn set_control(&mut self, _control: Option<Arc<PlaybackControl>>) {}
    fn buffered_ms(&self) -> u64;
    fn source_buffered_ms(&self, _source: MixerSource) -> u64 {
        self.buffered_ms()
    }
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
        'worker: loop {
            sink.set_control(receiver.0.lock().control.clone());
            let mut accept = MixerSource::ALL
                .map(|source| sink.source_buffered_ms(source) < target_buffer_ms.max(1));
            let mut enqueued = false;
            // Mix one ready block per lane without starving native writes.
            for _ in MixerSource::ALL {
                match receiver.take_sources(accept) {
                    Some(PlaybackAction::Shutdown) => return Ok(()),
                    Some(PlaybackAction::Stop) => {
                        sink.stop()?;
                        continue 'worker;
                    }
                    Some(PlaybackAction::Play(block)) => {
                        accept[block.source.index()] = false;
                        sink.enqueue(block)?;
                        enqueued = true;
                    }
                    None => break,
                }
            }
            sink.pump()?;
            if !enqueued {
                let buffered_ms = sink.buffered_ms();
                let accept = MixerSource::ALL
                    .map(|source| sink.source_buffered_ms(source) < target_buffer_ms.max(1));
                receiver.wait(accept, buffered_ms == 0);
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
    submission: Arc<AudioSubmission>,
    source: MixerSource,
    output_write: Option<SubmissionWrite>,
}

/// Both lanes advance on the same native frame clock. They retain independent
/// source cursors and output leases all the way through the actual native write.
pub(super) struct PlaybackBuffer {
    lanes: [PlaybackLane; 2],
    channels: Option<usize>,
    #[cfg(any(target_os = "linux", test))]
    offered: [usize; 2],
}

impl PlaybackBuffer {
    pub fn new(current: Arc<AtomicU64>) -> Self {
        Self {
            lanes: [
                PlaybackLane::new(current.clone()),
                PlaybackLane::new(current),
            ],
            channels: None,
            #[cfg(any(target_os = "linux", test))]
            offered: [0; 2],
        }
    }
    pub fn is_current(&self, generation: u64) -> bool {
        self.lanes[0].is_current(generation)
    }
    pub fn set_control(&mut self, control: Option<Arc<PlaybackControl>>) {
        for lane in &mut self.lanes {
            lane.set_control(control.clone());
        }
    }
    pub fn is_held(&self) -> bool {
        self.lanes.iter().any(PlaybackLane::is_held)
    }
    #[cfg(target_os = "linux")]
    pub fn has_control(&self) -> bool {
        self.lanes.iter().any(PlaybackLane::has_control)
    }
    pub fn len(&self) -> usize {
        self.lanes.iter().map(PlaybackLane::len).max().unwrap_or(0)
    }
    pub fn source_len(&self, source: MixerSource) -> usize {
        self.lanes[source.index()].len()
    }
    pub fn append_rendered(&mut self, block: RenderedBlock) -> Result<bool> {
        for lane in &mut self.lanes {
            lane.discard_invalid();
            lane.retired.clear(); // Worker only: callback never frees PCM.
        }
        ensure!(
            self.channels
                .is_none_or(|channels| channels == block.channels),
            "mixed output format changed"
        );
        ensure!(
            self.lanes
                .iter()
                .map(|lane| lane.blocks.len())
                .sum::<usize>()
                < MAX_RENDERED_BLOCKS,
            "mixed playback queue capacity exceeded"
        );
        let retained = self
            .lanes
            .iter()
            .flat_map(|lane| lane.blocks.iter())
            .map(|b| std::mem::size_of_val(b.samples.as_slice()))
            .sum::<usize>();
        ensure!(
            retained
                .checked_add(std::mem::size_of_val(block.samples.as_slice()))
                .is_some_and(|bytes| bytes <= MAX_RENDERED_BYTES),
            "mixed playback exceeds 2 MiB retention budget"
        );
        self.channels = Some(block.channels);
        if let Some(control) = block.submission.control() {
            self.set_control(Some(control.clone()));
        }
        self.lanes[block.source.index()].append_rendered(block)
    }
    pub fn clear(&mut self) {
        for lane in &mut self.lanes {
            lane.clear();
        }
        self.channels = None;
        #[cfg(any(target_os = "linux", test))]
        {
            self.offered = [0; 2];
        }
    }
    pub fn write_next(&mut self, write: impl FnOnce(i16)) -> SampleContent {
        self.write_next_with(write, AudioSubmission::try_begin_output)
    }
    fn write_next_with(
        &mut self,
        write: impl FnOnce(i16),
        mut acquire: impl FnMut(
            &Arc<AudioSubmission>,
        ) -> std::result::Result<SubmissionWrite, OutputBlocked>,
    ) -> SampleContent {
        let [voice, cue] = &mut self.lanes;
        let mut cue_content = SampleContent::ZeroFill;
        // Nest writes so neither lane can settle its frame before the final
        // mixed sample has actually been written to the callback output.
        let voice_content = voice.write_next_with(
            |v| {
                cue_content = cue.write_next(|c| write(mix_sample(v, c)));
            },
            &mut acquire,
        );
        if voice_content == SampleContent::Media || cue_content == SampleContent::Media {
            SampleContent::Media
        } else if voice_content == SampleContent::Held || cue_content == SampleContent::Held {
            SampleContent::Held
        } else {
            SampleContent::ZeroFill
        }
    }
    #[cfg(test)]
    pub fn append(&mut self, generation: u64, samples: Vec<i16>) -> bool {
        self.append_rendered(RenderedBlock {
            samples,
            cursor: 0,
            channels: 1,
            generation,
            submission: tests::test_submission(generation, 16000, 1),
            source: MixerSource::Voice,
            output_write: None,
        })
        .unwrap()
    }
    #[cfg(test)]
    pub fn next_sample(&mut self) -> i16 {
        let mut value = 0;
        self.write_next(|sample| value = sample);
        value
    }
    #[cfg(any(target_os = "linux", test))]
    pub fn alsa_chunk(&mut self, max_frames: usize) -> (Vec<i16>, Option<MixedWrite>) {
        let [(voice, voice_write), (cue, cue_write)] = self
            .lanes
            .each_mut()
            .map(|lane| lane.alsa_chunk(max_frames));
        let channels = self.channels.unwrap_or(1);
        // Stop at the first active lane's block boundary. Advancing farther
        // would insert silence before its next already queued source block.
        let count = [voice.len(), cue.len()]
            .into_iter()
            .filter(|n| *n > 0)
            .min()
            .unwrap_or(0);
        if count == 0 {
            self.offered = [0; 2];
            return (Vec::new(), None);
        }
        self.offered = [
            if voice.is_empty() {
                0
            } else {
                count / channels
            },
            if cue.is_empty() { 0 } else { count / channels },
        ];
        let mixed = (0..count)
            .map(|i| {
                mix_sample(
                    voice.get(i).copied().unwrap_or(0),
                    cue.get(i).copied().unwrap_or(0),
                )
            })
            .collect();
        (
            mixed,
            Some(MixedWrite {
                writes: [voice_write, cue_write],
                offered: self.offered,
            }),
        )
    }
    #[cfg(any(target_os = "linux", test))]
    pub fn alsa_submitted(&mut self, frames: usize) {
        for (lane, offered) in self.lanes.iter_mut().zip(self.offered) {
            lane.alsa_submitted(frames.min(offered));
        }
    }
}

#[cfg(any(target_os = "linux", test))]
pub(super) struct MixedWrite {
    writes: [Option<SubmissionWrite>; 2],
    offered: [usize; 2],
}
#[cfg(any(target_os = "linux", test))]
impl MixedWrite {
    fn finish(self, frames: u64) {
        for (write, offered) in self.writes.into_iter().zip(self.offered) {
            if let Some(write) = write {
                write.finish(frames.min(offered as u64));
            }
        }
    }
}

struct PlaybackLane {
    blocks: VecDeque<RenderedBlock>,
    // The callback moves consumed allocations here. The worker frees them; no
    // PCM buffer allocation/deallocation is required on the realtime callback.
    retired: VecDeque<RenderedBlock>,
    current: Arc<AtomicU64>,
    control: Option<Arc<PlaybackControl>>,
    held_channels: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SampleContent {
    Media,
    Held,
    ZeroFill,
}

const MAX_RENDERED_BLOCKS: usize = 512;

impl PlaybackLane {
    pub fn new(current: Arc<AtomicU64>) -> Self {
        Self {
            blocks: VecDeque::with_capacity(MAX_RENDERED_BLOCKS),
            retired: VecDeque::with_capacity(MAX_RENDERED_BLOCKS),
            current,
            control: None,
            held_channels: 0,
        }
    }

    pub fn is_current(&self, generation: u64) -> bool {
        generation == self.current.load(Ordering::Acquire)
    }
    pub fn set_control(&mut self, control: Option<Arc<PlaybackControl>>) {
        self.control = control;
    }
    pub fn is_held(&self) -> bool {
        self.control.as_ref().is_some_and(|c| c.is_held())
    }
    #[cfg(target_os = "linux")]
    pub fn has_control(&self) -> bool {
        self.control.is_some()
    }

    fn valid(&self, block: &RenderedBlock) -> bool {
        self.is_current(block.generation) && block.submission.is_live()
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
        if let Some(control) = block.submission.control() {
            self.control = Some(control.clone());
        }
        ensure!(
            block.channels > 0 && block.samples.len() % block.channels == 0,
            "incomplete rendered frame"
        );
        ensure!(
            self.blocks.len() < MAX_RENDERED_BLOCKS,
            "rendered playback queue capacity exceeded"
        );
        ensure!(
            self.blocks
                .iter()
                .map(|b| std::mem::size_of_val(b.samples.as_slice()))
                .sum::<usize>()
                .checked_add(std::mem::size_of_val(block.samples.as_slice()))
                .is_some_and(|bytes| bytes <= MAX_RENDERED_BYTES),
            "rendered playback exceeds 2 MiB retention budget"
        );
        self.blocks.push_back(block);
        Ok(true)
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
        self.retired.clear();
        self.held_channels = 0;
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

    pub fn write_next(&mut self, write: impl FnOnce(i16)) -> SampleContent {
        self.write_next_with(write, AudioSubmission::try_begin_output)
    }
    fn write_next_with(
        &mut self,
        write: impl FnOnce(i16),
        mut acquire: impl FnMut(
            &Arc<AudioSubmission>,
        ) -> std::result::Result<SubmissionWrite, OutputBlocked>,
    ) -> SampleContent {
        if self.held_channels > 0 {
            self.held_channels -= 1;
            write(0);
            return SampleContent::Held;
        }
        loop {
            self.discard_invalid();
            let Some(block) = self.blocks.front_mut() else {
                write(0);
                return if self.is_held() {
                    SampleContent::Held
                } else {
                    SampleContent::ZeroFill
                };
            };
            if block.cursor % block.channels == 0 {
                {
                    let lease = match acquire(&block.submission) {
                        Ok(lease) => lease,
                        Err(OutputBlocked::Held) => {
                            self.held_channels = block.channels - 1;
                            write(0);
                            return SampleContent::Held;
                        }
                        Err(OutputBlocked::Retired) => {
                            self.retired
                                .push_back(self.blocks.pop_front().expect("front checked"));
                            continue;
                        }
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
            return SampleContent::Media;
        }
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn alsa_chunk(&mut self, max_frames: usize) -> (Vec<i16>, Option<SubmissionWrite>) {
        self.retired.clear();
        self.discard_invalid();
        let Some(block) = self.blocks.front() else {
            return (Vec::new(), None);
        };
        let lease = {
            let lease = match block.submission.try_begin_output() {
                Ok(lease) => lease,
                Err(OutputBlocked::Held | OutputBlocked::Retired) => return (Vec::new(), None),
            };
            Some(lease)
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

/// Only a successful native write's accepted prefix reaches this function.
/// In-flight cancellation cannot erase that fact or include the unwritten suffix.
#[cfg(any(target_os = "linux", test))]
pub(super) fn confirm_alsa_write(
    buffer: &mut PlaybackBuffer,
    chunk: &[i16],
    written_frames: usize,
    lease: Option<MixedWrite>,
    clock: &mut super::super::context::AudioStreamClock,
    reading: alia_core::audio_clock::AudioClockReading,
) -> Result<()> {
    let channels = usize::from(clock.probe.format().channels);
    ensure!(
        written_frames <= chunk.len() / channels,
        "ALSA accepted beyond the offered buffer"
    );
    buffer.alsa_submitted(written_frames);
    super::clock::record_samples(
        clock,
        &chunk[..written_frames * channels],
        reading,
        alia_core::delivery::SubmissionBoundary::AlsaWriteAccepted,
        super::super::context::ReferenceContent::Media,
    );
    if let Some(lease) = lease {
        lease.finish(written_frames as u64);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
