//! Offline generation and inspection of bounded evidence; never opens audio devices.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use alia_core::audio_clock::*;
use alia_embedded::module::audio_evidence::*;
use alia_embedded::module::context::{
    AudioSampleBoundary, AudioStreamSnapshot, NativeSampleFormat, ReferenceContent,
};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::{Value, json};
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
const USAGE: &str = "Usage: audio_evidence_replay generate <new-output.json>\n       audio_evidence_replay replay <existing-input.json>\n\nOffline only: no audio devices, models or services. Generation never overwrites files.";

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match execute(&args, &CancellationToken::new())? {
        Some(report) => println!("{}", serde_json::to_string_pretty(&report)?),
        None => println!("{USAGE}"),
    }
    Ok(())
}

fn execute(args: &[OsString], cancel: &CancellationToken) -> Result<Option<Value>> {
    let (generate, path) = match args {
        [help] if help == "--help" || help == "-h" => return Ok(None),
        [command, path] if command == "generate" => (true, PathBuf::from(path)),
        [command, path] if command == "replay" => (false, PathBuf::from(path)),
        _ => bail!("{USAGE}"),
    };
    let limits = EvidenceLimits::default();
    if generate {
        publish_fixture(&path, limits, cancel)?;
    }
    let evidence = read_file(&path, limits, cancel)?;
    Ok(Some(summarize(&evidence, limits)?))
}

fn publish_fixture(path: &Path, limits: EvidenceLimits, cancel: &CancellationToken) -> Result<()> {
    let mut bytes = Vec::new();
    synthetic_fixture().write_json(&mut bytes, limits, cancel)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    ensure!(!cancel.is_cancelled(), "fixture generation cancelled");
    temporary
        .persist_noclobber(path)
        .context("publishing new synthetic fixture without overwriting")?;
    Ok(())
}

fn read_file(
    path: &Path,
    limits: EvidenceLimits,
    cancel: &CancellationToken,
) -> Result<AudioEvidence> {
    ensure!(!cancel.is_cancelled(), "audio import cancelled");
    ensure!(
        std::fs::symlink_metadata(path)?.is_file(),
        "evidence input must be a regular file, not a symlink or device"
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path).context("opening evidence input")?;
    ensure!(
        file.metadata()?.is_file(),
        "evidence input must be a regular file"
    );
    AudioEvidence::read_json(file, limits, cancel)
}

#[derive(Serialize)]
struct StreamSummary {
    stream: AudioStreamId,
    format: AudioFormat,
    observed_records: usize,
    observed_frames: u64,
    observed_samples: usize,
    last_observed_sequence: Option<u64>,
    reset_counts: BTreeMap<String, usize>,
    close_marker_observed: bool,
    latest_snapshot: Option<AudioStreamSnapshot>,
}

fn summarize(evidence: &AudioEvidence, limits: EvidenceLimits) -> Result<Value> {
    let mut streams = BTreeMap::new();
    for step in evidence.replay(limits)? {
        let record = step.record;
        let stream = streams
            .entry(record.timing.stream.stream_id)
            .or_insert_with(|| StreamSummary {
                stream: record.timing.stream,
                format: record.timing.format,
                observed_records: 0,
                observed_frames: 0,
                observed_samples: 0,
                last_observed_sequence: None,
                reset_counts: BTreeMap::new(),
                close_marker_observed: false,
                latest_snapshot: None,
            });
        stream.observed_records += 1;
        stream.observed_frames += record.timing.frames;
        stream.observed_samples += record.samples_f64_bits.len();
        stream.last_observed_sequence = Some(record.sequence);
        stream.close_marker_observed |=
            record.discontinuity == Some(AudioDiscontinuity::StreamStopped);
        for reset in step.resets {
            *stream.reset_counts.entry(format!("{reset:?}")).or_default() += 1;
        }
    }
    for snapshot in [
        &evidence.observed_latest.capture,
        &evidence.observed_latest.playback,
    ]
    .into_iter()
    .flatten()
    {
        streams
            .entry(snapshot.stream.stream_id)
            .or_insert_with(|| StreamSummary {
                stream: snapshot.stream,
                format: snapshot.format,
                observed_records: 0,
                observed_frames: 0,
                observed_samples: 0,
                last_observed_sequence: None,
                reset_counts: BTreeMap::new(),
                close_marker_observed: false,
                latest_snapshot: None,
            })
            .latest_snapshot = Some(snapshot.clone());
    }
    Ok(json!({
        "source": evidence.source,
        "source_authenticated": false,
        "boot_id": evidence.boot_id,
        "end": evidence.end,
        "records": evidence.records.len(),
        "observed_samples": streams.values().map(|s| s.observed_samples).sum::<usize>(),
        "reset_signals": streams.values().flat_map(|s| s.reset_counts.values()).sum::<usize>(),
        "discarded_records": evidence.discarded_records,
        "discarded_samples": evidence.discarded_samples,
        "unobserved_history_possible": evidence.unobserved_history_possible,
        "streams": streams.values().collect::<Vec<_>>(),
        "physical_capture_playback_alignment": "unproven independent clocks",
        "physical_stream_complete": false,
        "snapshot_semantics": "latest per direction only; active fields are independent observations; a stream without a close marker or matching closed snapshot has unknown final state",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(command: &str, path: &Path) -> Vec<OsString> {
        vec![command.into(), path.as_os_str().into()]
    }

    #[test]
    fn cli_requires_explicit_operation_and_exact_arguments_before_writing() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("must-not-exist.json");
        for invalid in [
            vec![],
            vec![output.as_os_str().into()],
            args("--input", &output),
            vec!["generate".into()],
            vec!["generate".into(), output.as_os_str().into(), "extra".into()],
            vec!["--help".into(), output.as_os_str().into()],
        ] {
            assert!(execute(&invalid, &CancellationToken::new()).is_err());
        }
        for help in ["--help", "-h"] {
            assert!(
                execute(&[help.into()], &CancellationToken::new())
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn generate_then_replay_preserves_input_and_derives_each_stream_summary() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("evidence.json");
        let cancel = CancellationToken::new();
        let generated = execute(&args("generate", &path), &cancel).unwrap().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let replayed = execute(&args("replay", &path), &cancel).unwrap().unwrap();
        assert_eq!(replayed, generated);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(replayed["records"], 7);
        assert_eq!(replayed["observed_samples"], 16);
        assert_eq!(replayed["reset_signals"], 9);
        assert_eq!(replayed["source_authenticated"], false);
        assert_eq!(replayed["physical_stream_complete"], false);
        let streams = replayed["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 3);
        assert!(streams[0]["latest_snapshot"].is_null());
        assert_eq!(streams[0]["close_marker_observed"], false);
        assert_eq!(streams[1]["latest_snapshot"]["closed"], true);
        assert_eq!(streams[1]["latest_snapshot"]["lost_blocks"], 2);
        assert_eq!(streams[1]["close_marker_observed"], false);
        assert_eq!(streams[2]["latest_snapshot"]["running"], true);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn summary_handles_empty_single_direction_and_snapshot_only_without_inventing_records() {
        let limits = EvidenceLimits::default();
        let mut evidence = synthetic_fixture();
        evidence
            .records
            .retain(|r| r.timing.stream.direction == AudioDirection::Capture);
        evidence.observed_latest.playback = None;
        let capture = summarize(&evidence, limits).unwrap();
        assert!(
            capture["streams"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["stream"]["direction"] == "Capture")
        );
        evidence.records.clear();
        let footer = summarize(&evidence, limits).unwrap();
        assert_eq!(footer["records"], 0);
        assert_eq!(footer["streams"].as_array().unwrap().len(), 1);
        assert_eq!(footer["streams"][0]["observed_frames"], 0);
        assert!(footer["streams"][0]["last_observed_sequence"].is_null());
        evidence.observed_latest = ObservedLatest::default();
        evidence.source = EvidenceSource::ObservedNative;
        let empty = summarize(&evidence, limits).unwrap();
        assert_eq!(empty["streams"], json!([]));
        assert_eq!(empty["source"], "ObservedNative");
        assert_eq!(empty["source_authenticated"], false);
    }

    #[test]
    fn failure_cancel_and_existing_destination_preserve_files_without_temporary_leaks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("existing.json");
        let cancel = CancellationToken::new();
        std::fs::write(&path, b"original bytes").unwrap();
        assert!(execute(&args("generate", &path), &cancel).is_err());
        assert!(execute(&args("replay", &path), &cancel).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original bytes");
        let missing = root.path().join("missing.json");
        assert!(execute(&args("replay", &missing), &cancel).is_err());
        cancel.cancel();
        assert!(execute(&args("generate", &missing), &cancel).is_err());
        assert!(execute(&args("replay", &path), &cancel).is_err());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn file_input_rejects_symlinks_directories_and_fifos_without_modifying_targets() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture.json");
        let cancel = CancellationToken::new();
        execute(&args("generate", &path), &cancel).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let link = root.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let fifo = root.path().join("fifo");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        for input in [&link, &fifo, &root.path().to_path_buf()] {
            assert!(
                read_file(input, EvidenceLimits::default(), &cancel)
                    .unwrap_err()
                    .to_string()
                    .contains("regular file")
            );
        }
        assert!(execute(&args("generate", &link), &cancel).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 3);
    }
}
