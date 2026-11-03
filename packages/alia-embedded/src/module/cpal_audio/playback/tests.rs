use super::*;
use alia_core::delivery::{SubmissionBoundary, VoiceScope};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread;
mod control;
mod mixer;
mod offline_evidence;

pub(super) fn test_submission(generation: u64, rate: u32, channels: u32) -> Arc<AudioSubmission> {
    AudioSubmission::new(
        PlaybackIdentity {
            scope: VoiceScope {
                device_boot_id: uuid::Uuid::from_u128(1),
                session_id: uuid::Uuid::from_u128(2),
                generation_epoch: 1,
            },
            playback_id: uuid::Uuid::new_v4(),
            playback_sequence: 1,
        },
        generation,
        rate,
        SubmissionBackend {
            instance_id: uuid::Uuid::from_u128(10),
            boundary: SubmissionBoundary::CpalCallbackBuffer,
            output_sample_rate_hz: rate,
            output_channels: channels,
        },
    )
    .unwrap()
}

// Bounded finite-clip fixture. It exercises the same scoped admission and native
// owner as streamed production media without requiring a second queue slot for
// End before the test starts its consumer.
impl PlaybackSender {
    fn submit_clip(&self, pcm: &[i16], rate: u32) -> Result<()> {
        self.submit_clip_if_current(pcm, rate, self.0.0.generation.load(Ordering::Acquire))
            .map(|_| ())
    }
    fn submit_clip_if_current(&self, pcm: &[i16], rate: u32, generation: u64) -> Result<bool> {
        let shared = &self.0.0;
        {
            let mut state = shared.lock();
            ensure!(!state.closed, "test native runtime closed");
            if shared.generation.load(Ordering::Acquire) != generation {
                return Ok(false);
            }
            state.backend.get_or_insert(SubmissionBackend {
                instance_id: uuid::Uuid::from_u128(10),
                boundary: SubmissionBoundary::CpalCallbackBuffer,
                output_sample_rate_hz: rate,
                output_channels: 1,
            });
        }
        let identity = test_submission(generation, rate, 1).identity;
        let submission = self.open_submission(identity, rate, generation)?;
        let mut state = shared.lock();
        let bytes = std::mem::size_of_val(pcm);
        while !state.closed
            && submission.is_live()
            && shared.generation.load(Ordering::Acquire) == generation
            && (state.media.len() >= shared.capacity
                || shared.source_bytes.load(Ordering::Acquire) + bytes > MAX_SOURCE_BYTES)
        {
            state = shared
                .changed
                .wait_timeout(state, Duration::from_millis(2))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        ensure!(!state.closed, "test native runtime closed");
        if !submission.is_live() || shared.generation.load(Ordering::Acquire) != generation {
            return Ok(false);
        }
        submission.accept(0, pcm.len() as u64)?;
        submission.accept_end(pcm.len() as u64)?;
        state.inputs[MixerSource::Voice.index()] = None;
        state.media.push_back(AudioBlock {
            pcm: pcm.into(),
            range: 0..pcm.len(),
            sample_rate: rate,
            generation,
            submission,
            source: MixerSource::Voice,
            end: true,
            reservation: Some(shared.reserve_pcm(bytes)),
        });
        drop(state);
        shared.changed.notify_all();
        Ok(true)
    }
}

fn identified(
    sender: &PlaybackSender,
    receiver: &PlaybackReceiver,
    sequence: u64,
    generation: u64,
) -> Arc<AudioSubmission> {
    identified_at(
        sender,
        receiver,
        sequence,
        generation,
        SubmissionBackend {
            instance_id: uuid::Uuid::from_u128(10),
            boundary: SubmissionBoundary::CpalCallbackBuffer,
            output_sample_rate_hz: 16000,
            output_channels: 2,
        },
    )
}
fn identified_at(
    sender: &PlaybackSender,
    receiver: &PlaybackReceiver,
    sequence: u64,
    generation: u64,
    backend: SubmissionBackend,
) -> Arc<AudioSubmission> {
    receiver.set_backend(backend);
    sender
        .open_submission(
            PlaybackIdentity {
                scope: VoiceScope {
                    device_boot_id: uuid::Uuid::from_u128(1),
                    session_id: uuid::Uuid::from_u128(2),
                    generation_epoch: 1,
                },
                playback_id: uuid::Uuid::from_u128(100 + sequence as u128),
                playback_sequence: sequence,
            },
            16000,
            generation,
        )
        .unwrap()
}

fn render_pending(
    receiver: &PlaybackReceiver,
    renderer: &mut Renderer,
    buffer: &mut PlaybackBuffer,
) {
    while let Some(action) = receiver.take(true) {
        match action {
            PlaybackAction::Stop => {
                buffer.clear();
                renderer.clear();
            }
            PlaybackAction::Play(block) => {
                buffer
                    .append_rendered(renderer.render(block, 16000, 2).unwrap())
                    .unwrap();
            }
            PlaybackAction::Shutdown => break,
        }
    }
}

#[test]
fn end_keeps_observation_open_and_precise_cancel_preserves_later_fifo_audio() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let first = identified(&sender, &receiver, 1, generation);
    sender.submit(&first, 0, &[1, 2, 3, 4], false).unwrap();
    sender.submit(&first, 4, &[], true).unwrap();
    let second = identified(&sender, &receiver, 2, generation);
    sender.submit(&second, 0, &[5, 6, 7], false).unwrap();
    sender.submit(&second, 3, &[], true).unwrap();
    let before = first.snapshot(1);
    assert!(before.end_accepted);
    assert_eq!(before.submitted_source_frames, Some(0));
    assert!(before.observation_end.is_none());
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    render_pending(&receiver, &mut Renderer::default(), &mut buffer);
    for expected in [1, 1, 2, 2] {
        assert_eq!(buffer.next_sample(), expected);
    }
    assert_eq!(first.snapshot(2).submitted_source_frames, Some(2));
    assert!(sender.cancel_submission(&first, "exact cancel"));
    for expected in [5, 5, 6, 6, 7, 7] {
        assert_eq!(buffer.next_sample(), expected);
    }
    assert_eq!(buffer.next_sample(), 0);
    let cancelled = first.snapshot(3);
    cancelled.validate(Some(&before), 4).unwrap();
    assert_eq!(cancelled.submitted_source_frames, Some(2));
    assert!(matches!(
        cancelled.observation_end,
        Some(ObservationEnd::Cancelled { .. })
    ));
    let done = second.snapshot(1);
    done.validate(None, 3).unwrap();
    assert_eq!(
        done.observation_end,
        Some(ObservationEnd::SubmissionFinished)
    );
    assert_eq!(done.presented_source_frames, None);
}

#[test]
fn partial_alsa_writes_count_complete_frames_and_never_count_underrun_silence() {
    let (sender, receiver) = channel(4);
    let generation = sender.begin().unwrap();
    let playback = identified(&sender, &receiver, 1, generation);
    sender.submit(&playback, 0, &[11, 12, 13], false).unwrap();
    sender.submit(&playback, 3, &[], true).unwrap();
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    render_pending(&receiver, &mut Renderer::default(), &mut buffer);
    let (chunk, write) = buffer.alsa_chunk(3);
    assert_eq!(chunk, [11, 11, 12, 12, 13, 13]);
    // Preparing a write / an EAGAIN return has submitted nothing.
    assert_eq!(playback.snapshot(1).submitted_source_frames, Some(0));
    buffer.alsa_submitted(1);
    write.unwrap().finish(1);
    assert_eq!(playback.snapshot(2).submitted_source_frames, Some(1));
    let (chunk, write) = buffer.alsa_chunk(3);
    assert_eq!(chunk, [12, 12, 13, 13]);
    buffer.alsa_submitted(2);
    write.unwrap().finish(2);
    let done = playback.snapshot(3);
    assert_eq!(done.submitted_source_frames, Some(3));
    assert_eq!(
        done.observation_end,
        Some(ObservationEnd::SubmissionFinished)
    );
    for _ in 0..100 {
        assert_eq!(buffer.next_sample(), 0);
    }
    assert_eq!(playback.snapshot(4).submitted_source_frames, Some(3));
}

#[derive(Debug, PartialEq)]
enum Event {
    Stalled,
    Enqueued(Vec<i16>),
    Output(Vec<i16>),
    Stopped,
}

struct TestSink {
    buffer: PlaybackBuffer,
    stalled: bool,
    reported_stall: bool,
    events: mpsc::Sender<Event>,
}

impl PlaybackSink for TestSink {
    fn buffered_ms(&self) -> u64 {
        self.buffer.len() as u64 // The fake output clock is 1 kHz.
    }

    fn enqueue(&mut self, block: AudioBlock) -> Result<()> {
        let samples = block.samples().to_vec();
        if self.buffer.append(block.generation, samples.clone()) {
            self.events.send(Event::Enqueued(samples)).unwrap();
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.buffer.clear();
        self.stalled = false;
        self.events.send(Event::Stopped).unwrap();
        Ok(())
    }

    fn pump(&mut self) -> Result<()> {
        if self.stalled {
            if !self.reported_stall {
                self.events.send(Event::Stalled).unwrap();
                self.reported_stall = true;
            }
        } else if self.buffer.len() > 0 {
            let output = (0..self.buffer.len())
                .map(|_| self.buffer.next_sample())
                .collect();
            self.events.send(Event::Output(output)).unwrap();
        }
        Ok(())
    }
}

fn worker(
    receiver: PlaybackReceiver,
    initially_full: bool,
) -> (thread::JoinHandle<Result<()>>, mpsc::Receiver<Event>) {
    let (events, received) = mpsc::channel();
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    if initially_full {
        buffer.append(0, vec![9; 100]);
    }
    let handle = thread::spawn(move || {
        let mut sink = TestSink {
            buffer,
            stalled: initially_full,
            reported_stall: false,
            events,
        };
        run(receiver, &mut sink, 10)
    });
    (handle, received)
}

fn next(events: &mpsc::Receiver<Event>) -> Event {
    // A deadlock guard, not a claim about real sound-card latency.
    events.recv_timeout(Duration::from_secs(1)).unwrap()
}

#[test]
fn queued_media_does_not_starve_native_output_until_buffer_target() {
    let (sender, receiver) = channel(1);
    let pcm: Vec<i16> = (0..1600).collect();
    sender.submit_clip(&pcm, 8000).unwrap();
    let buffer = PlaybackBuffer::new(receiver.generation());
    let (events, received) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut sink = TestSink {
            buffer,
            stalled: false,
            reported_stall: false,
            events,
        };
        // The fake output target exceeds a conversion block; writes must not
        // wait for that target while more source media is queued.
        run(receiver, &mut sink, 300)
    });
    let mut observed = Vec::new();
    let mut output_samples = 0;
    while output_samples < pcm.len() {
        let event = next(&received);
        if let Event::Output(samples) = &event {
            output_samples += samples.len();
        }
        observed.push(event);
    }
    sender.shutdown();
    assert_eq!(next(&received), Event::Stopped);
    handle.join().unwrap().unwrap();

    let expected: Vec<_> = pcm
        .chunks(160)
        .flat_map(|chunk| {
            [
                Event::Enqueued(chunk.to_vec()),
                Event::Output(chunk.to_vec()),
            ]
        })
        .collect();
    assert_eq!(observed, expected);
}

#[test]
fn full_device_buffer_and_full_media_queue_do_not_delay_stop() {
    let (sender, receiver) = channel(1);
    sender.submit_clip(&[1], 8000).unwrap(); // The only media slot is full.
    let (handle, events) = worker(receiver, true);
    assert_eq!(next(&events), Event::Stalled); // The device does not drain.

    sender.stop();
    assert_eq!(next(&events), Event::Stopped);
    sender.shutdown();
    assert_eq!(next(&events), Event::Stopped);
    handle.join().unwrap().unwrap();
}

#[test]
fn stop_discards_queued_media_and_allows_new_playback() {
    let (sender, receiver) = channel(2);
    sender.submit_clip(&[1], 8000).unwrap();
    sender.submit_clip(&[2], 8000).unwrap();
    sender.stop();
    sender.submit_clip(&[3, 4], 8000).unwrap();

    let (handle, events) = worker(receiver, false);
    assert_eq!(next(&events), Event::Stopped);
    assert_eq!(next(&events), Event::Enqueued(vec![3, 4]));
    assert_eq!(next(&events), Event::Output(vec![3, 4]));
    sender.shutdown();
    assert_eq!(next(&events), Event::Stopped);
    handle.join().unwrap().unwrap();
}

#[test]
fn stop_invalidates_rendered_audio_and_a_block_already_in_flight() {
    let (sender, receiver) = channel(1);
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    sender.submit_clip(&[1], 8000).unwrap();
    let Some(PlaybackAction::Play(in_flight)) = receiver.take(true) else {
        panic!("expected submitted audio");
    };
    buffer.append(in_flight.generation, vec![7, 8]);

    sender.stop();
    // The callback rejects old audio even before the worker handles Stop.
    assert_eq!(buffer.next_sample(), 0);
    assert!(!buffer.append(in_flight.generation, in_flight.samples().to_vec()));
    assert_eq!(buffer.next_sample(), 0);

    assert!(matches!(receiver.take(true), Some(PlaybackAction::Stop)));
    sender.submit_clip(&[5], 8000).unwrap();
    let Some(PlaybackAction::Play(new)) = receiver.take(true) else {
        panic!("expected new-generation audio");
    };
    assert!(buffer.append(new.generation, new.samples().to_vec()));
    assert_eq!(buffer.next_sample(), 5);
}

#[test]
fn disconnect_shuts_down_without_waiting_for_the_device_to_drain() {
    let (sender, receiver) = channel(1);
    sender.submit_clip(&[1], 8000).unwrap();
    let (handle, events) = worker(receiver, true);
    assert_eq!(next(&events), Event::Stalled);
    drop(sender);
    assert_eq!(next(&events), Event::Stopped);
    handle.join().unwrap().unwrap();
}

#[test]
fn disconnect_wakes_an_idle_worker() {
    let (sender, receiver) = channel(1);
    let (handle, events) = worker(receiver, false);
    drop(sender);
    assert_eq!(next(&events), Event::Stopped);
    handle.join().unwrap().unwrap();
}

#[test]
fn device_failure_wakes_worker_and_rejects_further_media() {
    let (sender, receiver) = channel(1);
    let fail = receiver.device_failed();
    let (handle, events) = worker(receiver, false);
    fail();
    assert_eq!(next(&events), Event::Stopped);
    handle.join().unwrap().unwrap();
    assert!(sender.submit_clip(&[1], 8000).is_err());
}

#[test]
fn receiver_failure_releases_a_producer_when_the_queue_is_full() {
    let (sender, receiver) = channel(1);
    sender.submit_clip(&[1], 8000).unwrap();
    let (result_tx, result_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        result_tx.send(sender.submit_clip(&[2], 8000)).unwrap();
    });
    drop(receiver);
    assert!(
        result_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    handle.join().unwrap();
}

#[test]
fn repeated_stop_coalesces_without_a_control_backlog() {
    let (sender, receiver) = channel(1);
    for _ in 0..1000 {
        sender.stop();
    }
    assert!(matches!(receiver.take(false), Some(PlaybackAction::Stop)));
    assert!(receiver.take(false).is_none());
    sender.submit_clip(&[1], 8000).unwrap();
    assert!(matches!(receiver.take(true), Some(PlaybackAction::Play(_))));
}

#[test]
fn stop_retires_the_remainder_of_a_partly_consumed_clip() {
    let (sender, receiver) = channel(1);
    sender.submit_clip(&vec![1; 1000], 8000).unwrap();
    let Some(PlaybackAction::Play(first)) = receiver.take(true) else {
        panic!("expected first block");
    };
    assert_eq!(first.samples(), &[1; 160]);
    sender.stop();
    sender.submit_clip(&[2], 8000).unwrap();
    assert!(matches!(receiver.take(true), Some(PlaybackAction::Stop)));
    let Some(PlaybackAction::Play(fresh)) = receiver.take(true) else {
        panic!("expected fresh clip");
    };
    assert_eq!(fresh.samples(), &[2]);
    assert!(receiver.take(true).is_none());
}

#[test]
fn worker_blocks_preserve_resampling_phase_and_duration() {
    // 11025 Hz does not divide into the worker's 20 ms source blocks.
    let pcm: Vec<i16> = (0..11025).collect();
    for output_rate in [8000, 48000] {
        let (sender, receiver) = channel(1);
        sender.submit_clip(&pcm, 11025).unwrap();
        let mut rendered = Vec::new();
        let mut renderer = Renderer::default();
        while let Some(PlaybackAction::Play(block)) = receiver.take(true) {
            rendered.extend(renderer.render(block, output_rate, 2).unwrap().samples);
        }
        assert_eq!(rendered.len(), output_rate as usize * 2);
        for stereo in rendered.chunks_exact(2) {
            assert_eq!(stereo[0], stereo[1]);
        }
    }
}

#[test]
fn submillisecond_device_tail_is_not_idle() {
    assert_eq!(super::super::playback_buffered_ms(0, 48000, 2), 0);
    assert_eq!(super::super::playback_buffered_ms(2, 48000, 2), 1);
}

#[test]
fn scoped_media_waiting_for_capacity_cannot_revive_after_stop() {
    let (sender, receiver) = channel(1);
    let generation = sender.begin().unwrap();
    assert!(
        sender
            .submit_clip_if_current(&[1], 8000, generation)
            .unwrap()
    );
    let producer = sender.clone();
    let (result_tx, result_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        result_tx
            .send(producer.submit_clip_if_current(&[2], 8000, generation))
            .unwrap();
    });
    assert!(sender.stop_if_current(generation));
    let fresh = sender.begin().unwrap();
    assert!(!sender.stop_if_current(generation));
    assert!(
        !result_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap()
    );
    assert!(sender.submit_clip_if_current(&[3], 8000, fresh).unwrap());
    assert!(matches!(receiver.take(true), Some(PlaybackAction::Stop)));
    let Some(PlaybackAction::Play(block)) = receiver.take(true) else {
        panic!("fresh media missing")
    };
    assert_eq!(block.samples(), &[3]);
    handle.join().unwrap();
}

#[test]
fn restarting_a_worker_never_reuses_a_token_or_invalidates_it_from_an_old_drop() {
    let generation = Arc::new(AtomicU64::new(0));
    let (old_sender, old_receiver) = channel_with_generation(1, generation.clone());
    let old = old_sender.begin().unwrap();
    old_sender.shutdown();
    let (sender, receiver) = channel_with_generation(1, generation.clone());
    let fresh = sender.begin().unwrap();
    drop(old_sender);
    drop(old_receiver);
    assert!(fresh > old);
    assert_eq!(generation.load(Ordering::Acquire), fresh);
    assert!(!sender.submit_clip_if_current(&[1], 8000, old).unwrap());
    assert!(sender.submit_clip_if_current(&[2], 8000, fresh).unwrap());
    drop(receiver);
}

#[test]
fn scoped_ingress_never_initializes_or_resurrects_native_audio() {
    use crate::module::traits::AudioPlaybackBackend;
    let backend = super::super::CpalAudioPlayback::new(
        Default::default(),
        crate::module::volume::AudioVolumeState::new(Default::default()),
    );
    assert!(
        backend
            .begin_scoped_playback()
            .unwrap_err()
            .to_string()
            .contains("not running")
    );
    let identity = PlaybackIdentity {
        scope: VoiceScope {
            device_boot_id: uuid::Uuid::new_v4(),
            session_id: uuid::Uuid::new_v4(),
            generation_epoch: 1,
        },
        playback_id: uuid::Uuid::new_v4(),
        playback_sequence: 1,
    };
    assert!(
        backend
            .open_submission(identity, 16000, 1)
            .unwrap_err()
            .to_string()
            .contains("not running")
    );
    assert!(
        backend.runtime.lock().unwrap().is_none(),
        "no hardware was opened by network admission"
    );
}

#[test]
fn playback_startup_failure_shuts_down_and_joins_native_owner() {
    for rejected in [false, true] {
        let (sender, receiver) = channel(1);
        let (ready, startup) = mpsc::sync_channel(1);
        let (finished, completion) = mpsc::channel();
        let exited = Arc::new(AtomicBool::new(false));
        let worker_exited = exited.clone();
        let worker = thread::spawn(move || {
            if rejected {
                ready
                    .send(Err("synthetic startup rejection".into()))
                    .unwrap();
            }
            drop(ready);
            while !matches!(receiver.take(false), Some(PlaybackAction::Shutdown)) {
                receiver.wait([false; 2], true);
            }
            worker_exited.store(true, Ordering::Release);
            finished.send(()).unwrap();
        });
        let result = super::super::await_playback_startup(sender.clone(), worker, startup);
        let joined_before_return = exited.load(Ordering::Acquire);
        // Also release the fake owner if a regression drops its JoinHandle.
        sender.shutdown();
        completion.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            joined_before_return,
            "startup returned before native cleanup"
        );
        let error = result.err().expect("failed startup became available");
        assert!(error.to_string().contains(if rejected {
            "synthetic startup rejection"
        } else {
            "waiting for cpal playback startup"
        }));
    }
}

#[test]
fn playback_startup_panic_preserves_ready_failure() {
    for rejected in [false, true] {
        let (sender, _receiver) = channel(1);
        let (ready, startup) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            if rejected {
                ready
                    .send(Err("synthetic startup rejection".into()))
                    .unwrap();
            }
            panic!("synthetic native startup panic");
        });
        let error = super::super::await_playback_startup(sender, worker, startup)
            .err()
            .expect("panicked startup became available");
        let chain = format!("{error:#}");
        assert!(chain.contains("playback worker panicked during startup"));
        assert!(chain.contains(if rejected {
            "synthetic startup rejection"
        } else {
            "waiting for cpal playback startup"
        }));
    }
}

#[tokio::test]
async fn playback_owner_health_never_reopens_device_and_tracks_exit_and_stop() {
    use super::super::{CpalAudioPlayback, PlaybackRuntime};
    use crate::config::{AudioBackend, AudioConfig};
    use crate::module::lifecycle::{ModuleHealth, ModuleLifecycle};
    use crate::module::traits::AudioPlaybackBackend;

    for fail_owner in [false, true] {
        let backend = CpalAudioPlayback::new(
            // No real device can satisfy a probe, even while our fake owner runs.
            AudioConfig {
                playback_backend: AudioBackend::Alsa,
                playback_device: "hw:alia-test-missing-exclusive-output".into(),
                ..Default::default()
            },
            crate::module::volume::AudioVolumeState::new(Default::default()),
        );
        assert!(!backend.is_available());
        let (sender, receiver) = channel_with_generation(1, backend.generation.clone());
        let fail = receiver.device_failed();
        let thread = thread::spawn(move || {
            while !matches!(receiver.take(false), Some(PlaybackAction::Shutdown)) {
                receiver.wait([false; 2], true);
            }
        });
        *backend.runtime.lock().unwrap() = Some(PlaybackRuntime { sender, thread });
        assert!(backend.is_available());
        assert_eq!(backend.health_check().await, ModuleHealth::Healthy);

        if fail_owner {
            fail();
            tokio::time::timeout(Duration::from_secs(3), async {
                while !backend
                    .runtime
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .thread
                    .is_finished()
                {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
            assert!(!backend.is_available());
            assert_eq!(backend.health_check().await, ModuleHealth::Unhealthy);
        }
        backend.stop().await.unwrap();
        assert!(backend.runtime.lock().unwrap().is_none());
        assert!(!backend.is_available());
        assert_eq!(backend.health_check().await, ModuleHealth::Unhealthy);
    }
}

#[test]
fn cancel_waits_for_inflight_cpal_frame_and_preserves_stereo_alignment() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let first = identified(&sender, &receiver, 1, generation);
    sender.submit(&first, 0, &[1, 2], false).unwrap();
    sender.submit(&first, 2, &[], true).unwrap();
    let next = identified(&sender, &receiver, 2, generation);
    sender.submit(&next, 0, &[5], false).unwrap();
    sender.submit(&next, 1, &[], true).unwrap();
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    render_pending(&receiver, &mut Renderer::default(), &mut buffer);
    let in_write = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let (entered, proceed) = (in_write.clone(), release.clone());
    let worker = thread::spawn(move || {
        let mut output = Vec::new();
        buffer.write_next(|sample| {
            entered.wait();
            proceed.wait();
            output.push(sample);
        });
        for _ in 0..3 {
            output.push(buffer.next_sample());
        }
        output
    });
    in_write.wait();
    assert!(sender.cancel_submission(&first, "barrier cancellation"));
    assert!(!first.output_quiescent());
    assert_eq!(
        first.snapshot(1).observation_end,
        None,
        "terminal must await the in-flight write result"
    );
    release.wait();
    assert_eq!(worker.join().unwrap(), [1, 1, 5, 5]);
    let final_progress = first.snapshot(2);
    assert!(first.output_quiescent());
    assert_eq!(final_progress.submitted_source_frames, Some(1));
    assert!(matches!(
        final_progress.observation_end,
        Some(ObservationEnd::Cancelled { .. })
    ));
    assert_eq!(next.snapshot(1).submitted_source_frames, Some(1));
}

#[test]
fn cancel_waits_for_actual_partial_alsa_result_and_failed_write_settles_zero() {
    for actual in [0, 2] {
        let (sender, receiver) = channel(4);
        let generation = sender.begin().unwrap();
        let submission = identified(&sender, &receiver, 1, generation);
        sender.submit(&submission, 0, &[11, 12, 13], false).unwrap();
        sender.submit(&submission, 3, &[], true).unwrap();
        let mut buffer = PlaybackBuffer::new(receiver.generation());
        render_pending(&receiver, &mut Renderer::default(), &mut buffer);
        let (chunk, write) = buffer.alsa_chunk(3);
        assert_eq!(chunk.len(), 6);
        assert!(sender.cancel_submission(&submission, "writei in progress"));
        assert_eq!(submission.snapshot(1).observation_end, None);
        if actual == 0 {
            drop(write);
        } else {
            buffer.alsa_submitted(actual);
            write.unwrap().finish(actual as u64);
        }
        let final_progress = submission.snapshot(2);
        assert_eq!(final_progress.submitted_source_frames, Some(actual as u64));
        assert!(matches!(
            final_progress.observation_end,
            Some(ObservationEnd::Cancelled { .. })
        ));
        assert!(
            submission.begin_output().is_none(),
            "no new native write after cancellation"
        );
    }
}

fn reference_clock(
    rate: u32,
    channels: u16,
) -> (
    crate::module::context::AudioReferenceBus,
    crate::module::context::AudioStreamClock,
) {
    use alia_core::audio_clock::*;
    let bus = crate::module::context::AudioBus::new(2).reference();
    let clock = bus.open_stream(
        AudioDirection::Playback,
        AudioFormat {
            sample_rate_hz: rate,
            channels,
        },
    );
    clock.probe.activate();
    bus.try_claim_reader().unwrap().try_read();
    (bus, clock)
}
fn reference_data(
    bus: &crate::module::context::AudioReferenceBus,
) -> Vec<crate::module::context::AudioReferenceBlock> {
    let mut out = vec![];
    let mut reader = bus.try_claim_reader().unwrap();
    while let Some(block) = reader.try_read() {
        if block.sample_count > 0 {
            out.push(block);
        }
    }
    out
}
fn finite_block(input: Vec<i16>, rate: u32, generation: u64) -> AudioBlock {
    let len = input.len();
    AudioBlock {
        pcm: input.into(),
        range: 0..len,
        sample_rate: rate,
        generation,
        submission: test_submission(generation, rate, 1),
        source: MixerSource::Voice,
        end: true,
        reservation: None,
    }
}

#[test]
fn reference_matches_post_resample_gain_and_native_i8_quantization() {
    use super::super::cpal_backend::fill_output_buffer;
    use crate::module::context::ReferenceContent;
    use crate::module::volume::apply_gain_in_place;
    let input: Vec<i16> = (0..64)
        .map(|n| ((n as f64 * 0.2).sin() * 20000.0) as i16)
        .collect();
    let mut rendered = Renderer::default()
        .render(finite_block(input, 16000, 1), 48000, 2)
        .unwrap();
    apply_gain_in_place(&mut rendered.samples, 0.25);
    let expected: Vec<i8> = rendered
        .samples
        .iter()
        .map(|sample| (sample >> 8) as i8)
        .collect();
    assert_eq!(expected.len(), 64 * 3 * 2);
    let mut buffer = PlaybackBuffer::new(Arc::new(AtomicU64::new(1)));
    buffer.append_rendered(rendered).unwrap();
    let buffer = Mutex::new(buffer);
    let (bus, mut clock) = reference_clock(48000, 2);
    let reading = clock.probe.unknown_clock();
    let mut output = vec![0_i8; expected.len()];
    fill_output_buffer(&mut output, &buffer, &mut clock, reading);
    assert_eq!(output, expected);
    let packets = reference_data(&bus);
    let reference: Vec<f64> = packets
        .iter()
        .flat_map(|p| p.samples[..p.sample_count].iter().copied())
        .collect();
    assert_eq!(
        reference,
        output
            .iter()
            .map(|&s| f64::from(s) / 128.0)
            .collect::<Vec<_>>()
    );
    assert_eq!(packets[0].content, ReferenceContent::Media);
    assert_eq!(packets[0].timing.frames, 192);
    assert_eq!(
        packets[0].boundary,
        Some(crate::module::context::AudioSampleBoundary::CpalCallbackBuffer)
    );
}

#[test]
fn native_float_reference_preserves_the_actual_written_float_values() {
    use super::super::cpal_backend::fill_output_buffer;
    let mut buffer = PlaybackBuffer::new(Arc::new(AtomicU64::new(1)));
    buffer.append(1, vec![-32768, -12345, 0, 12345, 32767]);
    let (bus, mut clock) = reference_clock(16000, 1);
    let reading = clock.probe.unknown_clock();
    let mut actual = [0_f32; 5];
    fill_output_buffer(&mut actual, &Mutex::new(buffer), &mut clock, reading);
    let packets = reference_data(&bus);
    assert_eq!(&packets[0].samples[..5], &actual.map(f64::from));
    assert_ne!(
        packets[0].samples[1],
        -12345.0 / 32768.0,
        "reference was taken before native float conversion"
    );
}

#[test]
fn software_mute_and_callback_zero_fill_are_actual_samples_with_distinct_content() {
    use super::super::cpal_backend::fill_output_buffer;
    use crate::module::context::ReferenceContent;
    use crate::module::volume::apply_gain_in_place;
    let mut rendered = Renderer::default()
        .render(finite_block(vec![20000; 4], 16000, 1), 16000, 2)
        .unwrap();
    apply_gain_in_place(&mut rendered.samples, 0.0);
    let mut buffer = PlaybackBuffer::new(Arc::new(AtomicU64::new(1)));
    buffer.append_rendered(rendered).unwrap();
    let buffer = Mutex::new(buffer);
    let (bus, mut clock) = reference_clock(16000, 2);
    for _ in 0..2 {
        let reading = clock.probe.unknown_clock();
        let mut actual = [1_i16; 8];
        fill_output_buffer(&mut actual, &buffer, &mut clock, reading);
        assert_eq!(actual, [0; 8]);
    }
    let packets = reference_data(&bus);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].content, ReferenceContent::Media);
    assert_eq!(packets[1].content, ReferenceContent::ZeroFill);
    assert_eq!(packets[1].timing.start_frame, 4);
    assert!(
        packets
            .iter()
            .all(|p| p.samples[..p.sample_count].iter().all(|&v| v == 0.0))
    );
}

#[test]
fn callback_contention_writes_and_references_silence_without_consuming_media() {
    use super::super::cpal_backend::fill_output_buffer;
    use alia_core::audio_clock::{AudioDirection, AudioDiscontinuity};
    let mut buffer = PlaybackBuffer::new(Arc::new(AtomicU64::new(1)));
    buffer.append(1, vec![1200; 4]);
    let buffer = Mutex::new(buffer);
    let (bus, mut clock) = reference_clock(16000, 1);
    let lock = buffer.lock().unwrap();
    let reading = clock.probe.unknown_clock();
    let mut output = [1_i16; 4];
    fill_output_buffer(&mut output, &buffer, &mut clock, reading);
    assert_eq!(output, [0; 4]);
    assert_eq!(lock.len(), 4);
    drop(lock);
    let state = bus.snapshot(AudioDirection::Playback).unwrap();
    assert_eq!(
        state.last_fault,
        Some(AudioDiscontinuity::CallbackBufferUnavailable)
    );
    let reading = clock.probe.unknown_clock();
    fill_output_buffer(&mut output, &buffer, &mut clock, reading);
    assert_eq!(output, [1200; 4]);
    let packets = reference_data(&bus);
    assert_eq!(packets[0].samples[..4], [0.0; 4]);
    assert_eq!(packets[1].timing.start_frame, 4);
}

#[test]
fn unaligned_callback_never_leaves_a_partially_submitted_media_frame() {
    use super::super::cpal_backend::fill_output_buffer;
    let (sender, receiver) = channel(4);
    let generation = sender.begin().unwrap();
    let submission = identified(&sender, &receiver, 1, generation);
    sender.submit(&submission, 0, &[1, 2], false).unwrap();
    sender.submit(&submission, 2, &[], true).unwrap();
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    render_pending(&receiver, &mut Renderer::default(), &mut buffer);
    let buffer = Mutex::new(buffer);
    let (bus, mut clock) = reference_clock(16000, 2);
    let reading = clock.probe.unknown_clock();
    let mut output = [1_i16; 3];
    fill_output_buffer(&mut output, &buffer, &mut clock, reading);
    assert_eq!(output, [0; 3]);
    assert!(submission.output_quiescent());
    assert_eq!(submission.snapshot(1).submitted_source_frames, Some(0));
    let packets = reference_data(&bus);
    assert_eq!(packets[0].sample_count, 2);
}

#[test]
fn alsa_reference_contains_only_accepted_prefix_even_after_cancel_or_missing_timestamp() {
    use alia_core::audio_clock::AudioTimeQuality;
    for accepted in [0, 1, 2] {
        let (sender, receiver) = channel(4);
        let generation = sender.begin().unwrap();
        let (bus, mut clock) = reference_clock(16000, 2);
        let submission = identified_at(
            &sender,
            &receiver,
            1,
            generation,
            SubmissionBackend {
                instance_id: clock.probe.stream_id(),
                boundary: SubmissionBoundary::AlsaWriteAccepted,
                output_sample_rate_hz: 16000,
                output_channels: 2,
            },
        );
        sender
            .submit(&submission, 0, &[1100, 1200, 1300], false)
            .unwrap();
        sender.submit(&submission, 3, &[], true).unwrap();
        let mut buffer = PlaybackBuffer::new(receiver.generation());
        render_pending(&receiver, &mut Renderer::default(), &mut buffer);
        let (chunk, lease) = buffer.alsa_chunk(3);
        sender.cancel_submission(&submission, "native write is already in flight");
        assert_eq!(submission.snapshot(1).observation_end, None);
        let mut origin = super::super::clock::AlsaTimeOrigin::default();
        let reading = origin.read(&clock.probe, None, None);
        confirm_alsa_write(&mut buffer, &chunk, accepted, lease, &mut clock, reading).unwrap();
        let packets = reference_data(&bus);
        if accepted == 0 {
            assert!(packets.is_empty());
        } else {
            assert_eq!(packets.len(), 1);
            assert_eq!(packets[0].timing.frames, accepted as u64);
            assert_eq!(
                packets[0].timing.stream.stream_id,
                submission.backend.instance_id
            );
            assert_eq!(packets[0].timing.clock.quality, AudioTimeQuality::Unknown);
            assert_eq!(
                packets[0].boundary,
                Some(crate::module::context::AudioSampleBoundary::AlsaWriteAccepted)
            );
            assert_eq!(
                &packets[0].samples[..accepted * 2],
                &chunk[..accepted * 2]
                    .iter()
                    .map(|&x| f64::from(x) / 32768.0)
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(
            submission.snapshot(2).submitted_source_frames,
            Some(accepted as u64)
        );
        assert!(submission.output_quiescent());
        assert!(buffer.alsa_chunk(3).0.is_empty());
    }
}

#[test]
fn reference_blocks_are_frame_aligned_bounded_and_keep_native_clock_offsets() {
    use super::super::clock::record_samples;
    use crate::module::context::{REFERENCE_BLOCK_SAMPLES, ReferenceContent};
    use alia_core::audio_clock::*;
    let (bus, mut clock) = reference_clock(48000, 3);
    let samples = vec![1200_i16; 3600];
    let reading = AudioClockReading {
        clock_id: clock.probe.stream_id(),
        callback_ns: Some(0),
        sample_ns: Some(1000),
        queued_frames: None,
        quality: AudioTimeQuality::BackendEstimate,
    };
    record_samples(
        &mut clock,
        &samples,
        reading,
        SubmissionBoundary::AlsaWriteAccepted,
        ReferenceContent::Media,
    );
    let packets = reference_data(&bus);
    assert!(packets.len() > 1);
    let mut cursor = 0;
    for packet in packets {
        assert!(packet.sample_count <= REFERENCE_BLOCK_SAMPLES);
        assert_eq!(packet.sample_count % 3, 0);
        assert_eq!(packet.timing.start_frame, cursor);
        assert_eq!(
            packet.timing.clock.sample_ns,
            Some(1000 + (cursor * 1_000_000_000 / 48000) as i64)
        );
        cursor += packet.timing.frames;
    }
    assert_eq!(cursor, 1200);
}
