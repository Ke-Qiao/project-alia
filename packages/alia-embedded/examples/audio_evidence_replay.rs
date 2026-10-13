//! Offline only. Writes a new synthetic evidence file, reads it back and reports
//! gaps/unknowns. Does not open an audio device or overwrite an existing file.
use std::io::Write;
use std::path::PathBuf;

use alia_core::audio_clock::*;
use alia_embedded::module::audio_evidence::*;
use alia_embedded::module::context::{
    AudioSampleBoundary, AudioStreamSnapshot, NativeSampleFormat, ReferenceContent,
};
use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn record(
    boot: Uuid,
    stream_id: u128,
    direction: AudioDirection,
    sequence: u64,
    start_frame: u64,
) -> EvidenceRecord {
    let stream_id = Uuid::from_u128(stream_id);
    EvidenceRecord {
        timing: AudioTiming {
            stream: AudioStreamId {
                boot_id: boot,
                stream_id,
                direction,
            },
            format: AudioFormat {
                sample_rate_hz: 48000,
                channels: 2,
            },
            start_frame,
            frames: 2,
            continuity: 1,
            clock: AudioClockReading {
                clock_id: stream_id,
                callback_ns: None,
                sample_ns: None,
                queued_frames: None,
                quality: AudioTimeQuality::Unknown,
            },
            observed_boot_ns: sequence * 1000,
        },
        boundary: Some(match direction {
            AudioDirection::Capture => AudioSampleBoundary::CpalInputCallback,
            AudioDirection::Playback => AudioSampleBoundary::CpalCallbackBuffer,
        }),
        sample_format: Some(NativeSampleFormat::F64),
        content: ReferenceContent::Media,
        discontinuity: None,
        sequence,
        lost_blocks_before: 0,
        lost_frames_before: 0,
        samples_f64_bits: [0.0f64, -0.0, 0.25, -0.25].map(f64::to_bits).to_vec(),
    }
}
fn marker(mut record: EvidenceRecord, reason: AudioDiscontinuity) -> EvidenceRecord {
    record.timing.frames = 0;
    record.samples_f64_bits.clear();
    record.boundary = None;
    record.sample_format = None;
    record.content = ReferenceContent::ZeroFill;
    record.discontinuity = Some(reason);
    record
}
fn synthetic_fixture() -> AudioEvidence {
    let boot = Uuid::from_u128(0xa11a);
    let capture = record(boot, 1, AudioDirection::Capture, 2, 0);
    let mut playback = record(boot, 2, AudioDirection::Playback, 2, 0);
    playback.content = ReferenceContent::HeldSilence;
    playback.samples_f64_bits = vec![0.0f64.to_bits(); 4];
    let mut gap = marker(
        record(boot, 1, AudioDirection::Capture, 4, 4),
        AudioDiscontinuity::Overrun,
    );
    gap.timing.continuity = 2;
    gap.lost_blocks_before = 1;
    gap.lost_frames_before = 2;
    let mut after_gap = record(boot, 1, AudioDirection::Capture, 5, 4);
    after_gap.timing.continuity = 2;
    after_gap.lost_blocks_before = 1;
    after_gap.lost_frames_before = 2;
    let reopened = record(boot, 3, AudioDirection::Capture, 2, 0);
    let observed_latest = ObservedLatest {
        capture: Some(AudioStreamSnapshot {
            stream: reopened.timing.stream,
            format: reopened.timing.format,
            running: true,
            closing: false,
            closed: false,
            cursor: 2,
            continuity: 1,
            last_discontinuity: Some(AudioDiscontinuity::StreamStarted),
            last_fault: None,
            attempted_blocks: 2,
            lost_blocks: 0,
            lost_frames: 0,
        }),
        playback: Some(AudioStreamSnapshot {
            stream: playback.timing.stream,
            format: playback.timing.format,
            running: false,
            closing: false,
            closed: true,
            cursor: 4,
            continuity: 2,
            last_discontinuity: Some(AudioDiscontinuity::StreamStopped),
            last_fault: None,
            attempted_blocks: 4,
            lost_blocks: 2,
            lost_frames: 2,
        }),
    };
    AudioEvidence {
        version: 1,
        boot_id: boot,
        source: EvidenceSource::SyntheticFixture,
        records: vec![
            marker(
                record(boot, 1, AudioDirection::Capture, 1, 0),
                AudioDiscontinuity::StreamStarted,
            ),
            marker(
                record(boot, 2, AudioDirection::Playback, 1, 0),
                AudioDiscontinuity::StreamStarted,
            ),
            capture,
            playback,
            gap,
            after_gap,
            reopened,
        ],
        end: ObservationEnd::Finished,
        unobserved_history_possible: true,
        discarded_records: 0,
        discarded_samples: 0,
        observed_latest,
    }
}
fn main() -> Result<()> {
    let path = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .context("usage: audio_evidence_replay <new-output.json>")?,
    );
    let cancel = CancellationToken::new();
    let limits = EvidenceLimits::default();
    let evidence = synthetic_fixture();
    let mut bytes = Vec::new();
    evidence.write_json(&mut bytes, limits, &cancel)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(&path)
        .context("publishing new synthetic fixture without overwriting")?;
    let decoded = AudioEvidence::read_json(std::fs::File::open(&path)?, limits, &cancel)?;
    let mut reset_count = 0;
    let mut sample_count = 0;
    for step in decoded.replay(limits)? {
        reset_count += step.resets.len();
        sample_count += step.record.samples_f64_bits.len();
    }
    // The footer is mandatory evidence: playback lost both a PCM block and its
    // close marker, while the replaced capture stream has no proven final state.
    let playback = decoded.observed_latest.playback.as_ref().unwrap();
    let capture = decoded.observed_latest.capture.as_ref().unwrap();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "source": decoded.source,
            "records": decoded.records.len(), "observed_samples": sample_count,
            "reset_signals": reset_count, "unobserved_history_possible": decoded.unobserved_history_possible,
            "latest_playback_closed": playback.closed,
            "latest_playback_lost_blocks": playback.lost_blocks,
            "latest_playback_lost_frames": playback.lost_frames,
            "latest_capture_running": capture.running,
            "replaced_capture_final_state": "unknown: neither close marker nor retained snapshot",
            "physical_capture_playback_alignment": "unproven independent clocks",
            "physical_stream_complete": false
        }))?
    );
    Ok(())
}
