use std::pin::Pin;

use alia_server::expression::{ProsodyControls, SpeechExpression};
use alia_server::modules::platform::lifecycle::ModuleHealth;
use alia_server::modules::platform::lifecycle::ModuleLifecycle;
use alia_server::providers::adapter::{
    AudioEncoding, SttAdapter, TranscribeChunk, TranscriptionResult, TtsCapabilities,
    TtsLatencyClass, TtsProsodyLimits, TtsRequest,
};
use async_trait::async_trait;
use futures::Stream;

#[cfg(feature = "stt-whisper")]
use alia_server::providers::stt::WhisperCppStt;

struct MinimalStt;

#[async_trait]
impl ModuleLifecycle for MinimalStt {
    fn name(&self) -> &'static str {
        "minimal_stt"
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}

#[async_trait]
impl SttAdapter for MinimalStt {
    async fn transcribe_result(
        &self,
        _audio: &[u8],
        _encoding: AudioEncoding,
    ) -> anyhow::Result<TranscriptionResult> {
        Ok(TranscriptionResult::plain(
            self.name(),
            "hello from audio".to_string(),
        ))
    }

    async fn transcribe_stream(
        &self,
        _audio_stream: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
        _encoding: AudioEncoding,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<TranscribeChunk>> + Send>>> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

#[tokio::test]
async fn stt_minimal_result_and_capabilities_are_provider_neutral() {
    let stt = MinimalStt;

    let result = stt
        .transcribe_result(&[0, 1, 2], AudioEncoding::Pcm16kMono)
        .await
        .expect("minimal STT should return a provider-neutral result");
    let capabilities = stt.stt_capabilities().await;

    assert_eq!(result.text, "hello from audio");
    assert_eq!(result.provider, "minimal_stt");
    assert!(capabilities.supported_input_encodings.is_empty());
    assert!(!capabilities.supports_streaming);
    assert!(!capabilities.supports_partial_results);
    assert!(!capabilities.supports_segments);
    assert!(!capabilities.supports_alternatives);
}

#[cfg(feature = "stt-whisper")]
#[tokio::test]
async fn whisper_capabilities_match_verbose_json_metadata() {
    let stt = WhisperCppStt::new(&alia_server::config::WhisperCppProviderSection::default());
    let capabilities = stt.stt_capabilities().await;

    assert_eq!(capabilities.provider, "whisper");
    assert!(
        capabilities
            .supported_input_encodings
            .contains(&AudioEncoding::Pcm16kMono)
    );
    assert!(capabilities.supports_streaming);
    assert!(capabilities.supports_partial_results);
    assert!(!capabilities.supports_confidence);
    assert!(capabilities.supports_word_confidence);
    assert!(capabilities.supports_segment_quality);
    assert!(capabilities.supports_segments);
    assert!(capabilities.supports_language);
    assert!(capabilities.supports_language_hint);
    assert!(capabilities.supports_timestamps);
    assert!(capabilities.supports_punctuation);
    assert!(!capabilities.supports_alternatives);
}

#[test]
fn tts_request_preserves_provider_neutral_expression_metadata() {
    let speech = SpeechExpression {
        style: "gentle".to_string(),
        intensity: 0.42,
        voice_profile: Some("alia_default".to_string()),
        prosody: ProsodyControls {
            rate: 0.92,
            pitch_shift: 0.03,
            energy: 0.38,
            pause_bias: 0.15,
        },
    };

    let request = TtsRequest::with_speech("hello", &speech);

    assert_eq!(request.text, "hello");
    assert_eq!(request.style.as_deref(), Some("gentle"));
    assert_eq!(request.intensity, 0.42);
    assert_eq!(request.voice_profile.as_deref(), Some("alia_default"));
    assert_eq!(request.prosody.rate, 0.92);
    assert_eq!(request.prosody.pitch_shift, 0.03);
    assert_eq!(request.prosody.energy, 0.38);
    assert_eq!(request.prosody.pause_bias, 0.15);
}

#[test]
fn tts_capabilities_default_to_no_expression_controls() {
    let capabilities = TtsCapabilities::minimal("plain");

    assert_eq!(capabilities.provider, "plain");
    assert!(capabilities.supported_output_formats.is_empty());
    assert_eq!(capabilities.latency_class, TtsLatencyClass::Unknown);
    assert!(!capabilities.supports_pitch);
    assert_eq!(capabilities.prosody_limits.max_rate, 1.0);
}

#[test]
fn minimal_tts_capabilities_report_expression_degradations() {
    let capabilities = TtsCapabilities::minimal("plain");
    let request = TtsRequest::with_speech(
        "hello",
        &SpeechExpression {
            style: "cheerful".to_string(),
            intensity: 0.8,
            voice_profile: Some("alia_default".to_string()),
            prosody: ProsodyControls {
                rate: 1.1,
                pitch_shift: 0.2,
                energy: 0.8,
                pause_bias: 0.3,
            },
        },
    );

    let reasons = capabilities
        .provider_adjustments(&request)
        .into_iter()
        .map(|adjustment| adjustment.reason)
        .collect::<Vec<_>>();

    assert!(reasons.contains(&"provider_style_control_unavailable".to_string()));
    assert!(reasons.contains(&"provider_intensity_control_unavailable".to_string()));
    assert!(reasons.contains(&"provider_reference_voice_unavailable".to_string()));
    assert!(reasons.contains(&"provider_rate_control_unavailable".to_string()));
    assert!(reasons.contains(&"provider_pitch_control_unavailable".to_string()));
    assert!(reasons.contains(&"provider_energy_control_unavailable".to_string()));
    assert!(reasons.contains(&"provider_pause_control_unavailable".to_string()));
}

#[test]
fn tts_capabilities_report_limits_and_text_bounds() {
    let mut capabilities = TtsCapabilities::minimal("bounded");
    capabilities.supports_intensity = true;
    capabilities.supports_rate = true;
    capabilities.supports_energy = true;
    capabilities.supports_pause = true;
    capabilities.prosody_limits = TtsProsodyLimits {
        min_rate: 0.8,
        max_rate: 1.2,
        min_pitch_shift: 0.0,
        max_pitch_shift: 0.0,
        min_energy: 0.2,
        max_energy: 0.7,
        min_pause_bias: -0.2,
        max_pause_bias: 0.2,
    };
    capabilities.max_text_chars = Some(4);

    let request = TtsRequest {
        text: "hello".to_string(),
        style: None,
        intensity: 0.5,
        voice_profile: None,
        prosody: ProsodyControls {
            rate: 1.4,
            pitch_shift: 0.2,
            energy: 0.9,
            pause_bias: -0.5,
        },
    };

    let reasons = capabilities
        .provider_adjustments(&request)
        .into_iter()
        .map(|adjustment| adjustment.reason)
        .collect::<Vec<_>>();

    assert!(reasons.contains(&"provider_text_length_limit_exceeded".to_string()));
    assert!(reasons.contains(&"provider_rate_limit_clamped".to_string()));
    assert!(reasons.contains(&"provider_pitch_control_unavailable".to_string()));
    assert!(reasons.contains(&"provider_energy_limit_clamped".to_string()));
    assert!(reasons.contains(&"provider_pause_limit_clamped".to_string()));
}
