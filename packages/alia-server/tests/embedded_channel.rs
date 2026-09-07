use alia_server::channels::adapter::{AudioFormat, AudioPayload};
use alia_server::channels::embedded::{
    normalize_embedded_playback_payload, pcm16_to_le_bytes, pcm16le_bytes_to_samples,
};

#[test]
fn normalizes_stereo_pcm16_payload_to_mono() {
    let payload = AudioPayload {
        data: pcm16_to_le_bytes(&[100, 300, -200, -400]),
        sample_rate: 24_000,
        channels: 2,
        format: AudioFormat::Pcm16,
    };

    let normalized = normalize_embedded_playback_payload(payload).unwrap();

    assert_eq!(normalized.sample_rate, 24_000);
    assert_eq!(normalized.channels, 1);
    assert_eq!(normalized.format, AudioFormat::Pcm16);
    assert_eq!(
        pcm16le_bytes_to_samples(&normalized.data).unwrap(),
        vec![200, -300]
    );
}
#![cfg(feature = "channel-embedded")]
