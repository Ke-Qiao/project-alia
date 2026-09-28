use super::*;
use crate::module::{
    lifecycle::ModuleLifecycle, playback_session::PlaybackSession, traits::AudioPlaybackBackend,
};
use alia_core::playback_control::*;

fn pause(
    sender: &PlaybackSender,
    submission: &Arc<AudioSubmission>,
    reason: HoldReason,
    budget_ms: u64,
) -> (Arc<PlaybackControl>, ControlCommand) {
    let owner = sender
        .playback_control(submission.identity.scope, submission.generation)
        .unwrap();
    owner.observe_capture(7, true).unwrap();
    let command = ControlCommand {
        scope: owner.scope,
        hold_id: uuid::Uuid::new_v4(),
        command_id: uuid::Uuid::new_v4(),
        sequence: 1,
        action: ControlAction::Pause {
            reason,
            through_sequence: 7,
            budget_ms,
        },
    };
    (owner, command)
}
fn resume(pause: &ControlCommand, kind: ResumeKind) -> ControlCommand {
    ControlCommand {
        command_id: uuid::Uuid::new_v4(),
        sequence: 2,
        action: ControlAction::Resume {
            authorization: ResumeAuthorization {
                kind,
                detection_id: uuid::Uuid::new_v4(),
                through_sequence: 7,
            },
        },
        ..pause.clone()
    }
}

#[test]
fn hold_settles_entered_stereo_frame_and_keeps_later_same_scope_segment() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let first = identified(&sender, &receiver, 1, generation);
    sender.submit(&first, 0, &[100, 200], false).unwrap();
    sender.submit(&first, 2, &[], true).unwrap();
    let mut renderer = Renderer::default();
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    render_pending(&receiver, &mut renderer, &mut buffer);
    assert_eq!(buffer.next_sample(), 100);
    let (owner, p) = pause(&sender, &first, HoldReason::AcousticCandidate, 3000);
    assert_eq!(
        sender.apply_control(&owner, &p).unwrap().snapshot.phase,
        ControlPhase::Pausing
    );
    assert_eq!(buffer.next_sample(), 100); // entered frame's second channel
    assert_eq!(owner.snapshot().phase, ControlPhase::Held);
    for _ in 0..10 {
        assert_eq!(buffer.next_sample(), 0);
    }
    assert_eq!(buffer.len(), 2);
    assert_eq!(first.snapshot(1).submitted_source_frames, Some(1));
    let second = identified(&sender, &receiver, 2, generation);
    sender.submit(&second, 0, &[300], false).unwrap();
    sender.submit(&second, 1, &[], true).unwrap();
    assert!(receiver.take(true).is_none());
    assert!(second.begin_output().is_none());
    assert_eq!(
        sender
            .apply_control(&owner, &resume(&p, ResumeKind::FreshBackchannel))
            .unwrap()
            .snapshot
            .phase,
        ControlPhase::Running
    );
    render_pending(&receiver, &mut renderer, &mut buffer);
    assert_eq!(
        (0..4).map(|_| buffer.next_sample()).collect::<Vec<_>>(),
        vec![200, 200, 300, 300]
    );
    assert_eq!(first.snapshot(2).submitted_source_frames, Some(2));
    assert_eq!(second.snapshot(1).submitted_source_frames, Some(1));
    assert_eq!(receiver.generation().load(Ordering::Acquire), generation);
}

#[test]
fn hold_control_bypasses_full_media_queue_and_blocks_conversion_until_resume() {
    let (sender, receiver) = channel(1);
    let generation = sender.begin().unwrap();
    let s = identified(&sender, &receiver, 1, generation);
    assert!(matches!(receiver.take(true), Some(PlaybackAction::Stop)));
    sender.submit(&s, 0, &[11; 160], false).unwrap();
    let clone = sender.clone();
    let submission = s.clone();
    let (tx, rx) = mpsc::channel();
    let producer = thread::spawn(move || {
        tx.send(clone.submit(&submission, 160, &[22; 160], false))
            .unwrap()
    });
    assert!(rx.recv_timeout(Duration::from_millis(10)).is_err());
    let (owner, p) = pause(&sender, &s, HoldReason::AcousticCandidate, 3000);
    let started = std::time::Instant::now();
    assert_eq!(
        sender.apply_control(&owner, &p).unwrap().snapshot.phase,
        ControlPhase::Held
    );
    assert!(started.elapsed() < Duration::from_millis(200));
    assert!(receiver.take(true).is_none());
    assert!(rx.recv_timeout(Duration::from_millis(10)).is_err());
    sender
        .apply_control(&owner, &resume(&p, ResumeKind::ExplicitContinue))
        .unwrap();
    assert!(matches!(receiver.take(true), Some(PlaybackAction::Play(_))));
    assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap());
    producer.join().unwrap();
}

#[test]
fn held_reference_is_actual_zero_and_resume_keeps_resample_gain_and_quantization() {
    use super::super::super::cpal_backend::fill_output_buffer;
    use crate::module::context::ReferenceContent;
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let s = identified_at(
        &sender,
        &receiver,
        1,
        generation,
        SubmissionBackend {
            instance_id: uuid::Uuid::new_v4(),
            boundary: SubmissionBoundary::CpalCallbackBuffer,
            output_sample_rate_hz: 48000,
            output_channels: 2,
        },
    );
    sender.submit(&s, 0, &vec![12345; 320], false).unwrap();
    sender.submit(&s, 320, &[], true).unwrap();
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    let mut renderer = Renderer::default();
    let mut expected = vec![];
    while let Some(action) = receiver.take(true) {
        if let PlaybackAction::Play(block) = action {
            let mut r = renderer.render(block, 48000, 2).unwrap();
            super::super::super::apply_gain_in_place(&mut r.samples, 0.5);
            expected.extend(
                r.samples
                    .iter()
                    .map(|s| <i8 as super::super::super::FromI16Sample>::from_i16_sample(*s)),
            );
            buffer.append_rendered(r).unwrap();
        }
    }
    let (owner, p) = pause(&sender, &s, HoldReason::AcousticCandidate, 3000);
    sender.apply_control(&owner, &p).unwrap();
    let retained = buffer.len();
    let buffer = Mutex::new(buffer);
    let (bus, mut clock) = reference_clock(48000, 2);
    let mut silence = [9i8; 128];
    let reading = clock.probe.unknown_clock();
    fill_output_buffer(&mut silence, &buffer, &mut clock, reading);
    assert_eq!(silence, [0; 128]);
    assert_eq!(buffer.lock().unwrap().len(), retained);
    let refs = reference_data(&bus);
    assert!(
        refs.iter()
            .all(|r| r.content == ReferenceContent::HeldSilence)
    );
    assert!(
        refs.iter()
            .flat_map(|r| r.samples[..r.sample_count].iter())
            .all(|s| *s == 0.0)
    );
    sender
        .apply_control(&owner, &resume(&p, ResumeKind::FreshBackchannel))
        .unwrap();
    let mut actual = vec![0i8; expected.len()];
    let reading = clock.probe.unknown_clock();
    fill_output_buffer(&mut actual, &buffer, &mut clock, reading);
    assert_eq!(actual, expected);
    assert_eq!(s.snapshot(1).submitted_source_frames, Some(320));
}

#[test]
fn partial_alsa_submission_is_not_replayed_after_explicit_wait() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let s = identified(&sender, &receiver, 1, generation);
    sender.submit(&s, 0, &[10, 20, 30], false).unwrap();
    sender.submit(&s, 3, &[], true).unwrap();
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    render_pending(&receiver, &mut Renderer::default(), &mut buffer);
    let (chunk, lease) = buffer.alsa_chunk(3);
    assert_eq!(chunk, vec![10, 10, 20, 20, 30, 30]);
    let (owner, p) = pause(&sender, &s, HoldReason::ExplicitWait, 3000);
    assert_eq!(
        sender.apply_control(&owner, &p).unwrap().snapshot.phase,
        ControlPhase::Pausing
    );
    let (bus, mut clock) = reference_clock(16000, 2);
    let reading = clock.probe.unknown_clock();
    confirm_alsa_write(&mut buffer, &chunk, 1, lease, &mut clock, reading).unwrap();
    assert_eq!(owner.snapshot().phase, ControlPhase::Held);
    assert!(buffer.alsa_chunk(3).0.is_empty());
    assert_eq!(
        sender
            .apply_control(&owner, &resume(&p, ResumeKind::FreshBackchannel))
            .unwrap()
            .outcome,
        CommandOutcome::Rejected(ControlRejection::ExplicitContinueRequired)
    );
    let mut continued = resume(&p, ResumeKind::ExplicitContinue);
    continued.sequence = 3;
    sender.apply_control(&owner, &continued).unwrap();
    assert_eq!(buffer.alsa_chunk(3).0, vec![20, 20, 30, 30]);
    assert_eq!(
        reference_data(&bus)
            .iter()
            .map(|r| r.timing.frames)
            .sum::<u64>(),
        1
    );
}

struct LocalBackend(PlaybackSender);
#[tonic::async_trait]
impl ModuleLifecycle for LocalBackend {
    fn name(&self) -> &'static str {
        "native-channel-test"
    }
}
impl AudioPlaybackBackend for LocalBackend {
    fn playback_control(
        &self,
        scope: &VoiceScope,
        generation: u64,
    ) -> Result<Arc<PlaybackControl>> {
        self.0.playback_control(*scope, generation)
    }
    fn apply_playback_control(
        &self,
        owner: &Arc<PlaybackControl>,
        command: &ControlCommand,
    ) -> Result<ControlReceipt> {
        self.0.apply_control(owner, command)
    }
    fn open_submission(
        &self,
        id: PlaybackIdentity,
        rate: u32,
        generation: u64,
    ) -> Result<Arc<AudioSubmission>> {
        self.0.open_submission(id, rate, generation)
    }
    fn submit_audio(&self, s: &Arc<AudioSubmission>, offset: u64, pcm: &[i16]) -> Result<bool> {
        self.0.submit(s, offset, pcm, false)
    }
    fn cancel_submission(&self, s: &Arc<AudioSubmission>, reason: &str) -> Result<bool> {
        Ok(self.0.cancel_submission(s, reason))
    }
    fn end_submission(&self, s: &Arc<AudioSubmission>, total: u64) -> Result<bool> {
        self.0.submit(s, total, &[], true)
    }
    fn begin_scoped_playback(&self) -> Result<u64> {
        self.0.begin()
    }
    fn stop_if_current(&self, generation: u64) -> Result<bool> {
        Ok(self.0.stop_if_current(generation))
    }
    fn is_playback_current(&self, generation: u64) -> bool {
        self.0.0.0.generation.load(Ordering::Acquire) == generation
    }
    fn play(&self, _: &[i16], _: u32) -> Result<()> {
        unreachable!()
    }
    fn stop_playback(&self) -> Result<()> {
        self.0.stop();
        Ok(())
    }
    fn is_available(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn local_session_dropped_waiter_keeps_native_write_and_cancel_never_revives_it() {
    let (sender, receiver) = channel(8);
    receiver.set_backend(SubmissionBackend {
        instance_id: uuid::Uuid::new_v4(),
        boundary: SubmissionBoundary::CpalCallbackBuffer,
        output_sample_rate_hz: 16000,
        output_channels: 1,
    });
    let session = PlaybackSession::new(Arc::new(LocalBackend(sender.clone())));
    let scope = VoiceScope {
        device_boot_id: uuid::Uuid::new_v4(),
        session_id: uuid::Uuid::new_v4(),
        generation_epoch: 1,
    };
    session.activate(&scope).unwrap();
    session
        .observe_capture_for_control(&scope, 7, true)
        .unwrap();
    let lease = session
        .begin(&scope, &uuid::Uuid::new_v4().to_string(), 1, 16000)
        .unwrap();
    session.enqueue(&lease, &[1, 2, 3]).unwrap();
    let write = lease.submission.begin_output().unwrap();
    let p = ControlCommand {
        scope,
        hold_id: uuid::Uuid::new_v4(),
        command_id: uuid::Uuid::new_v4(),
        sequence: 1,
        action: ControlAction::Pause {
            reason: HoldReason::AcousticCandidate,
            through_sequence: 7,
            budget_ms: 3000,
        },
    };
    assert_eq!(
        session.apply_control(&p).unwrap().snapshot.phase,
        ControlPhase::Pausing
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(10),
            session.wait_held(&scope, p.hold_id)
        )
        .await
        .is_err()
    );
    assert_eq!(
        session.observe_control(&scope).unwrap().phase,
        ControlPhase::Pausing
    );
    write.finish(1);
    assert_eq!(
        session.wait_held(&scope, p.hold_id).await.unwrap().phase,
        ControlPhase::Held
    );
    session
        .cancel_identity(&lease.identity(), "exact cancellation")
        .unwrap();
    session
        .apply_control(&resume(&p, ResumeKind::FreshBackchannel))
        .unwrap();
    assert!(lease.submission.begin_output().is_none());
    assert_eq!(
        lease.submission.snapshot(1).submitted_source_frames,
        Some(1)
    );
    session.close(&scope).unwrap();
    assert!(
        session
            .apply_control(&resume(&p, ResumeKind::FreshBackchannel))
            .is_err()
    );
    session.stop().await.unwrap();
}

#[test]
fn public_session_replays_original_receipts_after_scope_gate_or_backend_retirement() {
    for retire_backend in [false, true] {
        let (sender, receiver) = channel(8);
        receiver.set_backend(SubmissionBackend {
            instance_id: uuid::Uuid::new_v4(),
            boundary: SubmissionBoundary::CpalCallbackBuffer,
            output_sample_rate_hz: 16000,
            output_channels: 1,
        });
        let session = PlaybackSession::new(Arc::new(LocalBackend(sender.clone())));
        let scope = VoiceScope {
            device_boot_id: uuid::Uuid::new_v4(),
            session_id: uuid::Uuid::new_v4(),
            generation_epoch: 1,
        };
        session.activate(&scope).unwrap();
        session
            .observe_capture_for_control(&scope, 7, true)
            .unwrap();
        let lease = session
            .begin(&scope, &uuid::Uuid::new_v4().to_string(), 1, 16000)
            .unwrap();
        session.enqueue(&lease, &[1, 2, 3]).unwrap();
        let owner = sender
            .playback_control(scope, lease.submission.generation)
            .unwrap();
        let command = ControlCommand {
            scope,
            hold_id: uuid::Uuid::new_v4(),
            command_id: uuid::Uuid::new_v4(),
            sequence: 1,
            action: ControlAction::Pause {
                reason: HoldReason::ExplicitWait,
                through_sequence: 7,
                budget_ms: MAX_HOLD_MS,
            },
        };
        let applied = session.apply_control(&command).unwrap();
        assert_eq!(applied.outcome, CommandOutcome::Applied);
        assert_eq!(applied.snapshot.phase, ControlPhase::Held);
        let refused = resume(&command, ResumeKind::FreshBackchannel);
        let rejection = session.apply_control(&refused).unwrap();
        assert_eq!(
            rejection.outcome,
            CommandOutcome::Rejected(ControlRejection::ExplicitContinueRequired)
        );
        // Both transitions are deterministic: an actual capture gap retires
        // the gate, or the native sender retires this precise generation.
        if retire_backend {
            assert!(sender.stop_if_current(owner.generation));
        } else {
            assert!(
                session
                    .observe_capture_for_control(&scope, 9, false)
                    .is_err()
            );
        }
        let retired = session.observe_control(&scope).unwrap();
        assert_eq!(retired.phase, ControlPhase::Retired);
        let native_generation = receiver.generation().load(Ordering::Acquire);
        let retained_bytes = sender.0.0.source_bytes.load(Ordering::Acquire);
        let replay = session
            .apply_control(&command)
            .expect("the original receipt survives retirement of its native owner");
        assert_eq!(replay.outcome, applied.outcome);
        assert_eq!(replay.snapshot.phase, ControlPhase::Retired);
        assert_eq!(replay.snapshot.sequence, 2);
        assert_eq!(replay.snapshot.retired, retired.retired);
        assert_eq!(
            session.apply_control(&refused).unwrap().outcome,
            rejection.outcome
        );
        assert_eq!(
            sender.apply_control(&owner, &refused).unwrap().outcome,
            rejection.outcome
        );
        assert_eq!(
            sender.apply_control(&owner, &command).unwrap().outcome,
            applied.outcome
        );
        let mut changed = command.clone();
        changed.sequence = 2;
        assert_eq!(
            session.apply_control(&changed).unwrap().outcome,
            CommandOutcome::Rejected(ControlRejection::IdentityConflict)
        );
        assert_eq!(
            sender.apply_control(&owner, &changed).unwrap().outcome,
            CommandOutcome::Rejected(ControlRejection::IdentityConflict)
        );
        let mut unknown = resume(&command, ResumeKind::ExplicitContinue);
        unknown.sequence = 3;
        assert!(session.apply_control(&unknown).is_err());
        if retire_backend {
            assert!(sender.apply_control(&owner, &unknown).is_err());
        } else {
            assert_eq!(
                sender.apply_control(&owner, &unknown).unwrap().outcome,
                CommandOutcome::Rejected(ControlRejection::Retired)
            );
        }
        assert_eq!(
            receiver.generation().load(Ordering::Acquire),
            native_generation
        );
        assert_eq!(
            sender.0.0.source_bytes.load(Ordering::Acquire),
            retained_bytes
        );
        assert_eq!(
            lease.submission.snapshot(1).submitted_source_frames,
            Some(0)
        );
        assert!(lease.submission.try_begin_output().is_err());
        assert!(owner.is_retired());
        assert_eq!(owner.snapshot().sequence, 2);
        let state = sender.0.0.lock();
        if retire_backend {
            assert!(state.control.is_none());
        } else {
            assert!(Arc::ptr_eq(state.control.as_ref().unwrap(), &owner));
        }
        drop(state);
        // Closing removes the local scope registration; replay never creates
        // a new session or an archive of closed/replaced native owners.
        session.close(&scope).unwrap();
        assert!(session.apply_control(&command).is_err());
    }
}

struct WorkerSink {
    stops: Arc<std::sync::atomic::AtomicUsize>,
}
impl PlaybackSink for WorkerSink {
    fn buffered_ms(&self) -> u64 {
        0
    }
    fn enqueue(&mut self, _: AudioBlock) -> Result<()> {
        Ok(())
    }
    fn stop(&mut self) -> Result<()> {
        self.stops.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    fn pump(&mut self) -> Result<()> {
        Ok(())
    }
}
#[test]
fn owned_worker_expires_hold_without_a_waiter_and_rejects_late_same_scope_start() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let s = identified(&sender, &receiver, 1, generation);
    sender.submit(&s, 0, &[1; 160], false).unwrap();
    sender.submit(&s, 160, &[], true).unwrap();
    let (owner, p) = pause(&sender, &s, HoldReason::AcousticCandidate, 30);
    sender.apply_control(&owner, &p).unwrap();
    let stops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = stops.clone();
    let task = thread::spawn(move || run(receiver, &mut WorkerSink { stops: observed }, 50));
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while stops.load(Ordering::Acquire) < 2 && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(owner.snapshot().retired, Some(RetiredReason::Expired));
    assert!(stops.load(Ordering::Acquire) >= 2);
    assert!(!s.is_live());
    let id = PlaybackIdentity {
        playback_id: uuid::Uuid::new_v4(),
        playback_sequence: 2,
        ..s.identity
    };
    assert!(sender.open_submission(id, 16000, generation).is_err());
    assert_eq!(
        sender
            .apply_control(&owner, &resume(&p, ResumeKind::FreshBackchannel))
            .unwrap()
            .outcome,
        CommandOutcome::Rejected(ControlRejection::Retired)
    );
    sender.shutdown();
    task.join().unwrap().unwrap();
}

#[test]
fn held_atomic_rejection_survives_resume_between_admission_and_buffer_handling() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let s = identified(&sender, &receiver, 1, generation);
    sender.submit(&s, 0, &[17, 23], false).unwrap();
    sender.submit(&s, 2, &[], true).unwrap();
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    render_pending(&receiver, &mut Renderer::default(), &mut buffer);
    let (owner, p) = pause(&sender, &s, HoldReason::AcousticCandidate, 3000);
    sender.apply_control(&owner, &p).unwrap();
    let rejected = Arc::new(std::sync::Barrier::new(2));
    let resumed = Arc::new(std::sync::Barrier::new(2));
    let (a, b) = (rejected.clone(), resumed.clone());
    let native = thread::spawn(move || {
        let mut output = vec![];
        let result = buffer.write_next_with(
            |v| output.push(v),
            |submission| {
                let decision = submission.try_begin_output();
                assert!(matches!(decision, Err(OutputBlocked::Held)));
                a.wait();
                b.wait();
                decision
            },
        );
        assert_eq!(result, SampleContent::Held);
        // Resume must not start left-channel media in the right-channel slot.
        for _ in 0..5 {
            output.push(buffer.next_sample());
        }
        output
    });
    rejected.wait();
    sender
        .apply_control(&owner, &resume(&p, ResumeKind::FreshBackchannel))
        .unwrap();
    resumed.wait();
    assert_eq!(native.join().unwrap(), vec![0, 0, 17, 17, 23, 23]);
    assert_eq!(s.snapshot(1).submitted_source_frames, Some(2));
}

#[test]
fn confirming_wait_does_not_release_output_or_admit_backchannel() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let s = identified(&sender, &receiver, 1, generation);
    sender.submit(&s, 0, &[1; 160], false).unwrap();
    let (owner, p) = pause(&sender, &s, HoldReason::AcousticCandidate, 3000);
    sender.apply_control(&owner, &p).unwrap();
    let c = ControlCommand {
        command_id: uuid::Uuid::new_v4(),
        sequence: 2,
        action: ControlAction::ConfirmExplicitWait {
            detection_id: uuid::Uuid::new_v4(),
            through_sequence: 7,
        },
        ..p.clone()
    };
    let receipt = sender.apply_control(&owner, &c).unwrap();
    assert_eq!(receipt.snapshot.reason, Some(HoldReason::ExplicitWait));
    assert!(s.try_begin_output().is_err());
    let mut r = resume(&p, ResumeKind::FreshBackchannel);
    r.sequence = 3;
    assert_eq!(
        sender.apply_control(&owner, &r).unwrap().outcome,
        CommandOutcome::Rejected(ControlRejection::ExplicitContinueRequired)
    );
    let mut r = resume(&p, ResumeKind::ExplicitContinue);
    r.sequence = 4;
    assert_eq!(
        sender.apply_control(&owner, &r).unwrap().snapshot.phase,
        ControlPhase::Running
    );
}

#[test]
fn retained_source_and_rendered_bytes_are_bounded_even_when_count_capacity_is_large() {
    let (sender, receiver) = channel(100);
    let generation = sender.begin().unwrap();
    let s = identified(&sender, &receiver, 1, generation);
    assert!(
        sender
            .submit(&s, 0, &vec![0; MAX_PAYLOAD_BYTES / 2 + 1], false)
            .is_err()
    );
    assert_eq!(s.accepted(), 0);
    let chunk = vec![0; MAX_PAYLOAD_BYTES / 2];
    sender.submit(&s, 0, &chunk, false).unwrap();
    sender
        .submit(&s, chunk.len() as u64, &chunk, false)
        .unwrap();
    assert_eq!(
        sender.0.0.source_bytes.load(Ordering::Acquire),
        MAX_SOURCE_BYTES
    );
    let clone = sender.clone();
    let submission = s.clone();
    let (tx, rx) = mpsc::channel();
    let producer = thread::spawn(move || {
        tx.send(clone.submit(&submission, (chunk.len() * 2) as u64, &[1], false))
            .unwrap()
    });
    assert!(rx.recv_timeout(Duration::from_millis(10)).is_err());
    sender.cancel_submission(&s, "release retained byte budget");
    assert!(!rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap());
    producer.join().unwrap();
    assert_eq!(sender.0.0.source_bytes.load(Ordering::Acquire), 0);
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    assert!(buffer.append(generation, vec![1; MAX_RENDERED_BYTES / 2]));
    assert!(
        buffer
            .append_rendered(RenderedBlock {
                samples: vec![1],
                cursor: 0,
                channels: 1,
                generation,
                submission: None,
                output_write: None
            })
            .is_err()
    );
}

#[test]
fn hold_wakes_an_already_idle_worker_and_expires_with_no_media_or_waiter() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let s = identified(&sender, &receiver, 1, generation);
    assert!(matches!(receiver.take(false), Some(PlaybackAction::Stop)));
    let (owner, p) = pause(&sender, &s, HoldReason::AcousticCandidate, 30);
    let (entered_tx, entered_rx) = mpsc::channel();
    let stops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = stops.clone();
    let task = thread::spawn(move || {
        // This signal is sent while the real wait owns the media mutex. Pause
        // cannot enter until condvar waiting atomically releases that mutex.
        receiver.wait_before(true, true, || entered_tx.send(()).unwrap());
        run(receiver, &mut WorkerSink { stops: observed }, 50)
    });
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    sender.apply_control(&owner, &p).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while stops.load(Ordering::Acquire) == 0 && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(owner.snapshot().retired, Some(RetiredReason::Expired));
    assert!(stops.load(Ordering::Acquire) > 0);
    sender.shutdown();
    task.join().unwrap().unwrap();
}

#[test]
fn initiative_capture_before_registration_rejects_without_allocating_a_native_owner() {
    use crate::module::playback_session::CaptureControlBarrier;
    use alia_core::output_control::{InitiativeCapture, OutputBinding};
    for before_fence in [true, false] {
        let (sender, receiver) = channel(8);
        receiver.set_backend(SubmissionBackend {
            instance_id: uuid::Uuid::new_v4(),
            boundary: SubmissionBoundary::CpalCallbackBuffer,
            output_sample_rate_hz: 16000,
            output_channels: 2,
        });
        let session = Arc::new(PlaybackSession::new(Arc::new(LocalBackend(sender))));
        let original = VoiceScope {
            device_boot_id: uuid::Uuid::new_v4(),
            session_id: uuid::Uuid::new_v4(),
            generation_epoch: 1,
        };
        let scope = VoiceScope {
            generation_epoch: 2,
            ..original
        };
        let capture = CaptureControlBarrier::default();
        session
            .activate_with_capture(&original, capture.clone(), 3000)
            .unwrap();
        session
            .accept_capture(&capture, 0, false, || Ok(()))
            .unwrap();
        if before_fence {
            session
                .accept_capture(&capture, 1, true, || Ok(()))
                .unwrap();
        }
        session.fence(&scope, false).unwrap();
        let generation = receiver.generation().load(Ordering::Acquire);
        let binding = OutputBinding {
            scope,
            response_id: uuid::Uuid::new_v4(),
            context_id: uuid::Uuid::new_v4(),
            server_instance_id: uuid::Uuid::new_v4(),
            initiative_capture: Some(InitiativeCapture {
                initiative_id: uuid::Uuid::new_v4(),
                request_id: uuid::Uuid::new_v4(),
                through_sequence: 0,
            }),
        };
        if before_fence {
            assert!(session.register_output(&binding).is_err());
        } else {
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let owner = session.clone();
            let captured = capture.clone();
            let capturing = thread::spawn(move || {
                owner.accept_capture(&captured, 1, true, || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                })
            });
            entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            let owner = session.clone();
            let (attempt_tx, attempt_rx) = mpsc::channel();
            let registering = thread::spawn(move || {
                attempt_tx.send(()).unwrap();
                owner.register_output(&binding)
            });
            attempt_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            release_tx.send(()).unwrap();
            capturing.join().unwrap().unwrap();
            assert!(registering.join().unwrap().is_err());
        }
        assert_eq!(receiver.generation().load(Ordering::Acquire), generation);
        assert!(
            session
                .begin_bound(&binding, &uuid::Uuid::new_v4().to_string(), 1, 16000)
                .is_err()
        );
        session.close(&scope).unwrap();
    }
}

#[test]
fn initiative_register_then_speech_before_start_retains_original_media_for_cpal_and_alsa() {
    use crate::module::playback_session::CaptureControlBarrier;
    use alia_core::output_control::*;
    for alsa in [false, true] {
        let (sender, receiver) = channel(8);
        receiver.set_backend(SubmissionBackend {
            instance_id: uuid::Uuid::new_v4(),
            boundary: if alsa {
                SubmissionBoundary::AlsaWriteAccepted
            } else {
                SubmissionBoundary::CpalCallbackBuffer
            },
            output_sample_rate_hz: 16000,
            output_channels: 2,
        });
        let session = Arc::new(PlaybackSession::new(Arc::new(LocalBackend(sender.clone()))));
        let original = VoiceScope {
            device_boot_id: uuid::Uuid::new_v4(),
            session_id: uuid::Uuid::new_v4(),
            generation_epoch: 1,
        };
        let scope = VoiceScope {
            generation_epoch: 2,
            ..original
        };
        let capture = CaptureControlBarrier::default();
        session
            .activate_with_capture(&original, capture.clone(), 3000)
            .unwrap();
        session
            .accept_capture(&capture, 0, false, || Ok(()))
            .unwrap();
        session.fence(&scope, false).unwrap();
        let binding = OutputBinding {
            scope,
            response_id: uuid::Uuid::new_v4(),
            context_id: uuid::Uuid::new_v4(),
            server_instance_id: uuid::Uuid::new_v4(),
            initiative_capture: Some(InitiativeCapture {
                initiative_id: uuid::Uuid::new_v4(),
                request_id: uuid::Uuid::new_v4(),
                through_sequence: 0,
            }),
        };
        let registered = session.register_output(&binding).unwrap();
        let generation = receiver.generation().load(Ordering::Acquire);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let owner = session.clone();
        let captured = capture.clone();
        let capturing = thread::spawn(move || {
            owner.accept_capture(&captured, 1, true, || {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let owner = session.clone();
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let starting = thread::spawn(move || {
            attempt_tx.send(()).unwrap();
            owner.begin_bound(&binding, &uuid::Uuid::new_v4().to_string(), 1, 16000)
        });
        attempt_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        release_tx.send(()).unwrap();
        capturing.join().unwrap().unwrap();
        let lease = starting.join().unwrap().unwrap();
        let local = session.observe_control(&scope).unwrap();
        assert_eq!(local.phase, ControlPhase::Held);
        assert_eq!(local.hold_id, None);
        // Late event, identical Fence and Register ACK cannot reopen local speech.
        session
            .admit_initiative_event(
                &scope,
                binding.initiative_capture.unwrap(),
                binding.context_id,
            )
            .unwrap();
        session.fence(&scope, false).unwrap();
        assert_eq!(session.register_output(&binding).unwrap(), registered);
        assert_eq!(
            session.observe_control(&scope).unwrap().phase,
            ControlPhase::Held
        );
        session.enqueue(&lease, &[10, 20, 30]).unwrap();
        session.end(&lease).unwrap();
        assert!(lease.submission.try_begin_output().is_err());
        assert!(matches!(receiver.take(true), Some(PlaybackAction::Stop)));
        assert!(receiver.take(true).is_none());
        let mut p = BoundControlCommand {
            binding,
            expected_backend: registered.backend.clone(),
            command: ControlCommand {
                scope,
                hold_id: uuid::Uuid::new_v4(),
                command_id: uuid::Uuid::new_v4(),
                sequence: 1,
                action: ControlAction::Pause {
                    reason: HoldReason::AcousticCandidate,
                    through_sequence: 1,
                    budget_ms: 3000,
                },
            },
        };
        assert_eq!(
            session.apply_bound_control(&p).unwrap().receipt.outcome,
            CommandOutcome::Applied
        );
        let mut r = p.clone();
        r.command.command_id = uuid::Uuid::new_v4();
        r.command.sequence = 2;
        r.command.action = ControlAction::Resume {
            authorization: ResumeAuthorization {
                kind: ResumeKind::FreshBackchannel,
                detection_id: uuid::Uuid::new_v4(),
                through_sequence: 1,
            },
        };
        assert_eq!(
            session
                .apply_bound_control(&r)
                .unwrap()
                .receipt
                .snapshot
                .phase,
            ControlPhase::Running
        );
        let mut renderer = Renderer::default();
        let mut buffer = PlaybackBuffer::new(receiver.generation());
        render_pending(&receiver, &mut renderer, &mut buffer);
        // Keep one real native write entered across the next local speech.
        let (chunk, write) = if alsa {
            buffer.alsa_chunk(3)
        } else {
            (vec![], None)
        };
        if !alsa {
            assert_eq!(buffer.next_sample(), 10);
        }
        session
            .accept_capture(&capture, 2, true, || Ok(()))
            .unwrap();
        assert_eq!(
            session.observe_control(&scope).unwrap().phase,
            ControlPhase::Pausing
        );
        if alsa {
            let (_, mut clock) = reference_clock(16000, 2);
            let reading = clock.probe.unknown_clock();
            confirm_alsa_write(&mut buffer, &chunk, 1, write, &mut clock, reading).unwrap();
        } else {
            assert_eq!(buffer.next_sample(), 10);
        }
        let late = session.observe_bound_control(&r).unwrap();
        assert_eq!(late.receipt.outcome, CommandOutcome::Applied);
        assert_eq!(late.receipt.snapshot.phase, ControlPhase::Held);
        assert_eq!(late.receipt.snapshot.hold_id, None);
        assert_eq!(
            late.receipt
                .snapshot
                .local_capture_hold
                .unwrap()
                .through_sequence,
            2
        );
        if alsa {
            assert!(buffer.alsa_chunk(2).0.is_empty());
        } else {
            assert_eq!(buffer.next_sample(), 0);
            assert_eq!(buffer.next_sample(), 0);
        }
        p.command.hold_id = uuid::Uuid::new_v4();
        p.command.command_id = uuid::Uuid::new_v4();
        p.command.sequence = 3;
        p.command.action = ControlAction::Pause {
            reason: HoldReason::ExplicitWait,
            through_sequence: 2,
            budget_ms: 2000,
        };
        session.apply_bound_control(&p).unwrap();
        let mut continued = p.clone();
        continued.command.command_id = uuid::Uuid::new_v4();
        continued.command.sequence = 4;
        continued.command.action = ControlAction::Resume {
            authorization: ResumeAuthorization {
                kind: ResumeKind::ExplicitContinue,
                detection_id: uuid::Uuid::new_v4(),
                through_sequence: 2,
            },
        };
        assert_eq!(
            session
                .apply_bound_control(&continued)
                .unwrap()
                .receipt
                .snapshot
                .phase,
            ControlPhase::Running
        );
        if alsa {
            assert_eq!(buffer.alsa_chunk(2).0, vec![20, 20, 30, 30]);
        } else {
            assert_eq!(
                (0..4).map(|_| buffer.next_sample()).collect::<Vec<_>>(),
                vec![20, 20, 30, 30]
            );
        }
        assert_eq!(receiver.generation().load(Ordering::Acquire), generation);
        assert_eq!(lease.identity().scope, scope);
        session.close(&scope).unwrap();
    }
}
