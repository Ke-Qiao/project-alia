use futures::StreamExt;
use tokio::sync::mpsc;
use tonic::transport::Channel;

use alia_proto::audio::AudioChunk;
use alia_proto::audio::audio_stream_client::AudioStreamClient;

use crate::channels::adapter::{AudioFormat, AudioPayload, AudioPayloadStream};
use crate::providers::tts::audio_budget::MAX_PCM_PAYLOAD_BYTES;

pub(super) const AUDIO_GRPC_CHUNK_BYTES: usize = 64 * 1024;
pub(super) const AUDIO_STREAM_GRPC_CHUNK_BYTES: usize = 8 * 1024;
pub(super) const AUDIO_STREAM_BACKPRESSURE_CHUNKS: usize = 8;
const GRPC_MESSAGE_LIMIT_BYTES: usize = 16 * 1024 * 1024;

pub(super) async fn produce_streaming_audio_chunks(
    first: AudioPayload,
    mut rest: AudioPayloadStream,
    tx: mpsc::Sender<AudioChunk>,
    encoding: i32,
) -> anyhow::Result<()> {
    let mut chunker = StreamingAudioChunker::new(tx, encoding, AUDIO_STREAM_GRPC_CHUNK_BYTES);
    let mut wav_decoder = StreamingWavPcmDecoder::default();

    process_stream_payload(first, &mut chunker, &mut wav_decoder).await?;
    while let Some(payload) = rest.next().await {
        process_stream_payload(
            normalize_embedded_playback_payload(payload?)?,
            &mut chunker,
            &mut wav_decoder,
        )
        .await?;
    }

    chunker.flush().await
}

async fn process_stream_payload(
    payload: AudioPayload,
    chunker: &mut StreamingAudioChunker,
    wav_decoder: &mut StreamingWavPcmDecoder,
) -> anyhow::Result<()> {
    match payload.format {
        AudioFormat::Wav => {
            let pcm = wav_decoder.push(&payload.data)?;
            if !pcm.is_empty() {
                chunker.push(&pcm).await?;
            }
        }
        AudioFormat::Pcm16 | AudioFormat::Opus => {
            chunker.push(&payload.data).await?;
        }
    }
    Ok(())
}

struct StreamingAudioChunker {
    tx: mpsc::Sender<AudioChunk>,
    encoding: i32,
    max_bytes: usize,
    sequence: u64,
    pending: Vec<u8>,
}

impl StreamingAudioChunker {
    fn new(tx: mpsc::Sender<AudioChunk>, encoding: i32, max_bytes: usize) -> Self {
        Self {
            tx,
            encoding,
            max_bytes,
            sequence: 0,
            pending: Vec::with_capacity(max_bytes),
        }
    }

    async fn push(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        for part in bytes.chunks(self.max_bytes) {
            let split = part.len().min(self.max_bytes - self.pending.len());
            self.pending.extend_from_slice(&part[..split]);
            if self.pending.len() == self.max_bytes {
                let chunk =
                    std::mem::replace(&mut self.pending, Vec::with_capacity(self.max_bytes));
                self.send(chunk).await?;
            }
            self.pending.extend_from_slice(&part[split..]);
        }
        Ok(())
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        if !self.pending.is_empty() {
            let chunk = std::mem::take(&mut self.pending);
            self.send(chunk).await?;
        }
        Ok(())
    }

    async fn send(&mut self, bytes: Vec<u8>) -> anyhow::Result<()> {
        let chunk = AudioChunk {
            opus_data: bytes,
            sequence: self.sequence,
            timestamp_us: chrono::Utc::now().timestamp_micros() as u64,
            encoding: self.encoding,
        };
        self.sequence += 1;
        self.tx
            .send(chunk)
            .await
            .map_err(|_| anyhow::anyhow!("embedded audio upload stream closed"))
    }
}

#[derive(Default)]
struct StreamingWavPcmDecoder {
    header: Vec<u8>,
    data_started: bool,
    pending_odd: Option<u8>,
}

impl StreamingWavPcmDecoder {
    fn push(&mut self, bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
        if self.data_started && bytes.starts_with(b"RIFF") {
            *self = Self::default();
        }

        anyhow::ensure!(
            bytes.len() <= MAX_PCM_PAYLOAD_BYTES,
            "decoder input exceeds 2 MiB"
        );
        anyhow::ensure!(
            self.header.len() <= MAX_PCM_PAYLOAD_BYTES - bytes.len(),
            "decoder workspace exceeds 2 MiB"
        );
        anyhow::ensure!(
            self.header.len() + bytes.len() + usize::from(self.pending_odd.is_some())
                <= MAX_PCM_PAYLOAD_BYTES,
            "decoder sample workspace exceeds 2 MiB"
        );
        let mut pcm = if self.data_started {
            bytes.to_vec()
        } else {
            self.header.try_reserve_exact(bytes.len())?;
            self.header.extend_from_slice(bytes);
            let Some(data_start) = wav_data_start(&self.header)? else {
                return Ok(Vec::new());
            };
            let pcm = self.header[data_start..].to_vec();
            self.header.clear();
            self.data_started = true;
            pcm
        };

        if let Some(odd) = self.pending_odd.take() {
            let mut with_odd = Vec::with_capacity(pcm.len() + 1);
            with_odd.push(odd);
            with_odd.extend_from_slice(&pcm);
            pcm = with_odd;
        }
        if pcm.len() % 2 != 0 {
            self.pending_odd = pcm.pop();
        }

        Ok(pcm)
    }
}

fn wav_data_start(buffer: &[u8]) -> anyhow::Result<Option<usize>> {
    if buffer.len() < 12 {
        return Ok(None);
    }
    if &buffer[0..4] != b"RIFF" || &buffer[8..12] != b"WAVE" {
        anyhow::bail!("streaming WAV payload missing RIFF/WAVE header");
    }

    let mut pos = 12usize;
    while pos + 8 <= buffer.len() {
        let chunk_id = &buffer[pos..pos + 4];
        let size = u32::from_le_bytes(buffer[pos + 4..pos + 8].try_into()?) as usize;
        let data_start = pos + 8;
        if chunk_id == b"data" {
            return Ok(Some(data_start));
        }
        let padded = size + (size % 2);
        let next = data_start.saturating_add(padded);
        if next > buffer.len() {
            return Ok(None);
        }
        pos = next;
    }

    if buffer.len() > 128 * 1024 {
        anyhow::bail!("streaming WAV header exceeded 128 KiB without data chunk");
    }
    Ok(None)
}

pub(super) fn configure_audio_client(
    client: AudioStreamClient<Channel>,
) -> AudioStreamClient<Channel> {
    client
        .max_encoding_message_size(GRPC_MESSAGE_LIMIT_BYTES)
        .max_decoding_message_size(GRPC_MESSAGE_LIMIT_BYTES)
}

pub fn normalize_embedded_playback_payload(audio: AudioPayload) -> anyhow::Result<AudioPayload> {
    anyhow::ensure!(
        audio.data.len() <= MAX_PCM_PAYLOAD_BYTES,
        "decoder input exceeds 2 MiB"
    );
    match audio.format {
        AudioFormat::Wav => {
            if let Some((pcm, sample_rate)) = pcm16_from_wav(&audio.data)? {
                Ok(AudioPayload {
                    data: audio.data.derived_pcm(pcm16_to_le_bytes(&pcm))?,
                    sample_rate,
                    channels: 1,
                    format: AudioFormat::Pcm16,
                })
            } else {
                Ok(AudioPayload {
                    format: AudioFormat::Pcm16,
                    ..audio
                })
            }
        }
        AudioFormat::Pcm16 if audio.channels > 1 => {
            let pcm = pcm16le_bytes_to_samples(&audio.data)?;
            let mono = downmix_pcm16(&pcm, audio.channels as usize);
            Ok(AudioPayload {
                data: audio.data.derived_pcm(pcm16_to_le_bytes(&mono))?,
                channels: 1,
                ..audio
            })
        }
        _ => Ok(audio),
    }
}

fn pcm16_from_wav(bytes: &[u8]) -> anyhow::Result<Option<(Vec<i16>, u32)>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Ok(None);
    }

    let mut offset = 12;
    let mut sample_rate = None;
    let mut channels = None;
    let mut bits_per_sample = None;
    let mut audio_format = None;
    let mut pcm_bytes = None;

    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        offset += 8;
        if offset + size > bytes.len() {
            anyhow::bail!("malformed WAV chunk");
        }

        match id {
            b"fmt " => {
                if size < 16 {
                    anyhow::bail!("invalid WAV fmt chunk");
                }
                audio_format = Some(u16::from_le_bytes(bytes[offset..offset + 2].try_into()?));
                channels = Some(u16::from_le_bytes(
                    bytes[offset + 2..offset + 4].try_into()?,
                ));
                sample_rate = Some(u32::from_le_bytes(
                    bytes[offset + 4..offset + 8].try_into()?,
                ));
                bits_per_sample = Some(u16::from_le_bytes(
                    bytes[offset + 14..offset + 16].try_into()?,
                ));
            }
            b"data" => pcm_bytes = Some(&bytes[offset..offset + size]),
            _ => {}
        }

        offset += size + (size % 2);
    }

    if audio_format != Some(1) || bits_per_sample != Some(16) {
        anyhow::bail!("expected 16-bit PCM WAV");
    }

    let channels = channels.unwrap_or(1).max(1) as usize;
    let sample_rate = sample_rate.unwrap_or(24_000);
    let samples: Vec<i16> = pcm_bytes
        .ok_or_else(|| anyhow::anyhow!("missing WAV data chunk"))?
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    if channels == 1 {
        return Ok(Some((samples, sample_rate)));
    }

    Ok(Some((
        samples
            .chunks_exact(channels)
            .map(|frame| frame[0])
            .collect(),
        sample_rate,
    )))
}

pub fn pcm16_to_le_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

pub fn pcm16le_bytes_to_samples(bytes: &[u8]) -> anyhow::Result<Vec<i16>> {
    if bytes.len() % 2 != 0 {
        anyhow::bail!("PCM16LE byte payload must have even length");
    }

    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
}

fn downmix_pcm16(samples: &[i16], channels: usize) -> Vec<i16> {
    if channels <= 1 {
        return samples.to_vec();
    }

    samples
        .chunks_exact(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|sample| i32::from(*sample)).sum();
            (sum / frame.len() as i32) as i16
        })
        .collect()
}
