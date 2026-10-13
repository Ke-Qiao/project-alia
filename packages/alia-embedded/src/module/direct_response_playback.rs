use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use alia_core::delivery::{ObservationEnd, VoiceScope};
use anyhow::{Context, Result, bail, ensure};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::module::context::{EventBus, ModuleEvent};
use crate::module::playback_session::{PlaybackLease, PlaybackSession};
use crate::module::traits::DirectResponseAction;

const MAX_CUE_SECONDS: usize = 5;
const MAX_WAV_BYTES: usize = 4 * 1024 * 1024;

struct CueGuard {
    playback: Arc<PlaybackSession>,
    lease: PlaybackLease,
    events: EventBus,
}
impl Drop for CueGuard {
    fn drop(&mut self) {
        let _ = self
            .playback
            .cancel_cue(&self.lease, "local cue worker ended");
        self.events.publish(ModuleEvent::PlaybackFinished {
            source: "local-cue".into(),
        });
    }
}

/// A cue may only use the original Ready scope. WAV I/O never creates a new
/// playback owner; late file completion after cancellation or replacement is
/// rejected by both this token and PlaybackSession's scoped admission.
pub async fn play_direct_response(
    action: DirectResponseAction,
    playback: Arc<PlaybackSession>,
    scope: VoiceScope,
    event_bus: EventBus,
    cancel: CancellationToken,
) -> bool {
    let Some(audio_path) = action.audio_path.as_deref() else {
        return false;
    };
    if cancel.is_cancelled() {
        return false;
    }
    let wav = match read_pcm16_wav(Path::new(audio_path)).await {
        Ok(wav) => wav,
        Err(error) => {
            warn!(%error, "local cue WAV rejected");
            return false;
        }
    };
    if cancel.is_cancelled() {
        return false;
    }
    let lease = match playback.start_cue(&scope, wav.sample_rate) {
        Ok(lease) => lease,
        Err(error) => {
            debug!(%error, "local cue scope is unavailable");
            return false;
        }
    };
    let guard = CueGuard {
        playback: playback.clone(),
        lease: lease.clone(),
        events: event_bus.clone(),
    };
    event_bus.publish(ModuleEvent::PlaybackStarted {
        source: "local-cue".into(),
    });
    let task_cancel = cancel.clone();
    let producer = tokio::task::spawn_blocking(move || -> Result<bool> {
        for chunk in wav.pcm.chunks(4096) {
            if task_cancel.is_cancelled()
                || lease.submission.is_held()
                || !playback.enqueue_cue(&lease, chunk)?
            {
                return Ok(false);
            }
        }
        playback.end_cue(&lease)
    });
    let mut producer = producer;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(7);
    let admitted = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            let _ = guard.playback.cancel_cue(&guard.lease, "local cue cancelled");
            let _ = producer.await;
            return false;
        }
        _ = tokio::time::sleep_until(deadline) => {
            let _ = guard.playback.cancel_cue(&guard.lease, "local cue submission timed out");
            let _ = producer.await;
            return false;
        }
        result = &mut producer => matches!(result, Ok(Ok(true))),
    };
    if !admitted {
        return false;
    }
    // The native submission receipt, rather than a duration estimate, ends the
    // event. A cue is ephemeral: a hold suppresses its remaining tail instead
    // of emitting a stale notification after the conversation resumes.
    let result = tokio::time::timeout_at(deadline, async {
        loop {
            if guard.lease.submission.is_held() {
                return false;
            }
            if let Some(end) = guard.lease.submission.snapshot(0).observation_end {
                return matches!(end, ObservationEnd::SubmissionFinished);
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return false,
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    })
    .await
    .unwrap_or(false);
    drop(guard);
    result
}

#[derive(Debug)]
pub struct DecodedPcmWav {
    pub pcm: Vec<i16>,
    pub sample_rate: u32,
    pub source_channels: u16,
}

async fn read_pcm16_wav(path: &Path) -> Result<DecodedPcmWav> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || read_pcm16_wav_blocking(&path))
        .await
        .context("joining WAV reader task")?
}

pub fn read_pcm16_wav_blocking(path: &Path) -> Result<DecodedPcmWav> {
    ensure!(
        std::fs::metadata(path)?.is_file(),
        "cue WAV must be a regular file"
    );
    let file = std::fs::File::open(path).context("opening cue WAV")?;
    ensure!(
        file.metadata()?.len() <= MAX_WAV_BYTES as u64,
        "cue WAV exceeds 4 MiB"
    );
    let mut data = Vec::new();
    file.take((MAX_WAV_BYTES + 1) as u64)
        .read_to_end(&mut data)
        .context("reading cue WAV")?;
    ensure!(data.len() <= MAX_WAV_BYTES, "cue WAV exceeds 4 MiB");
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        bail!("{} is not a RIFF/WAVE file", path.display());
    }

    let mut offset = 12;
    let mut sample_rate = None;
    let mut channels = None;
    let mut bits_per_sample = None;
    let mut audio_format = None;
    let mut pcm_bytes = None;

    while offset + 8 <= data.len() {
        let id = &data[offset..offset + 4];
        let size = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        offset += 8;
        if offset + size > data.len() {
            bail!("malformed WAV chunk in {}", path.display());
        }

        match id {
            b"fmt " => {
                if size < 16 {
                    bail!("invalid WAV fmt chunk in {}", path.display());
                }
                audio_format = Some(u16::from_le_bytes(
                    data[offset..offset + 2].try_into().unwrap(),
                ));
                channels = Some(u16::from_le_bytes(
                    data[offset + 2..offset + 4].try_into().unwrap(),
                ));
                sample_rate = Some(u32::from_le_bytes(
                    data[offset + 4..offset + 8].try_into().unwrap(),
                ));
                bits_per_sample = Some(u16::from_le_bytes(
                    data[offset + 14..offset + 16].try_into().unwrap(),
                ));
            }
            b"data" => {
                pcm_bytes = Some(&data[offset..offset + size]);
            }
            _ => {}
        }

        offset += size + (size % 2);
    }

    if audio_format != Some(1) {
        bail!("{} must be PCM WAV", path.display());
    }
    if bits_per_sample != Some(16) {
        bail!("{} must be 16-bit PCM WAV", path.display());
    }

    let channels = channels.context("missing WAV channel count")?;
    let sample_rate = sample_rate.context("missing WAV sample rate")?;
    let pcm_bytes = pcm_bytes.context("missing WAV data chunk")?;
    ensure!((1..=32).contains(&channels), "invalid cue channel count");
    ensure!(
        (8000..=192000).contains(&sample_rate),
        "invalid cue sample rate"
    );
    ensure!(
        pcm_bytes.len() % (usize::from(channels) * 2) == 0,
        "incomplete cue PCM frame"
    );
    let frames = pcm_bytes.len() / (usize::from(channels) * 2);
    ensure!(
        frames > 0 && frames <= sample_rate as usize * MAX_CUE_SECONDS,
        "cue must contain at most five seconds of audio"
    );
    let pcm = pcm_bytes
        .chunks_exact(usize::from(channels) * 2)
        .map(|frame| {
            let sum: i32 = frame
                .chunks_exact(2)
                .map(|sample| i32::from(i16::from_le_bytes([sample[0], sample[1]])))
                .sum();
            (sum / i32::from(channels)) as i16
        })
        .collect();
    Ok(DecodedPcmWav {
        pcm,
        sample_rate,
        source_channels: channels,
    })
}
