use super::super::super::cpal_backend::fill_output_buffer;
use super::*;
use alia_core::playback_control::*;

fn cue(sender: &PlaybackSender, voice: &AudioSubmission, rate: u32) -> Arc<AudioSubmission> {
    sender
        .open_cue_submission(
            PlaybackIdentity {
                scope: voice.identity.scope,
                playback_id: uuid::Uuid::new_v4(),
                playback_sequence: 1,
            },
            rate,
            voice.generation,
        )
        .unwrap()
}

fn submit(sender: &PlaybackSender, owner: &Arc<AudioSubmission>, pcm: &[i16]) {
    assert!(sender.submit(owner, 0, pcm, false).unwrap());
    assert!(sender.submit(owner, pcm.len() as u64, &[], true).unwrap());
}

struct NativeWorkerSink<'a> {
    sender: &'a PlaybackSender,
    submissions: [Arc<AudioSubmission>; 2],
    buffer: PlaybackBuffer,
    renderer: Renderer,
    clock: crate::module::context::AudioStreamClock,
    output: Vec<i16>,
    enqueued: Vec<MixerSource>,
    pumps: usize,
    stop_after_first_enqueue: bool,
}

impl PlaybackSink for NativeWorkerSink<'_> {
    fn buffered_ms(&self) -> u64 {
        self.buffer.len() as u64 / 32
    }
    fn source_buffered_ms(&self, source: MixerSource) -> u64 {
        self.buffer.source_len(source) as u64 / 32
    }
    fn enqueue(&mut self, block: AudioBlock) -> Result<()> {
        self.enqueued.push(block.source);
        self.buffer
            .append_rendered(self.renderer.render(block, 16000, 2)?)?;
        if self.stop_after_first_enqueue && self.enqueued.len() == 1 {
            self.sender.stop();
        }
        Ok(())
    }
    fn stop(&mut self) -> Result<()> {
        self.buffer.clear();
        self.renderer.clear();
        if self.stop_after_first_enqueue && !self.enqueued.is_empty() {
            self.sender.shutdown();
        }
        Ok(())
    }
    fn pump(&mut self) -> Result<()> {
        self.pumps += 1;
        ensure!(self.pumps <= 10, "worker did not finish bounded fixture");
        let (chunk, lease) = self.buffer.alsa_chunk(320);
        if !chunk.is_empty() {
            let reading = self.clock.probe.unknown_clock();
            confirm_alsa_write(
                &mut self.buffer,
                &chunk,
                chunk.len() / 2,
                lease,
                &mut self.clock,
                reading,
            )?;
            self.output.extend(chunk);
        }
        if self
            .submissions
            .iter()
            .all(|source| source.snapshot(1).observation_end.is_some())
        {
            self.sender.shutdown();
        }
        Ok(())
    }
}

#[test]
fn worker_mixes_ready_lanes_before_native_write_and_prioritizes_stop_between_them() {
    for stop_after_first_enqueue in [false, true] {
        let (sender, receiver) = channel(8);
        let generation = sender.begin().unwrap();
        let voice = identified(&sender, &receiver, 1, generation);
        let cue = cue(&sender, &voice, 16000);
        submit(&sender, &voice, &[10000; 640]);
        submit(&sender, &cue, &[5000; 640]);
        let (bus, clock) = reference_clock(16000, 2);
        let mut sink = NativeWorkerSink {
            sender: &sender,
            submissions: [voice.clone(), cue.clone()],
            buffer: PlaybackBuffer::new(receiver.generation()),
            renderer: Renderer::default(),
            clock,
            output: Vec::new(),
            enqueued: Vec::new(),
            pumps: 0,
            stop_after_first_enqueue,
        };
        run(receiver, &mut sink, 300).unwrap();
        let reference: Vec<_> = reference_data(&bus)
            .into_iter()
            .flat_map(|block| block.samples[..block.sample_count].to_vec())
            .collect();
        if stop_after_first_enqueue {
            assert_eq!(sink.enqueued, [MixerSource::Voice]);
            assert_eq!(sink.pumps, 0);
            assert!(sink.output.is_empty());
            assert!(reference.is_empty());
            for source in [&voice, &cue] {
                assert_eq!(source.snapshot(1).submitted_source_frames, Some(0));
            }
        } else {
            assert_eq!(sink.enqueued, MixerSource::ALL.repeat(2));
            assert_eq!(sink.pumps, 2);
            assert_eq!(sink.output, vec![11000; 1280]);
            assert_eq!(reference, vec![11000.0 / 32768.0; 1280]);
            for source in [&voice, &cue] {
                let progress = source.snapshot(1);
                assert_eq!(progress.submitted_source_frames, Some(640));
                assert_eq!(
                    progress.observation_end,
                    Some(ObservationEnd::SubmissionFinished)
                );
            }
        }
    }
}

#[test]
fn cpal_mixes_voice_and_quiet_cue_limits_peaks_and_references_final_samples() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let voice = identified(&sender, &receiver, 1, generation);
    let cue = cue(&sender, &voice, 16000);
    assert!(Arc::ptr_eq(
        voice.control().unwrap(),
        cue.control().unwrap()
    ));
    submit(&sender, &voice, &[20000, 30000, -30000, 1000]);
    submit(&sender, &cue, &[10000, 20000, -20000, 5000]);
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    render_pending(&receiver, &mut Renderer::default(), &mut buffer);
    let (bus, mut clock) = reference_clock(16000, 2);
    let reading = clock.probe.unknown_clock();
    let mut output = [0_i16; 8];
    fill_output_buffer(&mut output, &Mutex::new(buffer), &mut clock, reading);
    assert_eq!(
        output,
        [22000, 22000, 32767, 32767, -32768, -32768, 2000, 2000]
    );
    let recorded: Vec<_> = reference_data(&bus)
        .into_iter()
        .flat_map(|block| block.samples[..block.sample_count].to_vec())
        .collect();
    assert_eq!(recorded, output.map(|s| f64::from(s) / 32768.0));
    for source in [&voice, &cue] {
        let progress = source.snapshot(1);
        assert_eq!(progress.submitted_source_frames, Some(4));
        assert_eq!(
            progress.observation_end,
            Some(ObservationEnd::SubmissionFinished)
        );
        assert_eq!(progress.presented_source_frames, None);
    }
}

#[test]
fn cancelling_one_mixed_source_preserves_the_other_and_complete_stereo_frame() {
    for cancel_voice in [false, true] {
        let (sender, receiver) = channel(8);
        let generation = sender.begin().unwrap();
        let voice = identified(&sender, &receiver, 1, generation);
        let cue = cue(&sender, &voice, 16000);
        submit(&sender, &voice, &[10000, 20000]);
        submit(&sender, &cue, &[5000, 5000]);
        let mut buffer = PlaybackBuffer::new(receiver.generation());
        render_pending(&receiver, &mut Renderer::default(), &mut buffer);
        let cancelled = if cancel_voice { &voice } else { &cue };
        buffer.write_next(|sample| {
            assert_eq!(sample, 11000);
            assert!(sender.cancel_submission(cancelled, "cancel one source inside callback"));
            assert!(!voice.output_quiescent());
            assert!(!cue.output_quiescent());
            assert_eq!(cancelled.snapshot(1).observation_end, None);
        });
        assert_eq!(buffer.next_sample(), 11000);
        let expected = if cancel_voice { 1000 } else { 20000 };
        assert_eq!(buffer.next_sample(), expected);
        assert_eq!(buffer.next_sample(), expected);
        assert_eq!(buffer.next_sample(), 0);
        assert_eq!(cancelled.snapshot(2).submitted_source_frames, Some(1));
        assert!(matches!(
            cancelled.snapshot(3).observation_end,
            Some(ObservationEnd::Cancelled { .. })
        ));
        let survivor = if cancel_voice { &cue } else { &voice };
        assert_eq!(survivor.snapshot(1).submitted_source_frames, Some(2));
    }
}

#[test]
fn explicit_hold_then_immediate_resume_never_replays_cue_but_retains_voice_tail() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let voice = identified(&sender, &receiver, 1, generation);
    let cue = cue(&sender, &voice, 16000);
    submit(&sender, &voice, &[10000, 20000]);
    submit(&sender, &cue, &[5000, 5000]);
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    render_pending(&receiver, &mut Renderer::default(), &mut buffer);
    let owner = voice.control().unwrap();
    owner.observe_capture(0, true).unwrap();
    let pause = ControlCommand {
        scope: voice.identity.scope,
        hold_id: uuid::Uuid::new_v4(),
        command_id: uuid::Uuid::new_v4(),
        sequence: 1,
        action: ControlAction::Pause {
            reason: HoldReason::ExplicitWait,
            through_sequence: 0,
            budget_ms: 3000,
        },
    };
    assert_eq!(
        sender.apply_control(owner, &pause).unwrap().snapshot.phase,
        ControlPhase::Held
    );
    assert_eq!(buffer.next_sample(), 0);
    assert_eq!(buffer.next_sample(), 0);
    assert_eq!(voice.snapshot(1).submitted_source_frames, Some(0));
    assert_eq!(cue.snapshot(1).submitted_source_frames, Some(0));
    let resume = ControlCommand {
        command_id: uuid::Uuid::new_v4(),
        sequence: 2,
        action: ControlAction::Resume {
            authorization: ResumeAuthorization {
                kind: ResumeKind::ExplicitContinue,
                detection_id: uuid::Uuid::new_v4(),
                through_sequence: 0,
            },
        },
        ..pause
    };
    assert_eq!(
        sender.apply_control(owner, &resume).unwrap().snapshot.phase,
        ControlPhase::Running
    );
    assert_eq!(
        (0..4).map(|_| buffer.next_sample()).collect::<Vec<_>>(),
        [10000, 10000, 20000, 20000]
    );
    assert!(matches!(
        cue.snapshot(2).observation_end,
        Some(ObservationEnd::Cancelled { .. })
    ));
    assert_eq!(
        voice.snapshot(2).observation_end,
        Some(ObservationEnd::SubmissionFinished)
    );
}

#[test]
fn mixed_alsa_short_write_settles_both_original_prefixes_and_references_only_accepted_mix() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let voice = identified(&sender, &receiver, 1, generation);
    let cue = cue(&sender, &voice, 16000);
    submit(&sender, &voice, &[10000, 20000, 30000]);
    submit(&sender, &cue, &[5000, -5000, 15000]);
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    render_pending(&receiver, &mut Renderer::default(), &mut buffer);
    let (chunk, writes) = buffer.alsa_chunk(3);
    assert_eq!(chunk, [11000, 11000, 19000, 19000, 32767, 32767]);
    drop(writes); // EAGAIN / failure is not submission for either source.
    assert_eq!(voice.snapshot(1).submitted_source_frames, Some(0));
    assert_eq!(cue.snapshot(1).submitted_source_frames, Some(0));
    let (chunk, writes) = buffer.alsa_chunk(3);
    assert!(sender.cancel_submission(&cue, "cancel during accepted native write"));
    assert_eq!(cue.snapshot(2).observation_end, None);
    let (bus, mut clock) = reference_clock(16000, 2);
    let reading = clock.probe.unknown_clock();
    confirm_alsa_write(&mut buffer, &chunk, 1, writes, &mut clock, reading).unwrap();
    assert_eq!(voice.snapshot(2).submitted_source_frames, Some(1));
    assert_eq!(cue.snapshot(3).submitted_source_frames, Some(1));
    assert_eq!(buffer.alsa_chunk(3).0, [20000, 20000, 30000, 30000]);
    let reference = reference_data(&bus);
    assert_eq!(reference.len(), 1);
    assert_eq!(reference[0].timing.frames, 1);
    assert_eq!(&reference[0].samples[..2], &[11000.0 / 32768.0; 2]);
}

#[test]
fn per_source_resampler_history_survives_interleaved_worker_blocks() {
    let (sender, receiver) = channel(16);
    let generation = sender.begin().unwrap();
    let voice = identified_at(
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
    let cue = cue(&sender, &voice, 11025);
    let voice_pcm: Vec<_> = (0..3200)
        .map(|n| ((n as f64 * 0.17).sin() * 12000.0) as i16)
        .collect();
    let cue_pcm: Vec<_> = (0..1103)
        .map(|n| ((n as f64 * 0.11).sin() * 8000.0) as i16)
        .collect();
    let expected_voice = Renderer::default()
        .render(finite_block(voice_pcm.clone(), 16000, generation), 48000, 2)
        .unwrap()
        .samples;
    let expected_cue = Renderer::default()
        .render(finite_block(cue_pcm.clone(), 11025, generation), 48000, 2)
        .unwrap()
        .samples;
    submit(&sender, &voice, &voice_pcm);
    submit(&sender, &cue, &cue_pcm);
    let mut renderer = Renderer::default();
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    while let Some(action) = receiver.take(true) {
        match action {
            PlaybackAction::Stop => {
                buffer.clear();
                renderer.clear();
            }
            PlaybackAction::Play(block) => {
                buffer
                    .append_rendered(renderer.render(block, 48000, 2).unwrap())
                    .unwrap();
            }
            PlaybackAction::Shutdown => break,
        }
    }
    let expected: Vec<_> = expected_voice
        .iter()
        .enumerate()
        .map(|(i, voice)| mix_sample(*voice, expected_cue.get(i).copied().unwrap_or(0)))
        .collect();
    assert_eq!(
        (0..expected.len())
            .map(|_| buffer.next_sample())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        voice.snapshot(1).observation_end,
        Some(ObservationEnd::SubmissionFinished)
    );
    assert_eq!(
        cue.snapshot(1).observation_end,
        Some(ObservationEnd::SubmissionFinished)
    );
}

#[test]
fn mixed_alsa_chunk_stops_at_lane_boundary_without_inserting_silence_before_next_block() {
    let (sender, receiver) = channel(8);
    let generation = sender.begin().unwrap();
    let voice = identified(&sender, &receiver, 1, generation);
    let cue = cue(&sender, &voice, 16000);
    sender.submit(&voice, 0, &[10000], false).unwrap();
    sender.submit(&voice, 1, &[20000, 30000], false).unwrap();
    sender.submit(&voice, 3, &[], true).unwrap();
    submit(&sender, &cue, &[5000, 5000, 5000]);
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    render_pending(&receiver, &mut Renderer::default(), &mut buffer);
    let (chunk, writes) = buffer.alsa_chunk(3);
    assert_eq!(chunk, [11000, 11000]);
    buffer.alsa_submitted(1);
    writes.unwrap().finish(1);
    let (chunk, writes) = buffer.alsa_chunk(3);
    assert_eq!(chunk, [21000, 21000, 31000, 31000]);
    buffer.alsa_submitted(2);
    writes.unwrap().finish(2);
    assert_eq!(voice.snapshot(1).submitted_source_frames, Some(3));
    assert_eq!(cue.snapshot(1).submitted_source_frames, Some(3));
}
