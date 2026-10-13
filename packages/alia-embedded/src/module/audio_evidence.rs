//! Explicit, caller-owned native audio observation and offline replay.
//!
//! These limits bound diagnostic memory, not acoustic acceptance thresholds.
//! No callback performs I/O, no background task is started, and dropping an
//! observer never closes a native stream. Missing samples are never zero-filled.
use std::io::{Read, Write};

use alia_core::audio_clock::{AudioDirection, AudioStreamId, AudioTimeQuality, AudioTiming};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::context::{
    AudioReferenceBlock, AudioReferenceBus, AudioReferenceReader, AudioSampleBoundary,
    AudioStreamSnapshot, NativeSampleFormat, REFERENCE_BLOCK_SAMPLES, ReferenceContent,
};

pub const MAX_RECORDS: usize = 512;
pub const MAX_SAMPLES: usize = MAX_RECORDS * REFERENCE_BLOCK_SAMPLES;
pub const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;
// Conservative space for the header, termination and two latest snapshots.
const METADATA_RESERVE: usize = 8192;

#[derive(Clone, Copy, Debug)]
pub struct EvidenceLimits {
    pub records: usize,
    pub samples: usize,
    pub json_bytes: usize,
}
impl Default for EvidenceLimits {
    fn default() -> Self {
        Self {
            records: MAX_RECORDS,
            samples: MAX_SAMPLES,
            json_bytes: MAX_JSON_BYTES,
        }
    }
}
impl EvidenceLimits {
    fn validate(self) -> Result<()> {
        ensure!(
            (1..=MAX_RECORDS).contains(&self.records),
            "invalid record limit"
        );
        ensure!(
            (1..=MAX_SAMPLES).contains(&self.samples),
            "invalid sample limit"
        );
        ensure!(
            (METADATA_RESERVE..=MAX_JSON_BYTES).contains(&self.json_bytes),
            "invalid JSON byte limit"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceSource {
    /// Observed at declared native software boundaries, not physical evidence.
    ObservedNative,
    SyntheticFixture,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObservationEnd {
    /// The caller stopped observing. This does not assert a complete stream.
    Finished,
    Cancelled,
    LimitReached,
    InvalidRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecord {
    pub timing: AudioTiming,
    pub boundary: Option<AudioSampleBoundary>,
    pub sample_format: Option<NativeSampleFormat>,
    pub content: ReferenceContent,
    pub discontinuity: Option<alia_core::audio_clock::AudioDiscontinuity>,
    pub sequence: u64,
    pub lost_blocks_before: u64,
    pub lost_frames_before: u64,
    /// IEEE f64 bits preserve the normalized native sample exactly, including
    /// negative zero. Normalizing i64/u64 may already have lost integer precision.
    #[serde(deserialize_with = "deserialize_samples")]
    pub samples_f64_bits: Vec<u64>,
}
impl EvidenceRecord {
    fn from_block(block: AudioReferenceBlock) -> Result<Self> {
        ensure!(
            block.sample_count <= REFERENCE_BLOCK_SAMPLES,
            "invalid native sample count"
        );
        Ok(Self {
            timing: block.timing,
            boundary: block.boundary,
            sample_format: block.sample_format,
            content: block.content,
            discontinuity: block.discontinuity,
            sequence: block.sequence,
            lost_blocks_before: block.lost_blocks_before,
            lost_frames_before: block.lost_frames_before,
            samples_f64_bits: block.samples[..block.sample_count]
                .iter()
                .map(|x| x.to_bits())
                .collect(),
        })
    }
    pub fn samples(&self) -> impl ExactSizeIterator<Item = f64> + '_ {
        self.samples_f64_bits.iter().map(|x| f64::from_bits(*x))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedLatest {
    /// Each direction stores only its most recently opened stream. A replaced
    /// stream's final state is unknown unless its own close marker was observed.
    pub capture: Option<AudioStreamSnapshot>,
    pub playback: Option<AudioStreamSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioEvidence {
    pub version: u32,
    pub boot_id: Uuid,
    pub source: EvidenceSource,
    #[serde(deserialize_with = "deserialize_records")]
    pub records: Vec<EvidenceRecord>,
    pub end: ObservationEnd,
    /// Always true: an observation interval cannot establish physical completeness.
    pub unobserved_history_possible: bool,
    /// Records popped by this observer but rejected by a limit or validation.
    /// Pending queue entries and subsequent production remain unknown.
    pub discarded_records: usize,
    pub discarded_samples: usize,
    pub observed_latest: ObservedLatest,
}

/// Owns the only queue reader. No device cancellation/close handle is retained.
pub struct EvidenceSession {
    reader: AudioReferenceReader,
    cancel: CancellationToken,
    limits: EvidenceLimits,
    evidence: AudioEvidence,
    sample_count: usize,
    record_bytes: usize,
    stopped: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PumpStatus {
    /// No block was available; the native stream may still be running.
    Empty,
    Pumped(usize),
    Stopped(ObservationEnd),
}
impl EvidenceSession {
    pub fn start(
        bus: &AudioReferenceBus,
        limits: EvidenceLimits,
        cancel: CancellationToken,
    ) -> Result<Self> {
        limits.validate()?;
        ensure!(!cancel.is_cancelled(), "audio observation cancelled");
        let reader = bus
            .try_claim_reader()
            .ok_or_else(|| anyhow::anyhow!("audio reader already claimed"))?;
        let boot_id = reader.boot_id();
        ensure!(!boot_id.is_nil(), "invalid observation boot");
        Ok(Self {
            reader,
            cancel,
            limits,
            sample_count: 0,
            record_bytes: 0,
            stopped: false,
            evidence: AudioEvidence {
                version: 1,
                boot_id,
                source: EvidenceSource::ObservedNative,
                records: Vec::new(),
                end: ObservationEnd::Finished,
                unobserved_history_possible: true,
                discarded_records: 0,
                discarded_samples: 0,
                observed_latest: ObservedLatest::default(),
            },
        })
    }
    pub fn pump(&mut self, max_records: usize) -> Result<PumpStatus> {
        ensure!(
            (1..=MAX_RECORDS).contains(&max_records),
            "invalid pump limit"
        );
        let mut added = 0;
        while added < max_records {
            if self.stopped {
                return Ok(PumpStatus::Stopped(self.evidence.end));
            }
            if self.cancel.is_cancelled() {
                return Ok(self.stop(ObservationEnd::Cancelled));
            }
            if self.evidence.records.len() == self.limits.records {
                return Ok(self.stop(ObservationEnd::LimitReached));
            }
            let Some(block) = self.reader.try_read() else {
                return Ok(if added == 0 {
                    PumpStatus::Empty
                } else {
                    PumpStatus::Pumped(added)
                });
            };
            let count = block.sample_count;
            let record = match EvidenceRecord::from_block(block) {
                Ok(record) => record,
                Err(error) => {
                    self.discard(count, ObservationEnd::InvalidRecord);
                    return Err(error);
                }
            };
            let bytes = serde_json::to_vec(&record)?.len() + 1;
            if self.sample_count.saturating_add(count) > self.limits.samples
                || self.record_bytes.saturating_add(bytes)
                    > self.limits.json_bytes - METADATA_RESERVE
            {
                self.discard(count, ObservationEnd::LimitReached);
                return Ok(PumpStatus::Stopped(ObservationEnd::LimitReached));
            }
            if let Err(error) =
                validate_record(&record, self.evidence.boot_id, &self.evidence.records)
            {
                self.discard(count, ObservationEnd::InvalidRecord);
                return Err(error);
            }
            self.sample_count += count;
            self.record_bytes += bytes;
            self.evidence.records.push(record);
            added += 1;
        }
        Ok(PumpStatus::Pumped(added))
    }
    fn stop(&mut self, reason: ObservationEnd) -> PumpStatus {
        self.stopped = true;
        self.evidence.end = reason;
        PumpStatus::Stopped(reason)
    }
    fn discard(&mut self, samples: usize, reason: ObservationEnd) {
        self.evidence.discarded_records += 1;
        self.evidence.discarded_samples += samples;
        self.stop(reason);
    }
    pub fn finish(mut self) -> Result<AudioEvidence> {
        if !self.stopped && self.cancel.is_cancelled() {
            self.stop(ObservationEnd::Cancelled);
        }
        self.evidence.observed_latest = ObservedLatest {
            capture: self.reader.snapshot(AudioDirection::Capture),
            playback: self.reader.snapshot(AudioDirection::Playback),
        };
        self.evidence.validate(self.limits)?;
        Ok(self.evidence)
    }
}

impl AudioEvidence {
    pub fn validate(&self, limits: EvidenceLimits) -> Result<()> {
        limits.validate()?;
        ensure!(
            self.version == 1 && !self.boot_id.is_nil(),
            "unsupported evidence header"
        );
        ensure!(
            self.unobserved_history_possible,
            "observation cannot establish complete history"
        );
        ensure!(
            self.records.len() <= limits.records,
            "record limit exceeded"
        );
        ensure!(
            self.discarded_records <= 1 && self.discarded_samples <= REFERENCE_BLOCK_SAMPLES,
            "invalid observer discard count"
        );
        ensure!(
            self.discarded_records != 0 || self.discarded_samples == 0,
            "samples discarded without a record"
        );
        ensure!(
            self.discarded_records == 0
                || matches!(
                    self.end,
                    ObservationEnd::LimitReached | ObservationEnd::InvalidRecord
                ),
            "invalid discard termination"
        );
        let mut samples = 0usize;
        for (i, record) in self.records.iter().enumerate() {
            samples = samples
                .checked_add(record.samples_f64_bits.len())
                .ok_or_else(|| anyhow::anyhow!("sample count overflow"))?;
            ensure!(samples <= limits.samples, "sample limit exceeded");
            validate_record(record, self.boot_id, &self.records[..i])?;
        }
        for (direction, snapshot) in [
            (AudioDirection::Capture, &self.observed_latest.capture),
            (AudioDirection::Playback, &self.observed_latest.playback),
        ] {
            if let Some(snapshot) = snapshot {
                validate_id(snapshot.stream, self.boot_id)?;
                ensure!(
                    snapshot.stream.direction == direction && snapshot.format.is_valid(),
                    "invalid snapshot identity/format"
                );
                ensure!(
                    usize::from(snapshot.running)
                        + usize::from(snapshot.closing)
                        + usize::from(snapshot.closed)
                        <= 1,
                    "invalid snapshot phase"
                );
                // Active fields are separate atomic observations, not a coherent
                // transaction. Only a closed snapshot publishes final counters.
                if snapshot.closed {
                    ensure!(
                        snapshot.lost_blocks <= snapshot.attempted_blocks,
                        "invalid final snapshot loss"
                    );
                }
                for record in self
                    .records
                    .iter()
                    .filter(|r| r.timing.stream.stream_id == snapshot.stream.stream_id)
                {
                    ensure!(
                        record.timing.stream == snapshot.stream
                            && record.timing.format == snapshot.format,
                        "snapshot identity/format changed"
                    );
                    if snapshot.closed {
                        ensure!(
                            snapshot.attempted_blocks >= record.sequence
                                && snapshot.lost_blocks >= record.lost_blocks_before
                                && snapshot.lost_frames >= record.lost_frames_before,
                            "final snapshot counters precede observed records"
                        );
                        ensure!(
                            snapshot.cursor >= record.timing.start_frame + record.timing.frames
                                && snapshot.continuity >= record.timing.continuity,
                            "final snapshot precedes observed timing"
                        );
                    }
                }
            }
        }
        if let (Some(capture), Some(playback)) = (
            &self.observed_latest.capture,
            &self.observed_latest.playback,
        ) {
            ensure!(
                capture.stream.stream_id != playback.stream.stream_id,
                "snapshot stream identity reused across directions"
            );
        }
        Ok(())
    }
    /// Synchronous caller-owned export. The caller owns any partial destination
    /// on error; cancellation cannot interrupt a blocking Write implementation.
    pub fn write_json(
        &self,
        mut writer: impl Write,
        limits: EvidenceLimits,
        cancel: &CancellationToken,
    ) -> Result<usize> {
        self.validate(limits)?;
        ensure!(!cancel.is_cancelled(), "audio export cancelled");
        let mut bounded = BoundedJson {
            bytes: Vec::new(),
            limit: limits.json_bytes,
            cancel,
        };
        serde_json::to_writer(&mut bounded, self)?;
        for chunk in bounded.bytes.chunks(8192) {
            ensure!(!cancel.is_cancelled(), "audio export cancelled");
            writer.write_all(chunk)?;
        }
        ensure!(!cancel.is_cancelled(), "audio export cancelled");
        Ok(bounded.bytes.len())
    }
    /// Checks the byte cap before parsing. File/network lifecycle stays with the
    /// caller; cancellation is checked between synchronous read operations.
    pub fn read_json(
        mut reader: impl Read,
        limits: EvidenceLimits,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        limits.validate()?;
        let mut bytes = Vec::new();
        let mut chunk = [0; 8192];
        loop {
            ensure!(!cancel.is_cancelled(), "audio import cancelled");
            let remaining = limits.json_bytes + 1 - bytes.len();
            let count = reader.read(&mut chunk[..remaining.min(8192)])?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            ensure!(bytes.len() <= limits.json_bytes, "JSON byte limit exceeded");
        }
        ensure!(!cancel.is_cancelled(), "audio import cancelled");
        let evidence: Self = serde_json::from_slice(&bytes)?;
        evidence.validate(limits)?;
        // Core timing structs are shared types, so also reject unknown nested
        // fields without changing their unrelated wire contracts.
        reject_unknown_fields(
            &serde_json::from_slice(&bytes)?,
            &serde_json::to_value(&evidence)?,
        )?;
        ensure!(!cancel.is_cancelled(), "audio import cancelled");
        Ok(evidence)
    }
    pub fn replay(&self, limits: EvidenceLimits) -> Result<Replay<'_>> {
        self.validate(limits)?;
        Ok(Replay {
            evidence: self,
            index: 0,
        })
    }
}

fn validate_id(stream: AudioStreamId, boot_id: Uuid) -> Result<()> {
    ensure!(
        stream.boot_id == boot_id && !stream.stream_id.is_nil(),
        "mixed or nil stream identity"
    );
    Ok(())
}
fn validate_record(
    record: &EvidenceRecord,
    boot_id: Uuid,
    previous: &[EvidenceRecord],
) -> Result<()> {
    let timing = record.timing;
    validate_id(timing.stream, boot_id)?;
    ensure!(
        timing.format.is_valid() && timing.clock.clock_id == timing.stream.stream_id,
        "invalid format or stream clock"
    );
    ensure!(
        record.sequence > 0 && record.lost_blocks_before < record.sequence,
        "invalid sequence/loss"
    );
    ensure!(
        timing.start_frame.checked_add(timing.frames).is_some(),
        "frame cursor overflow"
    );
    ensure!(
        record.samples_f64_bits.len() <= REFERENCE_BLOCK_SAMPLES,
        "record sample limit exceeded"
    );
    ensure!(
        timing.frames.checked_mul(u64::from(timing.format.channels))
            == Some(record.samples_f64_bits.len() as u64),
        "frame/sample length mismatch"
    );
    ensure!(
        record
            .samples()
            .all(|x| x.is_finite() && (-1.0..=1.0).contains(&x)),
        "invalid normalized sample"
    );
    if timing.frames == 0 {
        ensure!(
            record.discontinuity.is_some()
                && record.boundary.is_none()
                && record.sample_format.is_none()
                && record.content == ReferenceContent::ZeroFill
                && timing.clock.quality == AudioTimeQuality::Unknown
                && timing.clock.callback_ns.is_none()
                && timing.clock.sample_ns.is_none()
                && timing.clock.queued_frames.is_none(),
            "invalid marker"
        );
    } else {
        ensure!(
            record.discontinuity.is_none() && record.sample_format.is_some(),
            "invalid sample block"
        );
        let boundary_valid = matches!(
            (timing.stream.direction, record.boundary),
            (
                AudioDirection::Capture,
                Some(AudioSampleBoundary::CpalInputCallback | AudioSampleBoundary::AlsaCaptureRead)
            ) | (
                AudioDirection::Playback,
                Some(
                    AudioSampleBoundary::CpalCallbackBuffer
                        | AudioSampleBoundary::AlsaWriteAccepted
                )
            )
        );
        ensure!(boundary_valid, "boundary/direction mismatch");
        ensure!(
            timing.clock.quality != AudioTimeQuality::BackendEstimate
                || timing.clock.callback_ns.is_some(),
            "backend estimate lacks callback observation"
        );
        if timing.stream.direction == AudioDirection::Capture {
            ensure!(
                record.content == ReferenceContent::Media,
                "invalid capture content"
            );
        }
        match record.boundary {
            Some(
                AudioSampleBoundary::CpalInputCallback | AudioSampleBoundary::CpalCallbackBuffer,
            ) => ensure!(
                timing.clock.queued_frames.is_none(),
                "CPAL queue depth is unknown"
            ),
            Some(AudioSampleBoundary::AlsaCaptureRead) => {
                ensure!(
                    timing.clock.sample_ns.is_none()
                        && matches!(
                            record.sample_format,
                            Some(NativeSampleFormat::I16 | NativeSampleFormat::I32)
                        ),
                    "invalid ALSA capture representation"
                );
            }
            Some(AudioSampleBoundary::AlsaWriteAccepted) => {
                ensure!(
                    timing.clock.sample_ns.is_none()
                        && record.sample_format == Some(NativeSampleFormat::I16)
                        && matches!(
                            record.content,
                            ReferenceContent::Media | ReferenceContent::HeldSilence
                        ),
                    "invalid ALSA playback representation"
                );
            }
            None => unreachable!("sample boundary checked above"),
        }
        if matches!(
            record.content,
            ReferenceContent::ZeroFill | ReferenceContent::HeldSilence
        ) {
            ensure!(
                record.samples().all(|x| x == 0.0),
                "silent block contains nonzero samples"
            );
        }
    }
    for prior in previous
        .iter()
        .filter(|p| p.timing.stream.stream_id == timing.stream.stream_id)
    {
        ensure!(
            prior.timing.stream == timing.stream && prior.timing.format == timing.format,
            "stream identity/format changed"
        );
        ensure!(
            prior.sequence < record.sequence
                && prior.lost_blocks_before <= record.lost_blocks_before
                && prior.lost_frames_before <= record.lost_frames_before,
            "sequence/loss moved backwards"
        );
        // A concurrent error marker can enter the queue before PCM whose native
        // observation predates it. Preserve that continuity value and reset at
        // replay; do not relabel or discard actual samples.
        if prior.timing.frames > 0 && timing.frames > 0 {
            ensure!(
                prior.timing.start_frame + prior.timing.frames <= timing.start_frame,
                "sample cursors overlap"
            );
            ensure!(
                prior.sample_format == record.sample_format && prior.boundary == record.boundary,
                "native representation changed within a stream"
            );
        }
    }
    Ok(())
}
fn reject_unknown_fields(actual: &serde_json::Value, expected: &serde_json::Value) -> Result<()> {
    match (actual, expected) {
        (serde_json::Value::Object(actual), serde_json::Value::Object(expected)) => {
            for (key, value) in actual {
                let Some(expected) = expected.get(key) else {
                    bail!("unknown evidence field");
                };
                reject_unknown_fields(value, expected)?;
            }
        }
        (serde_json::Value::Array(actual), serde_json::Value::Array(expected)) => {
            for (actual, expected) in actual.iter().zip(expected) {
                reject_unknown_fields(actual, expected)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn deserialize_records<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<EvidenceRecord>, D::Error> {
    deserialize_capped::<D, EvidenceRecord, MAX_RECORDS>(deserializer)
}
fn deserialize_samples<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<u64>, D::Error> {
    deserialize_capped::<D, u64, REFERENCE_BLOCK_SAMPLES>(deserializer)
}
fn deserialize_capped<'de, D, T, const MAX: usize>(
    deserializer: D,
) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Capped<T, const N: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> serde::de::Visitor<'de> for Capped<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "at most {N} elements")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> std::result::Result<Vec<T>, A::Error> {
            let mut output = Vec::new();
            while let Some(value) = sequence.next_element()? {
                if output.len() == N {
                    return Err(serde::de::Error::custom(
                        "audio evidence array limit exceeded",
                    ));
                }
                output.push(value);
            }
            Ok(output)
        }
    }
    deserializer.deserialize_seq(Capped::<T, MAX>(std::marker::PhantomData))
}
struct BoundedJson<'a> {
    bytes: Vec<u8>,
    limit: usize,
    cancel: &'a CancellationToken,
}
impl Write for BoundedJson<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::other("audio export cancelled"));
        }
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            return Err(std::io::Error::other("JSON byte limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayReset {
    FirstObservation,
    StreamChanged,
    Discontinuity,
    SequenceGap,
    FrameGap,
    ProducerLoss,
}
pub struct ReplayStep<'a> {
    pub record: &'a EvidenceRecord,
    /// Reset requirements, independent for each direction. No replacement PCM.
    pub resets: Vec<ReplayReset>,
}
/// Only the observed records are yielded. Always inspect `observed_latest` on
/// the evidence after replay: tail loss or a lost close marker may exist only
/// there. A replaced stream with no observed close has unknown final state.
pub struct Replay<'a> {
    evidence: &'a AudioEvidence,
    index: usize,
}
impl<'a> Iterator for Replay<'a> {
    type Item = ReplayStep<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let record = self.evidence.records.get(self.index)?;
        let previous = &self.evidence.records[..self.index];
        self.index += 1;
        let mut resets = Vec::new();
        match previous
            .iter()
            .rev()
            .find(|p| p.timing.stream.direction == record.timing.stream.direction)
        {
            None => resets.push(ReplayReset::FirstObservation),
            Some(p) if p.timing.stream != record.timing.stream => {
                resets.push(ReplayReset::StreamChanged)
            }
            _ => {}
        }
        if record.discontinuity.is_some() {
            resets.push(ReplayReset::Discontinuity);
        }
        if let Some(prior) = previous
            .iter()
            .rev()
            .find(|p| p.timing.stream == record.timing.stream)
        {
            if prior.timing.continuity != record.timing.continuity && record.discontinuity.is_none()
            {
                resets.push(ReplayReset::Discontinuity);
            }
            if prior.sequence + 1 != record.sequence {
                resets.push(ReplayReset::SequenceGap);
            }
            if prior.lost_blocks_before != record.lost_blocks_before
                || prior.lost_frames_before != record.lost_frames_before
            {
                resets.push(ReplayReset::ProducerLoss);
            }
        } else if record.lost_blocks_before > 0 || record.lost_frames_before > 0 {
            resets.push(ReplayReset::ProducerLoss);
        }
        if record.timing.frames > 0 {
            if let Some(prior) = previous
                .iter()
                .rev()
                .find(|p| p.timing.stream == record.timing.stream && p.timing.frames > 0)
            {
                if prior.timing.start_frame + prior.timing.frames != record.timing.start_frame {
                    resets.push(ReplayReset::FrameGap);
                }
            }
        }
        Some(ReplayStep { record, resets })
    }
}

#[cfg(test)]
mod tests;
