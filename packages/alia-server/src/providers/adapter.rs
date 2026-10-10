use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::expression::{ProsodyControls, SpeechExpression};
use crate::modules::delivery::audio::{EncodedAudio, ResponseAudioBudget};
use crate::modules::platform::lifecycle::ModuleLifecycle;

// ========== Common types ==========

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmToolCall {
    pub id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmResponse {
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<LlmToolCall>,
}

impl LlmResponse {
    pub fn text(content: String) -> Self {
        Self {
            content,
            tool_calls: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCapabilities {
    pub provider: String,
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub supports_tool_calling: bool,
    #[serde(default)]
    pub supports_streaming_tool_calls: bool,
    #[serde(default)]
    pub supports_system_messages: bool,
    #[serde(default)]
    pub supports_json_mode: bool,
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub supports_parallel_tool_calls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_specific: BTreeMap<String, serde_json::Value>,
}

impl LlmCapabilities {
    pub fn minimal(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            supports_streaming: false,
            supports_tool_calling: false,
            supports_streaming_tool_calls: false,
            supports_system_messages: false,
            supports_json_mode: false,
            supports_vision: false,
            supports_parallel_tool_calls: false,
            max_output_tokens: None,
            provider_specific: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<LlmToolCall>,
}

impl ChatMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioEncoding {
    Pcm16kMono,
    Opus,
    Wav,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscribeChunk {
    pub text: String,
    pub is_final: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub text: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<TranscriptionSegment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<TranscriptionAlternative>,
}

impl TranscriptionResult {
    pub fn plain(provider: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            provider: provider.into(),
            confidence: None,
            language: None,
            segments: Vec::new(),
            alternatives: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionSegment {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_logprob: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_speech_prob: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<TranscriptionWord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionWord {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionAlternative {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttCapabilities {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_input_encodings: Vec<AudioEncoding>,
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub supports_partial_results: bool,
    #[serde(default)]
    pub supports_confidence: bool,
    #[serde(default)]
    pub supports_word_confidence: bool,
    #[serde(default)]
    pub supports_segment_quality: bool,
    #[serde(default)]
    pub supports_segments: bool,
    #[serde(default)]
    pub supports_alternatives: bool,
    #[serde(default)]
    pub supports_language: bool,
    #[serde(default)]
    pub supports_language_hint: bool,
    #[serde(default)]
    pub supports_timestamps: bool,
    #[serde(default)]
    pub supports_vad: bool,
    #[serde(default)]
    pub supports_diarization: bool,
    #[serde(default)]
    pub supports_translation: bool,
    #[serde(default)]
    pub supports_punctuation: bool,
    #[serde(default)]
    pub supports_text_normalization: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_languages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_audio_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_audio_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_specific: BTreeMap<String, serde_json::Value>,
}

impl SttCapabilities {
    pub fn minimal(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            supported_input_encodings: Vec::new(),
            supports_streaming: false,
            supports_partial_results: false,
            supports_confidence: false,
            supports_word_confidence: false,
            supports_segment_quality: false,
            supports_segments: false,
            supports_alternatives: false,
            supports_language: false,
            supports_language_hint: false,
            supports_timestamps: false,
            supports_vad: false,
            supports_diarization: false,
            supports_translation: false,
            supports_punctuation: false,
            supports_text_normalization: false,
            supported_languages: Vec::new(),
            max_audio_duration_ms: None,
            max_audio_bytes: None,
            provider_specific: BTreeMap::new(),
        }
    }
}

// ========== LLM Adapter ==========

#[async_trait]
pub trait LlmAdapter: ModuleLifecycle {
    async fn llm_capabilities(&self) -> LlmCapabilities {
        LlmCapabilities::minimal(self.name())
    }

    /// Single-turn chat (non-streaming)
    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String>;

    /// Single-turn chat with native tool definitions when the provider supports
    /// function/tool calling. Providers that do not support tool calling fall
    /// back to plain text so the pipeline can still use its text protocol
    /// parser.
    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(LlmResponse::text(self.chat(messages).await?))
    }

    /// Streaming chat with native tool definitions, aggregated back into a
    /// provider-neutral response. Providers without streaming tool-call support
    /// may fall back to the non-streaming tool path.
    async fn chat_stream_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.chat_with_tools(messages, tools).await
    }

    /// Streaming chat, returns a token stream
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>>;
}

// ========== STT Adapter ==========

#[async_trait]
pub trait SttAdapter: ModuleLifecycle {
    /// Transcribe complete audio buffer with provider-neutral metadata.
    async fn transcribe_result(
        &self,
        audio: &[u8],
        encoding: AudioEncoding,
    ) -> anyhow::Result<TranscriptionResult>;

    async fn stt_capabilities(&self) -> SttCapabilities {
        SttCapabilities::minimal(self.name())
    }

    /// Streaming transcription
    async fn transcribe_stream(
        &self,
        audio_stream: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
        encoding: AudioEncoding,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<TranscribeChunk>> + Send>>>;
}

// ========== TTS Adapter ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsRequest {
    /// Text to synthesize.
    pub text: String,
    /// Provider-neutral expression style, such as `gentle`, `calm`, or
    /// `cheerful`.
    ///
    /// Providers may map this to a native style ID, reference voice, prompt,
    /// embedding, or ignore it when unsupported.
    pub style: Option<String>,
    /// Normalized expressive strength in the `[0, 1]` range.
    pub intensity: f32,
    /// Runtime voice profile ID, resolved by the active provider.
    ///
    /// This is an opaque profile name at the adapter boundary; provider-specific
    /// profile fields must stay inside the concrete adapter implementation.
    pub voice_profile: Option<String>,
    /// Provider-neutral prosody controls. Concrete adapters translate these
    /// normalized axes to native parameters and report unsupported/clamped
    /// fields through `provider_adjustments`.
    pub prosody: ProsodyControls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsProviderAdjustment {
    /// Adapter/provider that adjusted the request.
    pub provider: String,
    /// Stable path in `TtsRequest`, for example `speech.prosody.pitch_shift`.
    pub path: String,
    pub from: serde_json::Value,
    pub to: serde_json::Value,
    /// Machine-readable reason suitable for debug traces.
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsCapabilities {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_output_formats: Vec<String>,
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub supports_style: bool,
    #[serde(default)]
    pub supports_intensity: bool,
    #[serde(default)]
    pub supports_style_embedding: bool,
    #[serde(default)]
    pub supports_emotion_labels: bool,
    #[serde(default)]
    pub supports_reference_voice: bool,
    #[serde(default)]
    pub supports_voice_cloning: bool,
    #[serde(default)]
    pub supports_voice_selection: bool,
    #[serde(default)]
    pub supports_multilingual_voice: bool,
    #[serde(default)]
    pub supports_ssml: bool,
    #[serde(default)]
    pub supports_phoneme_control: bool,
    #[serde(default)]
    pub supports_sentence_level_prosody: bool,
    #[serde(default)]
    pub supports_pitch: bool,
    #[serde(default)]
    pub supports_rate: bool,
    #[serde(default)]
    pub supports_energy: bool,
    #[serde(default)]
    pub supports_pause: bool,
    #[serde(default)]
    pub supports_streaming_prosody: bool,
    #[serde(default)]
    pub prosody_limits: TtsProsodyLimits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_text_chars: Option<usize>,
    #[serde(default)]
    pub latency_class: TtsLatencyClass,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_specific: BTreeMap<String, serde_json::Value>,
}

impl TtsCapabilities {
    pub fn minimal(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            supported_output_formats: Vec::new(),
            supports_streaming: false,
            supports_style: false,
            supports_intensity: false,
            supports_style_embedding: false,
            supports_emotion_labels: false,
            supports_reference_voice: false,
            supports_voice_cloning: false,
            supports_voice_selection: false,
            supports_multilingual_voice: false,
            supports_ssml: false,
            supports_phoneme_control: false,
            supports_sentence_level_prosody: false,
            supports_pitch: false,
            supports_rate: false,
            supports_energy: false,
            supports_pause: false,
            supports_streaming_prosody: false,
            prosody_limits: TtsProsodyLimits::default(),
            max_text_chars: None,
            latency_class: TtsLatencyClass::Unknown,
            provider_specific: BTreeMap::new(),
        }
    }

    pub fn provider_adjustments(&self, request: &TtsRequest) -> Vec<TtsProviderAdjustment> {
        let mut adjustments = Vec::new();
        if let Some(max_chars) = self.max_text_chars
            && request.text.chars().count() > max_chars
        {
            adjustments.push(tts_adjustment(
                &self.provider,
                "speech.text",
                serde_json::json!(request.text.chars().count()),
                serde_json::json!(max_chars),
                "provider_text_length_limit_exceeded",
            ));
        }
        if request.style.is_some() && !self.supports_style {
            adjustments.push(tts_adjustment(
                &self.provider,
                "speech.style",
                serde_json::json!(request.style),
                serde_json::Value::Null,
                "provider_style_control_unavailable",
            ));
        }
        if request.intensity.abs() > f32::EPSILON && !self.supports_intensity {
            adjustments.push(tts_adjustment(
                &self.provider,
                "speech.intensity",
                serde_json::json!(request.intensity),
                serde_json::json!(0.0),
                "provider_intensity_control_unavailable",
            ));
        }
        if request.voice_profile.is_some() && !self.supports_reference_voice {
            adjustments.push(tts_adjustment(
                &self.provider,
                "speech.voice_profile",
                serde_json::json!(request.voice_profile),
                serde_json::Value::Null,
                "provider_reference_voice_unavailable",
            ));
        }
        adjustments.extend(prosody_capability_adjustments(self, request));
        adjustments
    }
}

fn prosody_capability_adjustments(
    capabilities: &TtsCapabilities,
    request: &TtsRequest,
) -> Vec<TtsProviderAdjustment> {
    let mut adjustments = Vec::new();
    let prosody = request.prosody;
    if !capabilities.supports_rate && (prosody.rate - 1.0).abs() > f32::EPSILON {
        adjustments.push(tts_adjustment(
            &capabilities.provider,
            "speech.prosody.rate",
            serde_json::json!(prosody.rate),
            serde_json::json!(1.0),
            "provider_rate_control_unavailable",
        ));
    } else if capabilities.supports_rate
        && outside_range(
            prosody.rate,
            capabilities.prosody_limits.min_rate,
            capabilities.prosody_limits.max_rate,
        )
    {
        adjustments.push(tts_adjustment(
            &capabilities.provider,
            "speech.prosody.rate",
            serde_json::json!(prosody.rate),
            serde_json::json!(prosody.rate.clamp(
                capabilities.prosody_limits.min_rate,
                capabilities.prosody_limits.max_rate
            )),
            "provider_rate_limit_clamped",
        ));
    }
    if !capabilities.supports_pitch && prosody.pitch_shift.abs() > f32::EPSILON {
        adjustments.push(tts_adjustment(
            &capabilities.provider,
            "speech.prosody.pitch_shift",
            serde_json::json!(prosody.pitch_shift),
            serde_json::json!(0.0),
            "provider_pitch_control_unavailable",
        ));
    } else if capabilities.supports_pitch
        && outside_range(
            prosody.pitch_shift,
            capabilities.prosody_limits.min_pitch_shift,
            capabilities.prosody_limits.max_pitch_shift,
        )
    {
        adjustments.push(tts_adjustment(
            &capabilities.provider,
            "speech.prosody.pitch_shift",
            serde_json::json!(prosody.pitch_shift),
            serde_json::json!(prosody.pitch_shift.clamp(
                capabilities.prosody_limits.min_pitch_shift,
                capabilities.prosody_limits.max_pitch_shift
            )),
            "provider_pitch_limit_clamped",
        ));
    }
    if !capabilities.supports_energy && (prosody.energy - 0.5).abs() > f32::EPSILON {
        adjustments.push(tts_adjustment(
            &capabilities.provider,
            "speech.prosody.energy",
            serde_json::json!(prosody.energy),
            serde_json::json!(0.5),
            "provider_energy_control_unavailable",
        ));
    } else if capabilities.supports_energy
        && outside_range(
            prosody.energy,
            capabilities.prosody_limits.min_energy,
            capabilities.prosody_limits.max_energy,
        )
    {
        adjustments.push(tts_adjustment(
            &capabilities.provider,
            "speech.prosody.energy",
            serde_json::json!(prosody.energy),
            serde_json::json!(prosody.energy.clamp(
                capabilities.prosody_limits.min_energy,
                capabilities.prosody_limits.max_energy
            )),
            "provider_energy_limit_clamped",
        ));
    }
    if !capabilities.supports_pause && prosody.pause_bias.abs() > f32::EPSILON {
        adjustments.push(tts_adjustment(
            &capabilities.provider,
            "speech.prosody.pause_bias",
            serde_json::json!(prosody.pause_bias),
            serde_json::json!(0.0),
            "provider_pause_control_unavailable",
        ));
    } else if capabilities.supports_pause
        && outside_range(
            prosody.pause_bias,
            capabilities.prosody_limits.min_pause_bias,
            capabilities.prosody_limits.max_pause_bias,
        )
    {
        adjustments.push(tts_adjustment(
            &capabilities.provider,
            "speech.prosody.pause_bias",
            serde_json::json!(prosody.pause_bias),
            serde_json::json!(prosody.pause_bias.clamp(
                capabilities.prosody_limits.min_pause_bias,
                capabilities.prosody_limits.max_pause_bias
            )),
            "provider_pause_limit_clamped",
        ));
    }
    adjustments
}

fn outside_range(value: f32, min: f32, max: f32) -> bool {
    value < min || value > max
}

fn tts_adjustment(
    provider: &str,
    path: &str,
    from: serde_json::Value,
    to: serde_json::Value,
    reason: &str,
) -> TtsProviderAdjustment {
    TtsProviderAdjustment {
        provider: provider.to_string(),
        path: path.to_string(),
        from,
        to,
        reason: reason.to_string(),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TtsProsodyLimits {
    pub min_rate: f32,
    pub max_rate: f32,
    pub min_pitch_shift: f32,
    pub max_pitch_shift: f32,
    pub min_energy: f32,
    pub max_energy: f32,
    pub min_pause_bias: f32,
    pub max_pause_bias: f32,
}

impl Default for TtsProsodyLimits {
    fn default() -> Self {
        Self {
            min_rate: 1.0,
            max_rate: 1.0,
            min_pitch_shift: 0.0,
            max_pitch_shift: 0.0,
            min_energy: 0.5,
            max_energy: 0.5,
            min_pause_bias: 0.0,
            max_pause_bias: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TtsLatencyClass {
    Realtime,
    Interactive,
    Batch,
    Unknown,
}

impl Default for TtsLatencyClass {
    fn default() -> Self {
        Self::Unknown
    }
}

impl TtsRequest {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
            intensity: 0.0,
            voice_profile: None,
            prosody: ProsodyControls::default(),
        }
    }

    pub fn with_speech(text: impl Into<String>, speech: &SpeechExpression) -> Self {
        Self {
            text: text.into(),
            style: Some(speech.style.clone()),
            intensity: speech.intensity,
            voice_profile: speech.voice_profile.clone(),
            prosody: speech.prosody,
        }
    }
}

pub type TtsAudioStream =
    Pin<Box<dyn Stream<Item = anyhow::Result<EncodedAudio>> + Send + 'static>>;

#[async_trait]
pub trait TtsAdapter: ModuleLifecycle {
    /// Providers must reserve before copying encoded media. No unbounded
    /// Vec-returning fallback is part of this contract.
    async fn synthesize_request(
        &self,
        request: &TtsRequest,
        budget: ResponseAudioBudget,
    ) -> anyhow::Result<EncodedAudio>;

    async fn synthesize_stream_request(
        &self,
        request: &TtsRequest,
        budget: ResponseAudioBudget,
    ) -> anyhow::Result<TtsAudioStream>;

    async fn synthesize(
        &self,
        text: &str,
        budget: ResponseAudioBudget,
    ) -> anyhow::Result<EncodedAudio> {
        self.synthesize_request(&TtsRequest::plain(text), budget)
            .await
    }

    async fn synthesize_stream(
        &self,
        text: &str,
        budget: ResponseAudioBudget,
    ) -> anyhow::Result<TtsAudioStream> {
        self.synthesize_stream_request(&TtsRequest::plain(text), budget)
            .await
    }

    /// Report provider-specific request degradations before synthesis.
    ///
    /// This lets pipelines trace provider limits without teaching the
    /// expression system about a concrete backend. For example, GPT-SoVITS can
    /// report that pitch control was dropped, while another provider may report
    /// unsupported pause control or a native rate clamp.
    async fn provider_adjustments(&self, request: &TtsRequest) -> Vec<TtsProviderAdjustment> {
        self.tts_capabilities().await.provider_adjustments(request)
    }

    /// Provider-neutral capability/limits negotiation for expression prosody.
    async fn tts_capabilities(&self) -> TtsCapabilities {
        TtsCapabilities::minimal(self.name())
    }
}

// ========== Embedding Adapter ==========

/// The immutable descriptor of the configuration used for one embedding request.
/// `revision` names the deployment's descriptor format, not an unverifiable
/// remote weights version. Model/endpoint/dimension changes change the ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingProfile {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub revision: String,
    pub dimensions: usize,
    pub distance: String,
    pub vector_type: String,
}

impl EmbeddingProfile {
    pub fn from_server_config(
        config: &crate::config::ServerConfig,
    ) -> anyhow::Result<Option<Self>> {
        if config.llm.roles.embedding == "noop" {
            return Ok(None);
        }
        let profile = config.profiles.llm_profile(&config.llm.roles.embedding)?;
        if profile.kind != crate::config::LlmProfileKind::Embedding {
            return Ok(None);
        }
        let crate::config::LlmProviderProfileRef::OpenAi(provider) =
            config.llm_provider_profile(&profile.provider)?
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            provider: profile.provider.clone(),
            endpoint: provider.base_url.trim_end_matches('/').to_string(),
            model: profile.model.clone(),
            revision: "request-config.v1".into(),
            dimensions: profile
                .dimensions
                .unwrap_or_else(|| embedding_model_dimension(&profile.model)),
            distance: "cosine".into(),
            vector_type: "vector".into(),
        }))
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.provider.is_empty()
                && self.provider.len() <= 256
                && !self.endpoint.is_empty()
                && self.endpoint.len() <= 2048
                && !self.model.is_empty()
                && self.model.len() <= 256
                && !self.revision.is_empty()
                && self.revision.len() <= 256
                && (1..=16_000).contains(&self.dimensions)
                && self.distance == "cosine"
                && self.vector_type == "vector",
            "invalid embedding profile"
        );
        Ok(())
    }

    pub fn id(&self) -> anyhow::Result<String> {
        self.validate()?;
        Ok(format!(
            "alia.embedding.{}",
            embedding_text_digest(&serde_json::to_string(self)?)
        ))
    }
}

pub fn embedding_model_dimension(model: &str) -> usize {
    match model {
        "text-embedding-3-large" => 3072,
        _ => 1536,
    }
}

pub fn embedding_text_digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[derive(Debug, Clone)]
pub struct EmbeddingVector {
    pub input_digest: String,
    pub values: Vec<f32>,
}

impl EmbeddingVector {
    pub fn validate(&self, profile: &EmbeddingProfile) -> anyhow::Result<()> {
        profile.validate()?;
        anyhow::ensure!(
            self.input_digest.len() == 64
                && self
                    .input_digest
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                && self.values.len() == profile.dimensions
                && self.values.iter().all(|v| v.is_finite())
                && self.values.iter().any(|v| *v != 0.0),
            "invalid profiled embedding vector"
        );
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ProfiledEmbeddings {
    pub profile: EmbeddingProfile,
    pub vectors: Vec<EmbeddingVector>,
}

#[async_trait]
pub trait EmbeddingAdapter: ModuleLifecycle {
    /// Batch text embedding
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>>;
    /// None means this adapter cannot prove the profile of its returned vectors.
    /// Callers must skip semantic indexing, never label `embed()` after the fact.
    async fn embed_profiled(
        &self,
        _texts: &[String],
    ) -> anyhow::Result<Option<ProfiledEmbeddings>> {
        Ok(None)
    }
    /// Pure preview from one configuration snapshot. A policy CAS must commit
    /// before apply_settings publishes a different embedding identity.
    async fn preview_profile_settings(
        &self,
        _updates: &std::collections::BTreeMap<String, serde_json::Value>,
    ) -> anyhow::Result<Option<EmbeddingProfile>> {
        Ok(None)
    }
    /// Vector dimension
    fn dimension(&self) -> usize;
}

// ========== Rule Evaluator ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleAction {
    pub rule_id: Uuid,
    pub rule_name: String,
    pub action_type: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationRule {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub enabled: bool,
    /// JSON pattern matched against serialized `ServerEvent`.
    pub event_pattern: serde_json::Value,
    /// JSON condition expression evaluated after the event pattern matches.
    pub conditions: serde_json::Value,
    /// Action config with a `type` field, executed by the shared action registry.
    pub action: serde_json::Value,
    pub cooldown_sec: u32,
    pub created_by: String,
}

#[derive(Debug, Clone, Default)]
pub struct RuleFilter {
    pub user_id: Option<Uuid>,
    pub enabled_only: bool,
}

#[derive(Debug, Clone)]
pub struct RulePatch {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub event_pattern: Option<serde_json::Value>,
    pub conditions: Option<serde_json::Value>,
    pub action: Option<serde_json::Value>,
    pub cooldown_sec: Option<u32>,
}

#[async_trait]
pub trait RuleEvaluator: ModuleLifecycle {
    async fn evaluate(&self, event: &crate::event::ServerEvent) -> anyhow::Result<Vec<RuleAction>>;
    async fn reload(&self) -> anyhow::Result<()>;
    async fn create_rule(&self, rule: AutomationRule) -> anyhow::Result<Uuid>;
    async fn update_rule(&self, rule_id: Uuid, patch: RulePatch) -> anyhow::Result<()>;
    async fn delete_rule(&self, rule_id: Uuid) -> anyhow::Result<()>;
    async fn list_rules(&self, filter: RuleFilter) -> anyhow::Result<Vec<AutomationRule>>;
    async fn pause_rule(&self, rule_id: Uuid) -> anyhow::Result<()>;
    async fn resume_rule(&self, rule_id: Uuid) -> anyhow::Result<()>;
}

// ========== Scheduler Backend ==========

/// Scheduled task trigger types: cron, once, or interval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleConfig {
    #[serde(rename = "type")]
    pub schedule_type: String, // "cron" | "once" | "interval" | "condition"
    #[serde(default)]
    pub cron_expression: Option<String>, // For cron type
    #[serde(default)]
    pub run_at: Option<String>, // For once type (ISO 8601)
    #[serde(default)]
    pub every_sec: Option<u32>, // For interval type
    #[serde(default)]
    pub poll_interval_sec: Option<u32>, // For condition/polling type
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub schedule: ScheduleConfig,
    pub conditions: serde_json::Value, // JSON array of condition configs
    pub action: serde_json::Value,     // Action config (type + payload)
    pub cooldown_sec: u32,
    pub max_retries: u32,
    pub created_by: String, // "user" | "ai"
}

#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub user_id: Option<Uuid>,
    pub enabled_only: bool,
}

#[derive(Debug, Clone)]
pub struct TaskPatch {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub schedule: Option<ScheduleConfig>,
    pub conditions: Option<serde_json::Value>,
    pub action: Option<serde_json::Value>,
}

/// Event subscription: trigger actions when EventBus events match a pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSubscription {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub event_pattern: serde_json::Value, // Pattern to match against ServerEvent
    pub conditions: serde_json::Value,    // Additional conditions
    pub action: serde_json::Value,        // Action config
    pub cooldown_sec: u32,
    pub created_by: String,
}

/// Trait for plugin-provided action executors
#[async_trait]
pub trait ActionExecutor: Send + Sync {
    fn action_type(&self) -> &str;
    async fn execute(
        &self,
        config: &serde_json::Value,
        ctx: &ActionContext,
    ) -> anyhow::Result<serde_json::Value>;
}

#[derive(Debug, Clone)]
pub struct ActionContext {
    pub user_id: Uuid,
    /// Identity of this triggering occurrence, retained across retries.
    pub request_id: Uuid,
    /// Trusted trigger observation time, retained while actions wait or retry.
    pub source_received_at: chrono::DateTime<chrono::Utc>,
    /// Captured at trusted source admission; queueing and retries must retain it.
    pub source_privacy_epoch: Option<i64>,
    pub source: String,
    pub trigger_id: Option<Uuid>,
    pub trigger_name: Option<String>,
    pub event: Option<serde_json::Value>,
}

#[async_trait]
pub trait SchedulerBackend: ModuleLifecycle {
    /// Create a new scheduled task
    async fn create_task(&self, task: ScheduledTask) -> anyhow::Result<Uuid>;
    /// Update a task
    async fn update_task(&self, task_id: Uuid, patch: TaskPatch) -> anyhow::Result<()>;
    /// Delete a task
    async fn delete_task(&self, task_id: Uuid) -> anyhow::Result<()>;
    /// List tasks with filters
    async fn list_tasks(&self, filter: TaskFilter) -> anyhow::Result<Vec<ScheduledTask>>;
    /// Pause a task
    async fn pause_task(&self, task_id: Uuid) -> anyhow::Result<()>;
    /// Resume a task
    async fn resume_task(&self, task_id: Uuid) -> anyhow::Result<()>;
    /// Subscribe to events
    async fn subscribe_event(&self, sub: EventSubscription) -> anyhow::Result<Uuid>;
    /// Unsubscribe from events
    async fn unsubscribe_event(&self, sub_id: Uuid) -> anyhow::Result<()>;
    /// List event subscriptions
    async fn list_subscriptions(&self, user_id: Uuid) -> anyhow::Result<Vec<EventSubscription>>;
}

// ========== Tool Registry ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
    /// Optional routing hints owned by the tool provider. These are not shown
    /// to the LLM as executable arguments; they only help the server narrow
    /// large tool catalogs before a turn reaches the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routing_hints: Vec<String>,
    /// Owner source. Use "builtin", "component:<name>", "api", or a plugin name.
    pub source: String,
    pub require_confirmation: bool,
    pub required_role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub call_id: String, // LLM-assigned ID for result correlation
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub success: bool,
    pub output: serde_json::Value,
    pub display: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRoutingEmbedding {
    pub model: Option<String>,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRoutingIndexEntry {
    pub name: String,
    pub routing_text: String,
    #[serde(default)]
    pub routing_keywords: Vec<String>,
    #[serde(default)]
    pub routing_embedding: Option<ToolRoutingEmbedding>,
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub user_id: Uuid,
    /// Original model input lineage; required for every AI-originated effect.
    pub context_id: Option<Uuid>,
    /// Assigned by the trusted ingress; never supplied by a model argument.
    pub request_id: Uuid,
    /// Original input observation time; queueing, approval and retries cannot refresh it.
    pub source_received_at: chrono::DateTime<chrono::Utc>,
    /// Captured at trusted source admission; absent is not the current epoch.
    pub source_privacy_epoch: Option<i64>,
    /// Persisted user message for AI-triggered writes; absent for manual/scheduled work.
    pub source_event_id: Option<Uuid>,
    /// Position within the request (for example LLM depth:index).
    pub invocation_key: String,
    pub conversation_id: Option<Uuid>,
    pub source: String, // "ai" | "manual" | "scheduler"
    pub channel_id: Option<String>,
    /// Cancels waiting and undispatched work; dispatched effects are independent.
    pub cancel: Option<tokio_util::sync::CancellationToken>,
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    /// Identifies the loaded implementation and any immutable configuration
    /// that selects external targets. Missing revisions cannot execute durably.
    /// Compiled implementations are additionally bound to the host binary hash.
    fn implementation_revision(&self) -> Option<String> {
        None
    }
    /// Opt in only when a returned failure proves rejection/non-execution.
    /// Some legacy adapters encode transport errors as Ok(success=false), so
    /// the durable execution service must treat that case as unknown by default.
    fn failure_is_definitive(&self, _result: &ToolResult) -> bool {
        false
    }
    async fn execute(
        &self,
        arguments: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult>;
}

#[derive(Clone)]
pub struct PreparedTool {
    pub executor: Arc<dyn ToolExecutor>,
    pub registration_revision: String,
}

#[async_trait]
pub trait ToolRegistry: ModuleLifecycle {
    /// Pin the actual implementation before claiming a durable execution.
    /// Product-facing registries deliberately keep this unavailable.
    async fn resolve_executor(&self, _name: &str) -> anyhow::Result<Option<PreparedTool>> {
        anyhow::bail!("raw tool executor access is unavailable")
    }
    /// List all available tools
    async fn list_tools(&self) -> anyhow::Result<Vec<ToolDefinition>>;
    /// Get a specific tool definition
    async fn get_tool(&self, name: &str) -> anyhow::Result<Option<ToolDefinition>>;
    /// Execute a tool call
    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> anyhow::Result<ToolResult>;
    /// Register plugin tool metadata only.
    ///
    /// Implementations may reject this because tools should be executable
    /// units. Rust/server plugins should prefer `register_tool_executor`.
    async fn register_plugin_tool(&self, tool: ToolDefinition) -> anyhow::Result<()>;
    /// Register a code-backed tool executor. This is the preferred path for
    /// Rust plugins/components because tools are executable units, not only DB
    /// metadata. `owner_source` should be "builtin", "component:<name>", or a
    /// plugin name and must match `executor.definition().source`.
    async fn register_tool_executor(
        &self,
        owner_source: &str,
        executor: Arc<dyn ToolExecutor>,
    ) -> anyhow::Result<()>;
    /// Unregister a plugin tool
    async fn unregister_plugin_tool(&self, tool_name: &str) -> anyhow::Result<()>;
    /// Unregister all tools owned by a plugin.
    async fn unregister_plugin_tools(&self, plugin_name: &str) -> anyhow::Result<u64>;
    /// Return routing index entries for the requested tool names. Implementors
    /// can back this with a database catalog; the pipeline can still fall back
    /// to deterministic ranking when no index is available.
    async fn routing_index(
        &self,
        _tool_names: &[String],
    ) -> anyhow::Result<Vec<ToolRoutingIndexEntry>> {
        Ok(Vec::new())
    }
    /// Store or refresh the routing embedding for a tool catalog row.
    async fn update_routing_embedding(
        &self,
        _tool_name: &str,
        _embedding_model: &str,
        _embedding: &[f32],
    ) -> anyhow::Result<()> {
        Ok(())
    }
}
