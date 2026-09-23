//! Identity-preserving delivery. Start/Audio are gated by durable observer writes;
//! End keeps receipts open until an explicit observation end.
use crate::channels::adapter::{
    AudioDeliveryReport, AudioFormat, AudioPayload, AudioPayloadStream, PlaybackObserver,
};
use alia_core::delivery::{
    ObservationEnd, PLAYBACK_CONTRACT_VERSION, PlaybackIdentity, PlaybackProgress,
    SubmissionBackend, SubmissionBoundary, VoiceScope,
};
use alia_proto::playback::{self as proto, playback_client::PlaybackClient, playback_frame};
use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;

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
    ensure!(
        receipt.playback_id.is_none()
            && receipt.playback_sequence.is_none()
            && receipt.final_progress.is_none(),
        "scope fence returned an exact-playback receipt"
    );
    let same = receipt.active_scope.as_ref().is_some_and(|s| {
        s.device_boot_id == scope.device_boot_id.to_string()
            && s.session_id == scope.session_id.to_string()
    });
    if close_session {
        ensure!(!same, "device did not retire the requested voice session");
    } else {
        ensure!(
            !receipt.session_closed
                && same
                && receipt
                    .active_scope
                    .as_ref()
                    .is_some_and(|s| s.generation_epoch >= scope.generation_epoch),
            "device did not establish the requested playback fence"
        );
    }
    Ok(())
}
pub(super) async fn cancel_exact(
    mut client: PlaybackClient<Channel>,
    identity: PlaybackIdentity,
    auth: tonic::metadata::MetadataMap,
    reason: &str,
) -> Result<PlaybackProgress> {
    let mut request = tonic::Request::new(proto::FenceRequest {
        scope: Some(encode_scope(identity.scope)),
        reason: reason.into(),
        close_session: false,
        playback_id: Some(identity.playback_id.to_string()),
        playback_sequence: Some(identity.playback_sequence),
    });
    *request.metadata_mut() = auth;
    let receipt = tokio::time::timeout(Duration::from_secs(2), client.fence(request))
        .await
        .context("precise cancellation deadline")??
        .into_inner();
    ensure!(
        receipt.applied
            && receipt.playback_id.as_deref() == Some(identity.playback_id.to_string().as_str())
            && receipt.playback_sequence == Some(identity.playback_sequence),
        "device did not confirm exact playback cancellation"
    );
    // The active voice session may have changed while an entered write settled.
    // Exact final evidence is scoped by its immutable playback identity below.
    let progress = decode_receipt(
        identity,
        receipt
            .final_progress
            .context("precise cancellation lacks settled evidence")?,
    )?;
    progress.validate(None, progress.accepted_source_frames)?;
    ensure!(
        progress.observation_end.is_some()
            && progress.submitted_source_frames.is_some()
            && progress.backend.is_some(),
        "precise cancellation has no settled backend observation"
    );
    Ok(progress)
}

pub(super) async fn push(
    mut client: PlaybackClient<Channel>,
    identity: PlaybackIdentity,
    binding: alia_core::output_control::OutputBinding,
    audio: AudioPayloadStream,
    auth: tonic::metadata::MetadataMap,
    observer: Arc<dyn PlaybackObserver>,
    cancel: CancellationToken,
    idle: Duration,
) -> Result<AudioDeliveryReport> {
    ensure!(identity.is_valid(), "invalid voice playback identity");
    ensure!(
        binding.is_valid() && binding.scope == identity.scope,
        "playback differs from canonical output"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let cancel_client = client.clone();
    let cancel_auth = auth.clone();
    let (tx, rx) = mpsc::channel(8);
    let (ack_tx, ack_rx) = oneshot::channel();
    let (start_tx, start_rx) = oneshot::channel();
    let offered = Arc::new(AtomicU64::new(0));
    let end_offered = Arc::new(AtomicBool::new(false));
    let mut last: Option<PlaybackProgress> = None;
    let child = cancel.child_token();
    let mut producer = Box::pin(produce(
        audio,
        tx,
        identity,
        binding,
        offered.clone(),
        observer.clone(),
        ack_rx,
        start_tx,
        child.clone(),
        end_offered.clone(),
    ));
    let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
    *request.metadata_mut() = auth;
    let receiving = async {
        // A reversible hold may retain the first provider payload. Start the
        // transport's handshake clock only once Start is actually available.
        start_rx.await.context("playback ended before Start was prepared")?;
        let mut receipts = tokio::time::timeout(idle, client.push(request))
            .await
            .context("playback response handshake timeout")??
            .into_inner();
        let mut handshake = Some(ack_tx);
        loop {
            let receipt = tokio::time::timeout(idle, receipts.message())
                .await
                .context("playback receipt idle timeout")??
                .context("playback receipt stream ended without terminal evidence")?;
            let progress = decode_receipt(identity, receipt)?;
            ensure!(
                !progress.end_accepted || end_offered.load(Ordering::Acquire),
                "device acknowledged End before it was sent"
            );
            progress.validate(last.as_ref(), offered.load(Ordering::Acquire))?;
            if handshake.is_some() {
                ensure!(
                    progress.receipt_sequence == 1
                        && progress.accepted_source_frames == 0
                        && !progress.end_accepted
                        && progress.observation_end.is_none(),
                    "invalid playback v2 handshake acknowledgement"
                );
            }
            observer.progress(progress.clone()).await?;
            let terminal = progress.observation_end.clone();
            last = Some(progress);
            if let Some(ack) = handshake.take() {
                let _ = ack.send(());
            }
            if let Some(end) = terminal {
                return Ok::<_, anyhow::Error>(end);
            }
        }
    };
    let transfer = async {
        tokio::pin!(receiving);
        let mut produced = false;
        loop {
            tokio::select! {
                result = &mut producer, if !produced => { result?; produced = true; },
                result = &mut receiving => return result,
            }
        }
    };
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(anyhow::anyhow!("delivery cancelled")),
        _ = tokio::time::sleep_until(deadline) => Err(anyhow::anyhow!("absolute delivery observation deadline")),
        result = transfer => result,
    };
    // Both upload and receipt futures are local to this owned Delivery future.
    // Cancelling drops them directly; there are no detached producer tasks.
    child.cancel();
    drop(producer);
    match result {
        Ok(observation_end) => Ok(AudioDeliveryReport {
            progress: last,
            observation_end,
        }),
        Err(error) => {
            let reason = error.to_string();
            let confirmed = cancel_exact(cancel_client, identity, cancel_auth, &reason).await;
            let end;
            match confirmed {
                Ok(progress) => {
                    progress.validate(last.as_ref(), offered.load(Ordering::Acquire))?;
                    end = progress
                        .observation_end
                        .clone()
                        .context("missing final cancellation evidence")?;
                    observer.progress(progress.clone()).await?;
                    last = Some(progress);
                }
                Err(_) => {
                    // Observation loss is a local report, not a device receipt.
                    // Preserve its last evidence and sequence for a later exact
                    // cancellation checkpoint owned by the Delivery supervisor.
                    end = ObservationEnd::Unknown { reason };
                }
            }
            Ok(AudioDeliveryReport {
                progress: last,
                observation_end: end,
            })
        }
    }
}

fn decode_receipt(
    identity: PlaybackIdentity,
    receipt: proto::PlaybackReceipt,
) -> Result<PlaybackProgress> {
    ensure!(
        receipt.contract_version == PLAYBACK_CONTRACT_VERSION,
        "unsupported playback evidence contract"
    );
    ensure!(
        receipt.scope.as_ref() == Some(&encode_scope(identity.scope))
            && receipt.playback_id == identity.playback_id.to_string()
            && receipt.playback_sequence == identity.playback_sequence,
        "playback receipt identity mismatch"
    );
    ensure!(
        receipt.presented_samples.is_none(),
        "device presentation clocks are not implemented"
    );
    let observation_end = match proto::ObservationEnd::try_from(receipt.observation_end)? {
        proto::ObservationEnd::Unspecified => None,
        proto::ObservationEnd::SubmissionFinished => Some(ObservationEnd::SubmissionFinished),
        proto::ObservationEnd::Cancelled => Some(ObservationEnd::Cancelled {
            reason: receipt.reason,
        }),
        proto::ObservationEnd::Failed => Some(ObservationEnd::Failed {
            reason: receipt.reason,
        }),
        proto::ObservationEnd::Unknown => Some(ObservationEnd::Unknown {
            reason: receipt.reason,
        }),
        proto::ObservationEnd::Rejected => Some(ObservationEnd::Rejected {
            reason: receipt.reason,
        }),
        proto::ObservationEnd::PresentedFinished => {
            anyhow::bail!("unsupported presentation evidence")
        }
    };
    let state = proto::PlaybackState::try_from(receipt.state)?;
    ensure!(
        match &observation_end {
            None => matches!(
                state,
                proto::PlaybackState::Accepted | proto::PlaybackState::Submitted
            ),
            Some(ObservationEnd::SubmissionFinished) => state == proto::PlaybackState::Submitted,
            Some(ObservationEnd::Cancelled { .. }) => state == proto::PlaybackState::Cancelled,
            Some(ObservationEnd::Failed { .. }) => state == proto::PlaybackState::Failed,
            Some(ObservationEnd::Unknown { .. }) => state == proto::PlaybackState::Unknown,
            Some(ObservationEnd::Rejected { .. }) => state == proto::PlaybackState::Rejected,
            _ => false,
        },
        "playback state contradicts observation end"
    );
    let backend = receipt
        .backend
        .map(|b| {
            Ok::<_, anyhow::Error>(SubmissionBackend {
                instance_id: b.instance_id.parse()?,
                boundary: match proto::SubmissionBoundary::try_from(b.boundary)? {
                    proto::SubmissionBoundary::CpalCallbackBuffer => {
                        SubmissionBoundary::CpalCallbackBuffer
                    }
                    proto::SubmissionBoundary::AlsaWriteAccepted => {
                        SubmissionBoundary::AlsaWriteAccepted
                    }
                    _ => anyhow::bail!("missing submission boundary"),
                },
                output_sample_rate_hz: b.output_sample_rate_hz,
                output_channels: b.output_channels,
            })
        })
        .transpose()?;
    Ok(PlaybackProgress {
        identity,
        receipt_sequence: receipt.receipt_sequence,
        accepted_source_frames: receipt.accepted_samples,
        submitted_source_frames: receipt.submitted_samples,
        presented_source_frames: None,
        end_accepted: receipt.end_accepted,
        backend,
        observation_end,
    })
}

async fn produce(
    mut audio: AudioPayloadStream,
    tx: mpsc::Sender<proto::PlaybackFrame>,
    identity: PlaybackIdentity,
    binding: alia_core::output_control::OutputBinding,
    offered: Arc<AtomicU64>,
    observer: Arc<dyn PlaybackObserver>,
    handshake: oneshot::Receiver<()>,
    start: oneshot::Sender<()>,
    cancel: CancellationToken,
    end_offered: Arc<AtomicBool>,
) -> Result<u64> {
    let mut decoder = PcmDecoder::default();
    let mut handshake = Some(handshake);
    let mut start = Some(start);
    let mut sequence = 0_u64;
    let mut samples = 0_u64;
    let frame = |payload| proto::PlaybackFrame {
        scope: Some(encode_scope(identity.scope)),
        playback_id: identity.playback_id.to_string(),
        playback_sequence: identity.playback_sequence,
        payload: Some(payload),
    };
    while let Some(payload) = tokio::select! { _ = cancel.cancelled() => return Err(anyhow::anyhow!("audio producer cancelled")), payload = audio.next() => payload }
    {
        let pcm = decoder.push(payload?)?;
        let Some((sample_rate, _)) = decoder.spec else {
            continue;
        };
        if let Some(ack) = handshake.take() {
            observer.start(sample_rate, 1).await?;
            tx.send(frame(playback_frame::Payload::Start(
                proto::PlaybackStart {
                    output_binding: Some(alia_proto::output_control::encode_binding(binding)),
                    contract_version: PLAYBACK_CONTRACT_VERSION,
                    sample_rate_hz: sample_rate,
                    channels: 1,
                    source: "tts".into(),
                },
            )))
            .await
            .context("playback Start stream closed")?;
            let _ = start.take().context("playback Start already prepared")?.send(());
            ack.await
                .context("playback handshake was not acknowledged")?;
        }
        for bytes in pcm.chunks(sample_rate as usize / 10 * 2) {
            let next = samples
                .checked_add(bytes.len() as u64 / 2)
                .context("playback source clock overflow")?;
            ensure!(
                next <= u64::from(sample_rate) * 120,
                "playback source exceeds 120 seconds"
            );
            observer.offered(next).await?;
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
            sequence += 1;
        }
    }
    decoder.finish()?;
    ensure!(handshake.is_none(), "TTS returned no audio format");
    end_offered.store(true, Ordering::Release);
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
        // This is the explicit decoder workspace boundary. Its PCM output and
        // protobuf queue have separate caps; the encoded lease ends only when
        // the source AudioBuffer is dropped, never through an implicit clone.
        const WORKSPACE_BYTES: usize = crate::providers::tts::audio_budget::MAX_PCM_PAYLOAD_BYTES;
        ensure!(
            payload.data.len() <= WORKSPACE_BYTES,
            "decoder input exceeds 2 MiB"
        );
        ensure!(
            self.pending.len() <= WORKSPACE_BYTES - payload.data.len(),
            "decoder workspace exceeds 2 MiB"
        );
        self.pending.try_reserve_exact(payload.data.len())?;
        self.pending.extend_from_slice(&payload.data);
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
