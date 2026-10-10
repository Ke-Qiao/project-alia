#![cfg(feature = "channel-embedded")]
use alia_server::channels::embedded::{
    normalize_embedded_playback_payload, pcm16_to_le_bytes, pcm16le_bytes_to_samples,
};
use alia_server::modules::delivery::audio::AudioBuffer;
use alia_server::modules::delivery::transport::{AudioFormat, AudioPayload};

#[test]
fn normalizes_stereo_pcm16_payload_to_mono() {
    let payload = AudioPayload {
        data: AudioBuffer::input(pcm16_to_le_bytes(&[100, 300, -200, -400])),
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

#[tokio::test]
async fn normalized_pcm_keeps_encoded_credit_until_the_last_derived_clone() {
    use alia_server::modules::delivery::audio::ResponseAudioBudget;
    let budget = ResponseAudioBudget::for_response(uuid::Uuid::new_v4()).unwrap();
    let source = pcm16_to_le_bytes(&[100, 300, -200, -400]);
    let encoded = budget.copy_from_slice(&source).await.unwrap();
    let normalized = normalize_embedded_playback_payload(AudioPayload {
        data: encoded.into_buffer(),
        sample_rate: 24_000,
        channels: 2,
        format: AudioFormat::Pcm16,
    })
    .unwrap();
    let held = normalized.clone();
    drop(normalized);
    assert_eq!(budget.reserved_bytes(), source.len());
    assert_eq!(pcm16le_bytes_to_samples(&held.data).unwrap(), [200, -300]);
    drop(held);
    assert_eq!(budget.reserved_bytes(), 0);
}

#[test]
fn normalization_rejects_oversized_pcm_before_conversion() {
    use alia_server::modules::delivery::audio::MAX_PCM_PAYLOAD_BYTES;
    let payload = AudioPayload {
        data: AudioBuffer::input(vec![0; MAX_PCM_PAYLOAD_BYTES + 1]),
        sample_rate: 24_000,
        channels: 2,
        format: AudioFormat::Pcm16,
    };
    assert!(
        normalize_embedded_playback_payload(payload)
            .unwrap_err()
            .to_string()
            .contains("decoder input exceeds")
    );
}
