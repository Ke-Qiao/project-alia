use crate::modules::delivery::audio::MAX_PCM_PAYLOAD_BYTES;
use crate::modules::delivery::transport::{AudioFormat, AudioPayload};

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
