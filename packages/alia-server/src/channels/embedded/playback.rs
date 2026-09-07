//! Scoped playback transport. A successful Push proves queue acceptance only.
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alia_core::delivery::VoiceScope;
use alia_proto::playback::{self as proto, playback_client::PlaybackClient, playback_frame};
use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use tokio::sync::mpsc;
use tonic::transport::Channel;

use crate::channels::adapter::{AudioFormat, AudioPayload, AudioPayloadStream};

pub(super) fn encode_scope(scope: VoiceScope) -> proto::VoiceScope {
    proto::VoiceScope {
        device_boot_id: scope.device_boot_id.to_string(),
        session_id: scope.session_id.to_string(),
        generation_epoch: scope.generation_epoch,
    }
}

pub(super) fn validate_fence_receipt(
    scope: VoiceScope,
    close_session: bool,
    receipt: &proto::FenceReceipt,
) -> Result<()> {
    let same_session = receipt.active_scope.as_ref().is_some_and(|active| {
        active.device_boot_id == scope.device_boot_id.to_string()
            && active.session_id == scope.session_id.to_string()
    });
    if close_session {
        ensure!(
            !same_session,
            "device did not retire the requested voice session"
        );
    } else {
        ensure!(
            !receipt.session_closed
                && same_session
                && receipt
                    .active_scope
                    .as_ref()
                    .is_some_and(|active| active.generation_epoch >= scope.generation_epoch),
            "device did not establish the requested playback fence"
        );
    }
    Ok(())
}

struct Producer(tokio::task::JoinHandle<Result<u64>>);
impl Drop for Producer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn push(
    mut client: PlaybackClient<Channel>,
    scope: VoiceScope,
    playback_sequence: u64,
    audio: AudioPayloadStream,
    auth: tonic::metadata::MetadataMap,
    receipt_idle_timeout: Duration,
) -> Result<()> {
    ensure!(scope.is_valid(), "invalid voice output scope");
    let playback_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(8);
    let offered = Arc::new(AtomicU64::new(0));
    let offered_by_producer = offered.clone();
    let producer_id = playback_id.clone();
    let mut producer = Producer(tokio::spawn(async move {
        produce(
            audio,
            tx,
            scope,
            producer_id,
            playback_sequence,
            offered_by_producer,
        )
        .await
    }));
    let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
    *request.metadata_mut() = auth;
    let result = async {
        let mut receipts = tokio::time::timeout(receipt_idle_timeout, client.push(request))
            .await
            .context("playback response handshake timed out")??
            .into_inner();
        let mut accepted = 0;
        let mut ended = false;
        while let Some(receipt) = tokio::time::timeout(receipt_idle_timeout, receipts.message())
            .await
            .context("playback receipt stream idle timeout")??
        {
            ensure!(
                receipt.scope.as_ref() == Some(&encode_scope(scope))
                    && receipt.playback_id == playback_id
                    && receipt.playback_sequence == playback_sequence,
                "playback receipt identity mismatch"
            );
            ensure!(!ended, "receipt arrived after End acceptance");
            ensure!(
                receipt.accepted_samples >= accepted
                    && receipt.accepted_samples <= offered.load(Ordering::Acquire),
                "playback receipt sample count is invalid"
            );
            ensure!(
                proto::PlaybackState::try_from(receipt.state)? == proto::PlaybackState::Accepted,
                "device cancelled or failed playback admission"
            );
            // This adapter never upgrades queue acceptance into presentation.
            ensure!(
                receipt.presented_samples.is_none() && receipt.submitted_samples.is_none(),
                "unexpected device evidence in admission-only playback protocol"
            );
            accepted = receipt.accepted_samples;
            ended = receipt.end_accepted;
        }
        ensure!(ended, "playback stream ended without End acceptance");
        Ok::<u64, anyhow::Error>(accepted)
    }
    .await;
    if result.is_err() {
        producer.0.abort();
    }
    let produced = (&mut producer.0).await;
    let accepted = result?;
    let produced = produced.context("playback producer task failed")??;
    ensure!(
        accepted == produced,
        "device did not accept all playback samples"
    );
    Ok(())
}

async fn produce(
    mut audio: AudioPayloadStream,
    tx: mpsc::Sender<proto::PlaybackFrame>,
    scope: VoiceScope,
    playback_id: String,
    playback_sequence: u64,
    offered: Arc<AtomicU64>,
) -> Result<u64> {
    let mut decoder = PcmDecoder::default();
    let mut started = false;
    let mut sequence = 0_u64;
    let mut samples = 0_u64;
    let frame = |payload| proto::PlaybackFrame {
        scope: Some(encode_scope(scope)),
        playback_id: playback_id.clone(),
        playback_sequence,
        payload: Some(payload),
    };
    while let Some(payload) = audio.next().await {
        let pcm = decoder.push(payload?)?;
        let Some((sample_rate, _)) = decoder.spec else {
            continue;
        };
        if !started {
            tx.send(frame(playback_frame::Payload::Start(
                proto::PlaybackStart {
                    sample_rate_hz: sample_rate,
                    channels: 1,
                    source: "tts".into(),
                },
            )))
            .await
            .context("playback Start stream closed")?;
            started = true;
        }
        for bytes in pcm.chunks(sample_rate as usize / 10 * 2) {
            let next = samples
                .checked_add(bytes.len() as u64 / 2)
                .context("playback sample clock overflow")?;
            offered.store(next, Ordering::Release);
            tx.send(frame(playback_frame::Payload::Audio(
                proto::PlaybackAudio {
                    sequence,
                    sample_offset: samples,
                    pcm_s16le: bytes.to_vec(),
                },
            )))
            .await
            .context("playback Audio stream closed")?;
            samples = next;
            sequence = sequence
                .checked_add(1)
                .context("playback sequence overflow")?;
        }
    }
    decoder.finish()?;
    ensure!(started, "TTS returned no audio format");
    tx.send(frame(playback_frame::Payload::End(proto::PlaybackEnd {
        total_samples: samples,
    })))
    .await
    .context("playback End stream closed")?;
    Ok(samples)
}

/// Preserve arbitrary provider chunk boundaries, including split WAV headers and
/// split PCM samples. A WAV header, not the legacy hardcoded 24 kHz hint, owns rate.
#[derive(Default)]
struct PcmDecoder {
    format: Option<AudioFormat>,
    spec: Option<(u32, usize)>,
    pending: Vec<u8>,
    header_done: bool,
    declared_bytes: Option<u64>,
    received_bytes: u64,
}

impl PcmDecoder {
    fn push(&mut self, payload: AudioPayload) -> Result<Vec<u8>> {
        if let Some(format) = &self.format {
            ensure!(
                *format == payload.format,
                "TTS audio format changed mid-Push"
            );
        } else {
            self.format = Some(payload.format.clone());
        }
        ensure!(
            payload.format != AudioFormat::Opus,
            "scoped playback requires PCM16 or PCM WAV"
        );
        if payload.format == AudioFormat::Pcm16 {
            let spec = (payload.sample_rate, payload.channels as usize);
            validate_spec(spec)?;
            ensure!(
                self.spec.is_none_or(|previous| previous == spec),
                "TTS PCM format changed mid-Push"
            );
            self.spec = Some(spec);
            self.header_done = true;
        }
        self.pending.extend(payload.data);
        if !self.header_done {
            let Some((offset, spec, declared)) = wav_header(&self.pending)? else {
                ensure!(self.pending.len() <= 65536, "WAV header exceeds 64 KiB");
                return Ok(Vec::new());
            };
            validate_spec(spec)?;
            self.spec = Some(spec);
            self.declared_bytes = declared;
            self.pending.drain(..offset);
            self.header_done = true;
        }
        let channels = self.spec.context("missing PCM format")?.1;
        let block = channels * 2;
        let count = self.pending.len() / block * block;
        self.received_bytes = self
            .received_bytes
            .checked_add(count as u64)
            .context("WAV sample count overflow")?;
        ensure!(
            self.declared_bytes
                .is_none_or(|declared| self.received_bytes <= declared),
            "WAV data exceeds declared length"
        );
        let mut output = Vec::with_capacity(count / channels);
        for frame in self.pending[..count].chunks_exact(block) {
            let sum: i32 = frame
                .chunks_exact(2)
                .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as i32)
                .sum();
            output.extend_from_slice(&((sum / channels as i32) as i16).to_le_bytes());
        }
        self.pending.drain(..count);
        Ok(output)
    }

    fn finish(&self) -> Result<()> {
        ensure!(
            self.header_done && self.pending.is_empty(),
            "TTS ended inside WAV header or PCM sample frame"
        );
        ensure!(
            self.declared_bytes
                .is_none_or(|declared| self.received_bytes == declared),
            "TTS WAV ended before declared data length"
        );
        Ok(())
    }
}

fn validate_spec((sample_rate, channels): (u32, usize)) -> Result<()> {
    ensure!(
        (8000..=192000).contains(&sample_rate) && (1..=8).contains(&channels),
        "invalid TTS PCM sample rate or channels"
    );
    Ok(())
}

fn wav_header(bytes: &[u8]) -> Result<Option<(usize, (u32, usize), Option<u64>)>> {
    if bytes.len() < 12 {
        return Ok(None);
    }
    ensure!(
        &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "TTS WAV is missing RIFF/WAVE header"
    );
    let mut offset = 12usize;
    let mut spec = None;
    while offset + 8 <= bytes.len() {
        ensure!(offset <= 65536, "WAV header exceeds 64 KiB");
        let kind = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?);
        let start = offset + 8;
        if kind == b"data" {
            let spec = spec.context("WAV data arrived before PCM format")?;
            // GPT-SoVITS streaming WAVs use a zero-length placeholder; all
            // other finite declarations remain strict truncation boundaries.
            let declared = (size != 0 && size != u32::MAX).then_some(size as u64);
            return Ok(Some((start, spec, declared)));
        }
        let end = start
            .checked_add(size as usize)
            .context("WAV chunk overflow")?;
        if end > bytes.len() {
            return Ok(None);
        }
        if kind == b"fmt " {
            ensure!(
                size >= 16
                    && u16::from_le_bytes(bytes[start..start + 2].try_into()?) == 1
                    && u16::from_le_bytes(bytes[start + 14..start + 16].try_into()?) == 16,
                "expected PCM16 WAV"
            );
            let channels = u16::from_le_bytes(bytes[start + 2..start + 4].try_into()?) as usize;
            let sample_rate = u32::from_le_bytes(bytes[start + 4..start + 8].try_into()?);
            ensure!(
                u16::from_le_bytes(bytes[start + 12..start + 14].try_into()?) as usize
                    == channels * 2,
                "invalid WAV block alignment"
            );
            spec = Some((sample_rate, channels));
        }
        offset = end
            .checked_add(size as usize % 2)
            .context("WAV padding overflow")?;
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
