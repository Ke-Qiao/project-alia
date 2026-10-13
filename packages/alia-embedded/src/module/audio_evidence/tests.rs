use super::*;
use crate::module::context::{AudioBus, AudioStreamClock};
use alia_core::audio_clock::*;

fn format() -> AudioFormat {
    AudioFormat {
        sample_rate_hz: 48000,
        channels: 2,
    }
}

fn publish(clock: &mut AudioStreamClock, samples: &[f64], content: ReferenceContent) {
    let timing = clock.observe((samples.len() / 2) as u64, clock.probe.unknown_clock());
    let mut native = [0.0; REFERENCE_BLOCK_SAMPLES];
    native[..samples.len()].copy_from_slice(samples);
    clock.probe.publish(AudioReferenceBlock {
        timing,
        boundary: Some(match timing.stream.direction {
            AudioDirection::Capture => AudioSampleBoundary::CpalInputCallback,
            AudioDirection::Playback => AudioSampleBoundary::CpalCallbackBuffer,
        }),
        sample_format: Some(NativeSampleFormat::F64),
        content,
        discontinuity: None,
        sequence: 0,
        lost_blocks_before: 0,
        lost_frames_before: 0,
        samples: native,
        sample_count: samples.len(),
    });
}

fn fixture() -> AudioEvidence {
    let bus = AudioBus::new(2).reference();
    let mut session =
        EvidenceSession::start(&bus, EvidenceLimits::default(), CancellationToken::new()).unwrap();
    let mut capture = bus.open_stream(AudioDirection::Capture, format());
    let mut playback = bus.open_stream(AudioDirection::Playback, format());
    publish(
        &mut capture,
        &[0.25, -0.25, 0.0, -0.0],
        ReferenceContent::Media,
    );
    publish(
        &mut playback,
        &[0.0, -0.0, f64::MIN_POSITIVE, -1.0],
        ReferenceContent::MixedHeld,
    );
    capture.probe.close();
    playback.probe.close();
    session.pump(MAX_RECORDS).unwrap();
    let mut evidence = session.finish().unwrap();
    evidence.source = EvidenceSource::SyntheticFixture;
    evidence
}

fn encoded(evidence: &AudioEvidence) -> Vec<u8> {
    let mut bytes = Vec::new();
    evidence
        .write_json(
            &mut bytes,
            EvidenceLimits::default(),
            &CancellationToken::new(),
        )
        .unwrap();
    bytes
}

#[test]
fn native_dual_stream_round_trip_preserves_bits_clocks_boundaries_and_markers() {
    let original = fixture();
    let bytes = encoded(&original);
    let decoded = AudioEvidence::read_json(
        bytes.as_slice(),
        EvidenceLimits::default(),
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&decoded).unwrap(),
        serde_json::to_value(&original).unwrap()
    );
    let capture = decoded
        .records
        .iter()
        .find(|r| r.timing.frames > 0 && r.timing.stream.direction == AudioDirection::Capture)
        .unwrap();
    let playback = decoded
        .records
        .iter()
        .find(|r| r.timing.frames > 0 && r.timing.stream.direction == AudioDirection::Playback)
        .unwrap();
    assert_ne!(
        capture.timing.clock.clock_id,
        playback.timing.clock.clock_id
    );
    assert_eq!(capture.timing.clock.quality, AudioTimeQuality::Unknown);
    assert_eq!(capture.timing.clock.queued_frames, None);
    assert_eq!(
        playback.samples_f64_bits,
        [
            0.0f64.to_bits(),
            (-0.0f64).to_bits(),
            f64::MIN_POSITIVE.to_bits(),
            (-1.0f64).to_bits()
        ]
    );
    assert_eq!(playback.content, ReferenceContent::MixedHeld);
    assert_eq!(
        decoded
            .records
            .iter()
            .filter(|r| r.discontinuity == Some(AudioDiscontinuity::StreamStopped))
            .count(),
        2
    );
}

#[test]
fn observer_cancel_finish_and_drop_release_claim_without_closing_native_owner() {
    let bus = AudioBus::new(2).reference();
    let mut clock = bus.open_stream(AudioDirection::Capture, format());
    publish(&mut clock, &[0.25, 0.5], ReferenceContent::Media);
    let cancel = CancellationToken::new();
    let mut session =
        EvidenceSession::start(&bus, EvidenceLimits::default(), cancel.clone()).unwrap();
    assert!(
        EvidenceSession::start(&bus, EvidenceLimits::default(), CancellationToken::new()).is_err()
    );
    cancel.cancel();
    assert_eq!(
        session.pump(4).unwrap(),
        PumpStatus::Stopped(ObservationEnd::Cancelled)
    );
    let evidence = session.finish().unwrap();
    assert!(evidence.records.is_empty());
    assert_eq!(evidence.end, ObservationEnd::Cancelled);
    assert!(evidence.observed_latest.capture.unwrap().running);
    let session =
        EvidenceSession::start(&bus, EvidenceLimits::default(), CancellationToken::new()).unwrap();
    drop(session);
    assert!(bus.snapshot(AudioDirection::Capture).unwrap().running);
    let mut again =
        EvidenceSession::start(&bus, EvidenceLimits::default(), CancellationToken::new()).unwrap();
    again.pump(4).unwrap();
    assert_eq!(again.finish().unwrap().records.len(), 2);
    assert!(bus.try_claim_reader().is_some());
    clock.probe.close();
}

#[test]
fn record_sample_and_byte_budgets_stop_explicitly_without_closing_stream() {
    for limits in [
        EvidenceLimits {
            records: 1,
            ..EvidenceLimits::default()
        },
        EvidenceLimits {
            samples: 1,
            ..EvidenceLimits::default()
        },
        EvidenceLimits {
            json_bytes: METADATA_RESERVE,
            ..EvidenceLimits::default()
        },
    ] {
        let bus = AudioBus::new(2).reference();
        let mut clock = bus.open_stream(AudioDirection::Playback, format());
        let mut session = EvidenceSession::start(&bus, limits, CancellationToken::new()).unwrap();
        publish(&mut clock, &[0.5, -0.5], ReferenceContent::Media);
        assert_eq!(
            session.pump(10).unwrap(),
            PumpStatus::Stopped(ObservationEnd::LimitReached)
        );
        let evidence = session.finish().unwrap();
        assert_eq!(evidence.end, ObservationEnd::LimitReached);
        assert!(evidence.unobserved_history_possible);
        assert!(evidence.discarded_records <= 1);
        assert!(bus.snapshot(AudioDirection::Playback).unwrap().running);
        let mut bytes = Vec::new();
        evidence
            .write_json(&mut bytes, limits, &CancellationToken::new())
            .unwrap();
        assert!(bytes.len() <= limits.json_bytes);
        clock.probe.close();
    }
}

#[test]
fn invalid_native_sample_stops_observation_and_retains_prior_valid_records() {
    let bus = AudioBus::new(2).reference();
    let mut clock = bus.open_stream(AudioDirection::Capture, format());
    let mut session =
        EvidenceSession::start(&bus, EvidenceLimits::default(), CancellationToken::new()).unwrap();
    publish(&mut clock, &[f64::NAN, 0.0], ReferenceContent::Media);
    assert!(session.pump(4).is_err());
    let evidence = session.finish().unwrap();
    assert_eq!(evidence.end, ObservationEnd::InvalidRecord);
    assert_eq!(evidence.discarded_records, 1);
    assert_eq!(evidence.discarded_samples, 2);
    assert_eq!(evidence.records.len(), 1);
    assert!(bus.snapshot(AudioDirection::Capture).unwrap().running);
    clock.probe.close();
}

#[test]
fn overflow_and_lost_close_marker_use_only_observed_latest_final_snapshot() {
    let bus = AudioBus::new(2).reference();
    let mut clock = bus.open_stream(AudioDirection::Playback, format());
    let mut session =
        EvidenceSession::start(&bus, EvidenceLimits::default(), CancellationToken::new()).unwrap();
    for _ in 0..80 {
        publish(&mut clock, &[0.0, 0.0], ReferenceContent::ZeroFill);
    }
    clock.probe.close();
    session.pump(MAX_RECORDS).unwrap();
    let evidence = session.finish().unwrap();
    assert_eq!(evidence.records.len(), 64);
    assert!(
        !evidence
            .records
            .iter()
            .any(|r| r.discontinuity == Some(AudioDiscontinuity::StreamStopped))
    );
    let final_state = evidence.observed_latest.playback.as_ref().unwrap();
    assert!(final_state.closed);
    assert!(final_state.lost_blocks > 0 && final_state.lost_frames > 0);
    assert_eq!(final_state.cursor, 80);
    let replayed_samples: usize = evidence
        .replay(EvidenceLimits::default())
        .unwrap()
        .map(|s| s.record.samples_f64_bits.len())
        .sum();
    assert_eq!(replayed_samples, 126); // 63 actual data blocks, no invented tail.
}

#[test]
fn reopened_stream_never_inherits_prior_stream_final_state() {
    let bus = AudioBus::new(2).reference();
    let mut old = bus.open_stream(AudioDirection::Capture, format());
    let mut session =
        EvidenceSession::start(&bus, EvidenceLimits::default(), CancellationToken::new()).unwrap();
    for _ in 0..70 {
        publish(&mut old, &[0.0, 0.0], ReferenceContent::Media);
    }
    old.probe.close(); // Full queue loses this marker.
    session.pump(MAX_RECORDS).unwrap();
    let mut new = bus.open_stream(AudioDirection::Capture, format());
    publish(&mut new, &[0.5, -0.5], ReferenceContent::Media);
    session.pump(MAX_RECORDS).unwrap();
    let evidence = session.finish().unwrap();
    let latest = evidence.observed_latest.capture.as_ref().unwrap();
    assert_eq!(latest.stream.stream_id, new.probe.stream_id());
    assert!(latest.running);
    assert!(
        !evidence
            .records
            .iter()
            .any(|r| r.timing.stream.stream_id == old.probe.stream_id()
                && r.discontinuity == Some(AudioDiscontinuity::StreamStopped))
    );
    assert!(
        evidence
            .replay(EvidenceLimits::default())
            .unwrap()
            .any(|s| s.resets.contains(&ReplayReset::StreamChanged))
    );
    new.probe.close();
}

#[test]
fn replay_preserves_marker_pcm_interleaving_and_resets_on_gaps_without_zero_fill() {
    let mut evidence = fixture();
    evidence.observed_latest = ObservedLatest::default();
    let mut first = evidence
        .records
        .iter()
        .find(|r| r.timing.frames > 0)
        .unwrap()
        .clone();
    first.sequence = 1;
    let mut marker = first.clone();
    marker.sequence = 2;
    marker.timing.continuity = 7;
    marker.timing.start_frame = 20;
    marker.timing.frames = 0;
    marker.samples_f64_bits.clear();
    marker.discontinuity = Some(AudioDiscontinuity::BackendError);
    marker.boundary = None;
    marker.sample_format = None;
    marker.content = ReferenceContent::ZeroFill;
    let mut later = first.clone();
    later.sequence = 4;
    later.lost_blocks_before = 1;
    later.lost_frames_before = 3;
    later.timing.start_frame = 5;
    // A callback observed continuity 1 before the error marker entered the queue.
    later.timing.continuity = 1;
    evidence.records = vec![first.clone(), marker, later.clone()];
    let steps: Vec<_> = evidence
        .replay(EvidenceLimits::default())
        .unwrap()
        .collect();
    assert_eq!(
        steps[2].resets,
        vec![
            ReplayReset::Discontinuity,
            ReplayReset::SequenceGap,
            ReplayReset::ProducerLoss,
            ReplayReset::FrameGap
        ]
    );
    assert_eq!(steps[2].record.samples_f64_bits, later.samples_f64_bits);
    assert_eq!(
        steps
            .iter()
            .map(|s| s.record.samples_f64_bits.len())
            .sum::<usize>(),
        first.samples_f64_bits.len() * 2
    );
}

#[test]
fn input_rejects_invalid_schema_identity_format_sample_and_sequence_contracts() {
    let original = serde_json::to_value(fixture()).unwrap();
    let pcm = original["records"]
        .as_array()
        .unwrap()
        .iter()
        .position(|r| r["timing"]["frames"].as_u64().unwrap() > 0)
        .unwrap();
    let mutations: Vec<Box<dyn Fn(&mut serde_json::Value)>> = vec![
        Box::new(|v| v["version"] = 2.into()),
        Box::new(|v| v["boot_id"] = Uuid::nil().to_string().into()),
        Box::new(|v| v["extra"] = true.into()),
        Box::new(|v| v["records"][0]["timing"]["extra"] = true.into()),
        Box::new(|v| {
            v["records"][0]["timing"]["stream"]["boot_id"] = Uuid::new_v4().to_string().into()
        }),
        Box::new(|v| {
            v["records"][0]["timing"]["stream"]["stream_id"] = Uuid::nil().to_string().into()
        }),
        Box::new(|v| {
            v["records"][0]["timing"]["clock"]["clock_id"] = Uuid::new_v4().to_string().into()
        }),
        Box::new(move |v| v["records"][pcm]["timing"]["format"]["channels"] = 0.into()),
        Box::new(move |v| v["records"][pcm]["timing"]["frames"] = 999.into()),
        Box::new(move |v| v["records"][pcm]["boundary"] = "AlsaWriteAccepted".into()),
        Box::new(move |v| v["records"][pcm]["sample_format"] = serde_json::Value::Null),
        Box::new(move |v| v["records"][pcm]["samples_f64_bits"][0] = f64::NAN.to_bits().into()),
        Box::new(move |v| v["records"][pcm]["samples_f64_bits"][0] = 1.1f64.to_bits().into()),
        Box::new(move |v| v["records"][pcm]["sequence"] = 1.into()),
        Box::new(move |v| v["records"][pcm]["content"] = "HeldSilence".into()),
        Box::new(move |v| {
            v["records"][pcm]["timing"]["clock"]["quality"] = "BackendEstimate".into()
        }),
        Box::new(move |v| v["records"][pcm]["timing"]["clock"]["queued_frames"] = 1.into()),
        Box::new(|v| v["records"][0]["timing"]["clock"]["callback_ns"] = 0.into()),
        Box::new(|v| v["observed_latest"]["capture"]["cursor"] = 0.into()),
        Box::new(|v| v["observed_latest"]["capture"]["continuity"] = 0.into()),
        Box::new(|v| v["records"][0]["lost_blocks_before"] = 3.into()),
        Box::new(|v| v["unobserved_history_possible"] = false.into()),
    ];
    for (index, mutate) in mutations.into_iter().enumerate() {
        let mut value = original.clone();
        mutate(&mut value);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(
            AudioEvidence::read_json(
                bytes.as_slice(),
                EvidenceLimits::default(),
                &CancellationToken::new()
            )
            .is_err(),
            "mutation {index} unexpectedly accepted"
        );
    }
}

#[test]
fn native_representation_cannot_change_within_stream_and_silence_cannot_hide_pcm() {
    let mut evidence = fixture();
    let first = evidence
        .records
        .iter()
        .find(|r| r.timing.frames > 0 && r.timing.stream.direction == AudioDirection::Playback)
        .unwrap()
        .clone();
    evidence.observed_latest = ObservedLatest::default();
    evidence.records = vec![first.clone(), first.clone()];
    evidence.records[1].sequence += 1;
    evidence.records[1].timing.start_frame += first.timing.frames;
    evidence.validate(EvidenceLimits::default()).unwrap();
    evidence.records[1].sample_format = Some(NativeSampleFormat::F32);
    assert!(evidence.validate(EvidenceLimits::default()).is_err());
    evidence.records[1].sample_format = first.sample_format;
    evidence.records[1].content = ReferenceContent::HeldSilence;
    assert!(evidence.validate(EvidenceLimits::default()).is_err());
    evidence.records[1]
        .samples_f64_bits
        .fill((-0.0f64).to_bits());
    evidence.validate(EvidenceLimits::default()).unwrap();
}

#[test]
fn active_snapshot_is_an_observation_not_a_coherent_final_transaction() {
    let mut evidence = fixture();
    let snapshot = evidence.observed_latest.capture.as_mut().unwrap();
    snapshot.closed = false;
    snapshot.running = true;
    snapshot.cursor = 0;
    snapshot.continuity = 0;
    snapshot.attempted_blocks = 0;
    snapshot.lost_blocks = 1;
    evidence.validate(EvidenceLimits::default()).unwrap();
    evidence.observed_latest.capture.as_mut().unwrap().running = false;
    evidence.observed_latest.capture.as_mut().unwrap().closed = true;
    assert!(evidence.validate(EvidenceLimits::default()).is_err());
}

#[test]
fn json_arrays_are_capped_while_parsing_before_contract_validation() {
    let original = serde_json::to_value(fixture()).unwrap();
    let mut too_many_records = original.clone();
    too_many_records["records"] = vec![original["records"][0].clone(); MAX_RECORDS + 1].into();
    let mut too_many_samples = original;
    too_many_samples["records"][1]["samples_f64_bits"] =
        vec![0u64; REFERENCE_BLOCK_SAMPLES + 1].into();
    for value in [too_many_records, too_many_samples] {
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(
            AudioEvidence::read_json(
                bytes.as_slice(),
                EvidenceLimits::default(),
                &CancellationToken::new()
            )
            .unwrap_err()
            .to_string()
            .contains("array limit exceeded")
        );
    }
}

#[test]
fn import_export_caps_cancellation_and_writer_failure_are_explicit() {
    let evidence = fixture();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let mut bytes = Vec::new();
    assert!(
        evidence
            .write_json(&mut bytes, EvidenceLimits::default(), &cancelled)
            .is_err()
    );
    assert!(bytes.is_empty());
    assert!(
        AudioEvidence::read_json(
            encoded(&evidence).as_slice(),
            EvidenceLimits::default(),
            &cancelled
        )
        .is_err()
    );
    let over = vec![b' '; METADATA_RESERVE + 1];
    assert!(
        AudioEvidence::read_json(
            over.as_slice(),
            EvidenceLimits {
                json_bytes: METADATA_RESERVE,
                ..EvidenceLimits::default()
            },
            &CancellationToken::new()
        )
        .unwrap_err()
        .to_string()
        .contains("byte limit")
    );
    let too_few = EvidenceLimits {
        records: 1,
        ..EvidenceLimits::default()
    };
    assert!(
        evidence
            .write_json(&mut bytes, too_few, &CancellationToken::new())
            .is_err()
    );
    assert!(bytes.is_empty());
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("injected sink error"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(
        evidence
            .write_json(Broken, EvidenceLimits::default(), &CancellationToken::new())
            .is_err()
    );
    let complete = encoded(&evidence);
    assert!(
        AudioEvidence::read_json(
            &complete[..complete.len() - 1],
            EvidenceLimits::default(),
            &CancellationToken::new()
        )
        .is_err()
    );
}

#[test]
fn write_failure_and_mid_export_cancel_leave_partial_sink_owned_by_caller() {
    let mut evidence = fixture();
    let mut data = evidence
        .records
        .iter()
        .find(|r| r.timing.stream.direction == AudioDirection::Playback && r.timing.frames > 0)
        .unwrap()
        .clone();
    data.samples_f64_bits = vec![0.25f64.to_bits(); REFERENCE_BLOCK_SAMPLES];
    data.timing.frames = (REFERENCE_BLOCK_SAMPLES / 2) as u64;
    evidence.records = vec![data];
    evidence.observed_latest = ObservedLatest::default();
    let complete = encoded(&evidence);
    assert!(complete.len() > 8192);
    struct PrefixThenFail(Vec<u8>);
    impl Write for PrefixThenFail {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if !self.0.is_empty() {
                return Err(std::io::Error::other("injected failure after prefix"));
            }
            self.0.extend_from_slice(&bytes[..17]);
            Ok(17)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut broken = PrefixThenFail(Vec::new());
    assert!(
        evidence
            .write_json(
                &mut broken,
                EvidenceLimits::default(),
                &CancellationToken::new()
            )
            .is_err()
    );
    assert_eq!(broken.0, complete[..17]);
    struct CancelAfterChunk {
        bytes: Vec<u8>,
        cancel: CancellationToken,
    }
    impl Write for CancelAfterChunk {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            self.cancel.cancel();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let cancel = CancellationToken::new();
    let mut sink = CancelAfterChunk {
        bytes: Vec::new(),
        cancel: cancel.clone(),
    };
    assert!(
        evidence
            .write_json(&mut sink, EvidenceLimits::default(), &cancel)
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    assert_eq!(sink.bytes, complete[..8192]);
}
