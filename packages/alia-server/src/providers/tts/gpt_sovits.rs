//! GPT-SoVITS TTS Adapter
//!
//! Implements TtsAdapter for GPT-SoVITS server with streaming support.
//! Connects to GPT-SoVITS HTTP API for high-quality voice synthesis.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::RwLock;

use super::audio_budget::{
    EncodedAudio, MAX_ENCODED_PAYLOAD_BYTES, MAX_TTS_STREAM_BYTES, ResponseAudioBudget,
    TtsAudioStream,
};
use super::http_body::{ERROR_BODY_BYTES, check_length, error_body, read_body};
use crate::config::{ExpressionStyleSection, ServerConfig, VoiceProfileSection};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    TtsAdapter, TtsCapabilities, TtsProsodyLimits, TtsProviderAdjustment, TtsRequest,
};
use crate::settings::values::{bool_setting, f32_setting, string_setting, u64_setting};

/// GPT-SoVITS synthesis request
#[derive(Debug, Serialize)]
struct SovitsRequest {
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text_lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ref_audio_path: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    aux_ref_audio_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_lang: Option<String>,
    top_k: u32,
    top_p: f32,
    temperature: f32,
    text_split_method: String,
    batch_size: u32,
    batch_threshold: f32,
    split_bucket: bool,
    speed_factor: f32,
    fragment_interval: f32,
    seed: i32,
    parallel_infer: bool,
    repetition_penalty: f32,
    sample_steps: u32,
    super_sampling: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    media_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    streaming_mode: Option<u8>,
    overlap_length: u32,
    min_chunk_length: u32,
}

/// GPT-SoVITS TTS adapter
pub struct GptSovitsTts {
    client: Client,
    config: Arc<RwLock<SovitsConfig>>,
}

#[derive(Debug, Clone)]
struct SovitsConfig {
    endpoint: String,
    default_voice: String,
    default_profile: SovitsVoiceProfile,
    voice_profiles: BTreeMap<String, SovitsVoiceProfile>,
    expression_styles: BTreeMap<String, ExpressionStyleSection>,
    prosody_limits: SovitsProsodyLimits,
    sample_rate_pitch_fallback_enabled: bool,
    request_timeout_sec: u64,
    stream_idle_timeout_sec: u64,
}

#[derive(Debug, Clone, Copy)]
struct SovitsProsodyLimits {
    min_speed_factor: f32,
    max_speed_factor: f32,
    min_temperature: f32,
    max_temperature: f32,
    min_top_p: f32,
    max_top_p: f32,
    min_repetition_penalty: f32,
    max_repetition_penalty: f32,
    min_fragment_interval: f32,
    max_fragment_interval: f32,
    max_pitch_shift_semitones: f32,
}

#[derive(Debug, Clone, Copy)]
struct SovitsProsodyMapping {
    top_k: u32,
    top_p: f32,
    temperature: f32,
    speed_factor: f32,
    fragment_interval: f32,
    repetition_penalty: f32,
    pitch_factor: f32,
    pitch_shift_semitones: f32,
    requested_pitch_shift_semitones: f32,
    sample_rate_pitch_fallback_enabled: bool,
}

#[derive(Debug, Clone)]
struct SovitsVoiceProfile {
    model_name: Option<String>,
    gpt_weights_path: Option<String>,
    sovits_weights_path: Option<String>,
    ref_audio_path: Option<String>,
    aux_ref_audio_paths: Vec<String>,
    prompt_text: Option<String>,
    text_lang: String,
    prompt_lang: String,
    text_split_method: String,
    batch_size: u32,
    batch_threshold: f32,
    split_bucket: bool,
    speed_factor: f32,
    fragment_interval: f32,
    seed: i32,
    parallel_infer: bool,
    repetition_penalty: f32,
    sample_steps: u32,
    super_sampling: bool,
    overlap_length: u32,
    min_chunk_length: u32,
}

impl GptSovitsTts {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            config: Arc::new(RwLock::new(SovitsConfig {
                endpoint: "http://localhost:9880".to_string(),
                default_voice: "default".to_string(),
                default_profile: SovitsVoiceProfile::default(),
                voice_profiles: BTreeMap::new(),
                expression_styles: BTreeMap::new(),
                prosody_limits: SovitsProsodyLimits::default(),
                sample_rate_pitch_fallback_enabled: false,
                request_timeout_sec: 60,
                stream_idle_timeout_sec: 10,
            })),
        }
    }
}

impl Default for GptSovitsTts {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for SovitsVoiceProfile {
    fn default() -> Self {
        Self::from_voice_config(&VoiceProfileSection::default())
    }
}

impl Default for SovitsProsodyLimits {
    fn default() -> Self {
        Self {
            min_speed_factor: 0.85,
            max_speed_factor: 1.15,
            min_temperature: 0.75,
            max_temperature: 1.25,
            min_top_p: 0.82,
            max_top_p: 1.0,
            min_repetition_penalty: 1.05,
            max_repetition_penalty: 1.55,
            min_fragment_interval: 0.12,
            max_fragment_interval: 0.75,
            max_pitch_shift_semitones: 2.0,
        }
    }
}

impl SovitsProsodyLimits {
    fn from_provider_config(config: &crate::config::GptSovitsProviderSection) -> Self {
        Self {
            min_speed_factor: config.min_speed_factor.max(0.1),
            max_speed_factor: config
                .max_speed_factor
                .max(config.min_speed_factor.max(0.1)),
            min_temperature: config.min_temperature.max(0.0),
            max_temperature: config.max_temperature.max(config.min_temperature.max(0.0)),
            min_top_p: config.min_top_p.clamp(0.0, 1.0),
            max_top_p: config
                .max_top_p
                .clamp(config.min_top_p.clamp(0.0, 1.0), 1.0),
            min_repetition_penalty: config.min_repetition_penalty.max(0.0),
            max_repetition_penalty: config
                .max_repetition_penalty
                .max(config.min_repetition_penalty.max(0.0)),
            min_fragment_interval: config.min_fragment_interval.max(0.0),
            max_fragment_interval: config
                .max_fragment_interval
                .max(config.min_fragment_interval.max(0.0)),
            max_pitch_shift_semitones: config.max_pitch_shift_semitones.max(0.0).min(12.0),
        }
    }
}

impl SovitsProsodyMapping {
    fn provider_adjustments(&self) -> Vec<TtsProviderAdjustment> {
        if self.sample_rate_pitch_fallback_enabled
            || self.requested_pitch_shift_semitones.abs() <= f32::EPSILON
        {
            return Vec::new();
        }

        vec![TtsProviderAdjustment {
            provider: "gpt_sovits".to_string(),
            path: "speech.prosody.pitch_shift".to_string(),
            from: serde_json::json!(self.requested_pitch_shift_semitones),
            to: serde_json::json!(self.pitch_shift_semitones),
            reason: "provider_pitch_control_unavailable_sample_rate_fallback_disabled".to_string(),
        }]
    }
}

impl SovitsVoiceProfile {
    fn from_voice_config(config: &VoiceProfileSection) -> Self {
        let specific = &config.provider_specific;
        Self {
            model_name: non_empty(config.model_name.as_deref()),
            gpt_weights_path: provider_string(specific, "gpt_weights_path"),
            sovits_weights_path: provider_string(specific, "sovits_weights_path"),
            ref_audio_path: non_empty(config.reference_voice.as_deref()),
            aux_ref_audio_paths: provider_string_vec(specific, "aux_ref_audio_paths"),
            prompt_text: non_empty(config.style_prompt.as_deref()),
            text_lang: config.language.clone(),
            prompt_lang: provider_string(specific, "prompt_language")
                .unwrap_or_else(|| "zh".into()),
            text_split_method: provider_string(specific, "text_split_method")
                .unwrap_or_else(|| "cut5".into()),
            batch_size: provider_u32(specific, "batch_size", 1),
            batch_threshold: provider_f32(specific, "batch_threshold", 0.75),
            split_bucket: provider_bool(specific, "split_bucket", true),
            speed_factor: provider_f32(specific, "speed_factor", 1.0),
            fragment_interval: provider_f32(specific, "fragment_interval", 0.3),
            seed: provider_i32(specific, "seed", -1),
            parallel_infer: provider_bool(specific, "parallel_infer", true),
            repetition_penalty: provider_f32(specific, "repetition_penalty", 1.35),
            sample_steps: provider_u32(specific, "sample_steps", 32),
            super_sampling: provider_bool(specific, "super_sampling", false),
            overlap_length: provider_u32(specific, "overlap_length", 2),
            min_chunk_length: provider_u32(specific, "min_chunk_length", 16),
        }
    }
}

fn provider_string(values: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<String> {
    values
        .get(key)
        .and_then(serde_json::Value::as_str)
        .and_then(|value| non_empty(Some(value)))
}

fn provider_string_vec(values: &BTreeMap<String, serde_json::Value>, key: &str) -> Vec<String> {
    values
        .get(key)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter_map(|value| non_empty(Some(value)))
        .collect()
}

fn provider_f32(values: &BTreeMap<String, serde_json::Value>, key: &str, default: f32) -> f32 {
    values
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .map(|value| value as f32)
        .unwrap_or(default)
}

fn provider_u32(values: &BTreeMap<String, serde_json::Value>, key: &str, default: u32) -> u32 {
    values
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(default)
}

fn provider_i32(values: &BTreeMap<String, serde_json::Value>, key: &str, default: i32) -> i32 {
    values
        .get(key)
        .and_then(serde_json::Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(default)
}

fn provider_bool(values: &BTreeMap<String, serde_json::Value>, key: &str, default: bool) -> bool {
    values
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(default)
}

fn apply_sovits_provider_specific_setting(
    profile: &mut SovitsVoiceProfile,
    field: &str,
    value: &serde_json::Value,
) {
    match field {
        "gpt_weights_path" => {
            profile.gpt_weights_path = value.as_str().and_then(|v| non_empty(Some(v)))
        }
        "sovits_weights_path" => {
            profile.sovits_weights_path = value.as_str().and_then(|v| non_empty(Some(v)))
        }
        "aux_ref_audio_paths" => {
            profile.aux_ref_audio_paths = value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .filter_map(|v| non_empty(Some(v)))
                .collect();
        }
        "prompt_language" => {
            if let Some(v) = value.as_str().and_then(|v| non_empty(Some(v))) {
                profile.prompt_lang = v;
            }
        }
        "text_split_method" => {
            if let Some(v) = value.as_str().and_then(|v| non_empty(Some(v))) {
                profile.text_split_method = v;
            }
        }
        "batch_size" => {
            if let Some(v) = value.as_u64().and_then(|v| u32::try_from(v).ok()) {
                profile.batch_size = v;
            }
        }
        "batch_threshold" => {
            if let Some(v) = value.as_f64() {
                profile.batch_threshold = v as f32;
            }
        }
        "split_bucket" => {
            if let Some(v) = value.as_bool() {
                profile.split_bucket = v;
            }
        }
        "speed_factor" => {
            if let Some(v) = value.as_f64() {
                profile.speed_factor = v as f32;
            }
        }
        "fragment_interval" => {
            if let Some(v) = value.as_f64() {
                profile.fragment_interval = v as f32;
            }
        }
        "seed" => {
            if let Some(v) = value.as_i64().and_then(|v| i32::try_from(v).ok()) {
                profile.seed = v;
            }
        }
        "parallel_infer" => {
            if let Some(v) = value.as_bool() {
                profile.parallel_infer = v;
            }
        }
        "repetition_penalty" => {
            if let Some(v) = value.as_f64() {
                profile.repetition_penalty = v as f32;
            }
        }
        "sample_steps" => {
            if let Some(v) = value.as_u64().and_then(|v| u32::try_from(v).ok()) {
                profile.sample_steps = v;
            }
        }
        "super_sampling" => {
            if let Some(v) = value.as_bool() {
                profile.super_sampling = v;
            }
        }
        "overlap_length" => {
            if let Some(v) = value.as_u64().and_then(|v| u32::try_from(v).ok()) {
                profile.overlap_length = v;
            }
        }
        "min_chunk_length" => {
            if let Some(v) = value.as_u64().and_then(|v| u32::try_from(v).ok()) {
                profile.min_chunk_length = v;
            }
        }
        _ => {}
    }
}

impl SovitsConfig {
    fn request(
        &self,
        request: &TtsRequest,
        streaming_mode: u8,
    ) -> (SovitsRequest, SovitsProsodyMapping) {
        let profile = self.profile_for_request(request);
        let prosody = self.map_prosody(profile, request);
        let sovits_request = SovitsRequest {
            text: request.text.clone(),
            text_lang: Some(profile.text_lang.clone()),
            ref_audio_path: profile.ref_audio_path.clone(),
            aux_ref_audio_paths: profile.aux_ref_audio_paths.clone(),
            prompt_text: profile.prompt_text.clone(),
            prompt_lang: Some(profile.prompt_lang.clone()),
            top_k: prosody.top_k,
            top_p: prosody.top_p,
            temperature: prosody.temperature,
            text_split_method: profile.text_split_method.clone(),
            batch_size: profile.batch_size,
            batch_threshold: profile.batch_threshold,
            split_bucket: profile.split_bucket,
            speed_factor: prosody.speed_factor,
            fragment_interval: prosody.fragment_interval,
            seed: profile.seed,
            parallel_infer: profile.parallel_infer,
            repetition_penalty: prosody.repetition_penalty,
            sample_steps: profile.sample_steps,
            super_sampling: profile.super_sampling,
            media_type: Some("wav"),
            streaming_mode: Some(streaming_mode),
            overlap_length: profile.overlap_length,
            min_chunk_length: profile.min_chunk_length,
        };
        (sovits_request, prosody)
    }

    fn map_prosody(
        &self,
        profile: &SovitsVoiceProfile,
        request: &TtsRequest,
    ) -> SovitsProsodyMapping {
        let limits = self.prosody_limits;
        let rate = request.prosody.rate.clamp(0.5, 2.0);
        let energy = request.prosody.energy.clamp(0.0, 1.0);
        let intensity = request.intensity.clamp(0.0, 1.0);
        let pause_bias = request.prosody.pause_bias.clamp(-1.0, 1.0);
        let energy_axis = (energy - 0.5) * 2.0;
        let intensity_axis = (intensity - 0.35) * 1.5;

        let speed_factor =
            (profile.speed_factor * rate).clamp(limits.min_speed_factor, limits.max_speed_factor);
        let temperature = (1.0 + energy_axis * 0.12 + intensity_axis * 0.06)
            .clamp(limits.min_temperature, limits.max_temperature);
        let top_p = (0.94 + energy_axis * 0.04 + intensity_axis * 0.025)
            .clamp(limits.min_top_p, limits.max_top_p);
        let repetition_penalty =
            (profile.repetition_penalty - energy_axis * 0.05 - intensity_axis * 0.025)
                .clamp(limits.min_repetition_penalty, limits.max_repetition_penalty);
        let fragment_interval = (profile.fragment_interval + pause_bias * 0.22 - intensity * 0.04)
            .clamp(limits.min_fragment_interval, limits.max_fragment_interval);
        let top_k = (15.0 + energy_axis * 7.0 + intensity_axis * 3.0)
            .round()
            .clamp(5.0, 30.0) as u32;
        let requested_pitch_shift_semitones =
            request.prosody.pitch_shift.clamp(-1.0, 1.0) * limits.max_pitch_shift_semitones;
        let pitch_shift_semitones = if self.sample_rate_pitch_fallback_enabled {
            requested_pitch_shift_semitones
        } else {
            0.0
        };
        let pitch_factor = if self.sample_rate_pitch_fallback_enabled {
            2.0_f32.powf(pitch_shift_semitones / 12.0)
        } else {
            1.0
        };
        if !self.sample_rate_pitch_fallback_enabled
            && requested_pitch_shift_semitones.abs() > f32::EPSILON
        {
            tracing::debug!(
                requested_pitch_shift_semitones,
                "GPT-SoVITS pitch shift skipped because sample-rate fallback is disabled"
            );
        }

        SovitsProsodyMapping {
            top_k,
            top_p,
            temperature,
            speed_factor,
            fragment_interval,
            repetition_penalty,
            pitch_factor,
            pitch_shift_semitones,
            requested_pitch_shift_semitones,
            sample_rate_pitch_fallback_enabled: self.sample_rate_pitch_fallback_enabled,
        }
    }

    fn provider_adjustments(&self, request: &TtsRequest) -> Vec<TtsProviderAdjustment> {
        let profile = self.profile_for_request(request);
        let prosody = self.map_prosody(profile, request);
        prosody.provider_adjustments()
    }

    fn tts_capabilities(&self) -> TtsCapabilities {
        TtsCapabilities {
            provider: "gpt_sovits".to_string(),
            supported_output_formats: vec!["audio/wav".to_string()],
            supports_streaming: true,
            supports_style: true,
            supports_intensity: true,
            supports_style_embedding: false,
            supports_emotion_labels: true,
            supports_reference_voice: true,
            supports_voice_cloning: false,
            supports_voice_selection: true,
            supports_multilingual_voice: true,
            supports_ssml: false,
            supports_phoneme_control: false,
            supports_sentence_level_prosody: true,
            supports_pitch: self.sample_rate_pitch_fallback_enabled,
            supports_rate: true,
            supports_energy: true,
            supports_pause: true,
            supports_streaming_prosody: true,
            prosody_limits: TtsProsodyLimits {
                min_rate: self.prosody_limits.min_speed_factor,
                max_rate: self.prosody_limits.max_speed_factor,
                min_pitch_shift: if self.sample_rate_pitch_fallback_enabled {
                    -1.0
                } else {
                    0.0
                },
                max_pitch_shift: if self.sample_rate_pitch_fallback_enabled {
                    1.0
                } else {
                    0.0
                },
                min_energy: 0.0,
                max_energy: 1.0,
                min_pause_bias: -1.0,
                max_pause_bias: 1.0,
            },
            max_text_chars: None,
            latency_class: crate::providers::adapter::TtsLatencyClass::Interactive,
            provider_specific: BTreeMap::from([
                (
                    "native_provider".to_string(),
                    serde_json::json!("gpt_sovits"),
                ),
                (
                    "pitch_control_mode".to_string(),
                    serde_json::json!(if self.sample_rate_pitch_fallback_enabled {
                        "sample_rate_fallback"
                    } else {
                        "unsupported"
                    }),
                ),
            ]),
        }
    }

    fn profile_for_request(&self, request: &TtsRequest) -> &SovitsVoiceProfile {
        let voice_profile = request
            .voice_profile
            .as_deref()
            .or_else(|| {
                request
                    .style
                    .as_deref()
                    .and_then(|style| self.expression_styles.get(style))
                    .and_then(|style| style.voice_profile.as_deref())
            })
            .unwrap_or(&self.default_voice);

        self.voice_profiles
            .get(voice_profile)
            .unwrap_or(&self.default_profile)
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn apply_pitch_shift_to_wav(bytes: &mut [u8], prosody: SovitsProsodyMapping) -> anyhow::Result<()> {
    if pitch_is_neutral(prosody.pitch_factor) {
        return Ok(());
    }
    anyhow::ensure!(
        canonical_pcm_prefix(bytes),
        "pitch fallback requires a canonical PCM WAV fmt header"
    );
    if let Some((from, to)) = rewrite_wav_sample_rate(bytes, prosody.pitch_factor) {
        tracing::debug!(
            pitch_shift_semitones = prosody.pitch_shift_semitones,
            sample_rate_from = from,
            sample_rate_to = to,
            "applied bounded GPT-SoVITS pitch shift via WAV sample-rate header"
        );
    }
    Ok(())
}

fn pitch_is_neutral(pitch_factor: f32) -> bool {
    (pitch_factor - 1.0).abs() <= 0.001
}

fn canonical_pcm_prefix(bytes: &[u8]) -> bool {
    if bytes.len() < 36
        || &bytes[..4] != b"RIFF"
        || &bytes[8..12] != b"WAVE"
        || &bytes[12..16] != b"fmt "
    {
        return false;
    }
    let read16 = |offset| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let read32 = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let channels = read16(22);
    let rate = read32(24);
    read32(16) >= 16
        && read16(20) == 1
        && (1..=8).contains(&channels)
        && (1..=192_000).contains(&rate)
        && read16(34) == 16
        && read16(32) == channels * 2
        && read32(28) == rate * u32::from(channels) * 2
}

fn rewrite_wav_sample_rate(bytes: &mut [u8], pitch_factor: f32) -> Option<(u32, u32)> {
    if !canonical_pcm_prefix(bytes) || pitch_is_neutral(pitch_factor) {
        return None;
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }

    let old_sample_rate = u32::from_le_bytes(bytes[24..28].try_into().ok()?);
    if old_sample_rate == 0 {
        return None;
    }
    let new_sample_rate = ((old_sample_rate as f32) * pitch_factor)
        .round()
        .clamp(1.0, u32::MAX as f32) as u32;
    if new_sample_rate == old_sample_rate {
        return None;
    }
    bytes[24..28].copy_from_slice(&new_sample_rate.to_le_bytes());

    if bytes.len() >= 34 {
        let block_align = u16::from_le_bytes(bytes[32..34].try_into().ok()?) as u32;
        if block_align > 0 && bytes.len() >= 32 {
            let byte_rate = new_sample_rate.saturating_mul(block_align);
            bytes[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        }
    }

    Some((old_sample_rate, new_sample_rate))
}

#[derive(Debug)]
struct StreamingWavPitchRewriter {
    prosody: SovitsProsodyMapping,
    pending: [u8; 36],
    len: usize,
    complete: bool,
}
impl StreamingWavPitchRewriter {
    fn new(prosody: SovitsProsodyMapping) -> Self {
        Self {
            prosody,
            pending: [0; 36],
            len: 0,
            complete: pitch_is_neutral(prosody.pitch_factor),
        }
    }
    fn push(&mut self, chunk: &[u8]) -> anyhow::Result<(usize, Option<[u8; 36]>)> {
        if self.complete {
            return Ok((0, None));
        }
        let count = chunk.len().min(36 - self.len);
        self.pending[self.len..self.len + count].copy_from_slice(&chunk[..count]);
        self.len += count;
        if self.len < 36 {
            return Ok((count, None));
        }
        apply_pitch_shift_to_wav(&mut self.pending, self.prosody)?;
        self.complete = true;
        Ok((count, Some(self.pending)))
    }
    fn finish(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.complete || self.len == 0,
            "TTS ended inside pitch WAV header"
        );
        Ok(())
    }
}

async fn configure_remote_weights(client: &Client, config: &SovitsConfig) -> anyhow::Result<()> {
    let timeout = Duration::from_secs(config.request_timeout_sec.max(1));
    if let Some(path) = &config.default_profile.gpt_weights_path {
        set_remote_weights(
            client,
            &config.endpoint,
            "set_gpt_weights",
            "weights_path",
            path,
            timeout,
        )
        .await?;
    }
    if let Some(path) = &config.default_profile.sovits_weights_path {
        set_remote_weights(
            client,
            &config.endpoint,
            "set_sovits_weights",
            "weights_path",
            path,
            timeout,
        )
        .await?;
    }
    Ok(())
}

async fn set_remote_weights(
    client: &Client,
    endpoint: &str,
    path: &str,
    param: &str,
    value: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let budget = ResponseAudioBudget::for_standalone_request(uuid::Uuid::new_v4())?;
    let reservation = budget.reserve(ERROR_BODY_BYTES).await?;
    let base = endpoint.trim_end_matches("/tts").trim_end_matches('/');
    let response = client
        .get(format!("{base}/{path}"))
        .query(&[(param, value)])
        .timeout(timeout)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = error_body(response, &reservation).await;
        anyhow::bail!("GPT-SoVITS {path} failed {status}: {body}");
    }

    Ok(())
}

#[async_trait]
impl ModuleLifecycle for GptSovitsTts {
    fn name(&self) -> &'static str {
        "gpt_sovits"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let server_config: &ServerConfig = &ctx.config;
        let provider_config = &server_config.providers.gpt_sovits;
        let voice_config = server_config.profiles.default_voice_profile();
        if voice_config.provider != "gpt_sovits" {
            tracing::warn!(
                provider = %voice_config.provider,
                "default voice profile is not configured for GPT-SoVITS; applying shared voice fields anyway"
            );
        }

        let configured = {
            let mut config = self.config.write().await;
            config.endpoint = provider_config.endpoint.clone();
            config.default_voice = server_config.profiles.default_voice.clone();
            config.default_profile = SovitsVoiceProfile::from_voice_config(&voice_config);
            config.voice_profiles = server_config
                .profiles
                .voice
                .iter()
                .map(|(name, profile)| {
                    (name.clone(), SovitsVoiceProfile::from_voice_config(profile))
                })
                .collect();
            let default_voice = config.default_voice.clone();
            let default_profile = config.default_profile.clone();
            config
                .voice_profiles
                .entry(default_voice)
                .or_insert(default_profile);
            config.expression_styles = server_config.profiles.expression_styles.clone();
            config.prosody_limits = SovitsProsodyLimits::from_provider_config(provider_config);
            config.sample_rate_pitch_fallback_enabled =
                provider_config.sample_rate_pitch_fallback_enabled;
            config.request_timeout_sec = provider_config.request_timeout_sec;
            config.stream_idle_timeout_sec = provider_config.stream_idle_timeout_sec;
            config.clone()
        };

        if let Err(error) = configure_remote_weights(&self.client, &configured).await {
            tracing::warn!(
                error = %error,
            endpoint = %configured.endpoint,
            model = configured.default_profile.model_name.as_deref().unwrap_or("unspecified"),
            "GPT-SoVITS remote weight setup failed; continuing with the model already loaded by the TTS server"
            );
        }

        tracing::info!(
            endpoint = %configured.endpoint,
            model = configured.default_profile.model_name.as_deref().unwrap_or("unspecified"),
            "GPT-SoVITS TTS initialized"
        );
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let config = self.config.read().await.clone();

        // Try to connect to GPT-SoVITS server
        match self
            .client
            .get(&config.endpoint)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 400 => {
                // 400 is expected without proper parameters
                ModuleHealth::Healthy
            }
            Ok(resp) => {
                ModuleHealth::Unhealthy(format!("GPT-SoVITS returned status {}", resp.status()))
            }
            Err(e) => ModuleHealth::Unhealthy(format!("Cannot connect to GPT-SoVITS: {}", e)),
        }
    }

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut applied = Vec::new();
        let configured = {
            let mut config = self.config.write().await;

            if let Some(value) = string_setting(updates, "core.providers.gpt_sovits.endpoint") {
                config.endpoint = value;
                applied.push("core.providers.gpt_sovits.endpoint".to_string());
            }
            if let Some(value) =
                u64_setting(updates, "core.providers.gpt_sovits.request_timeout_sec")
            {
                config.request_timeout_sec = value;
                applied.push("core.providers.gpt_sovits.request_timeout_sec".to_string());
            }
            if let Some(value) =
                u64_setting(updates, "core.providers.gpt_sovits.stream_idle_timeout_sec")
            {
                config.stream_idle_timeout_sec = value;
                applied.push("core.providers.gpt_sovits.stream_idle_timeout_sec".to_string());
            }
            if let Some(value) = f32_setting(updates, "core.providers.gpt_sovits.min_speed_factor")
            {
                config.prosody_limits.min_speed_factor = value.max(0.1);
                applied.push("core.providers.gpt_sovits.min_speed_factor".to_string());
            }
            if let Some(value) = f32_setting(updates, "core.providers.gpt_sovits.max_speed_factor")
            {
                config.prosody_limits.max_speed_factor =
                    value.max(config.prosody_limits.min_speed_factor);
                applied.push("core.providers.gpt_sovits.max_speed_factor".to_string());
            }
            if let Some(value) = f32_setting(updates, "core.providers.gpt_sovits.min_temperature") {
                config.prosody_limits.min_temperature = value.max(0.0);
                applied.push("core.providers.gpt_sovits.min_temperature".to_string());
            }
            if let Some(value) = f32_setting(updates, "core.providers.gpt_sovits.max_temperature") {
                config.prosody_limits.max_temperature =
                    value.max(config.prosody_limits.min_temperature);
                applied.push("core.providers.gpt_sovits.max_temperature".to_string());
            }
            if let Some(value) = f32_setting(updates, "core.providers.gpt_sovits.min_top_p") {
                config.prosody_limits.min_top_p = value.clamp(0.0, 1.0);
                applied.push("core.providers.gpt_sovits.min_top_p".to_string());
            }
            if let Some(value) = f32_setting(updates, "core.providers.gpt_sovits.max_top_p") {
                config.prosody_limits.max_top_p = value.clamp(config.prosody_limits.min_top_p, 1.0);
                applied.push("core.providers.gpt_sovits.max_top_p".to_string());
            }
            if let Some(value) =
                f32_setting(updates, "core.providers.gpt_sovits.min_repetition_penalty")
            {
                config.prosody_limits.min_repetition_penalty = value.max(0.0);
                applied.push("core.providers.gpt_sovits.min_repetition_penalty".to_string());
            }
            if let Some(value) =
                f32_setting(updates, "core.providers.gpt_sovits.max_repetition_penalty")
            {
                config.prosody_limits.max_repetition_penalty =
                    value.max(config.prosody_limits.min_repetition_penalty);
                applied.push("core.providers.gpt_sovits.max_repetition_penalty".to_string());
            }
            if let Some(value) =
                f32_setting(updates, "core.providers.gpt_sovits.min_fragment_interval")
            {
                config.prosody_limits.min_fragment_interval = value.max(0.0);
                applied.push("core.providers.gpt_sovits.min_fragment_interval".to_string());
            }
            if let Some(value) =
                f32_setting(updates, "core.providers.gpt_sovits.max_fragment_interval")
            {
                config.prosody_limits.max_fragment_interval =
                    value.max(config.prosody_limits.min_fragment_interval);
                applied.push("core.providers.gpt_sovits.max_fragment_interval".to_string());
            }
            if let Some(value) = f32_setting(
                updates,
                "core.providers.gpt_sovits.max_pitch_shift_semitones",
            ) {
                config.prosody_limits.max_pitch_shift_semitones = value.clamp(0.0, 12.0);
                applied.push("core.providers.gpt_sovits.max_pitch_shift_semitones".to_string());
            }
            if let Some(value) = bool_setting(
                updates,
                "core.providers.gpt_sovits.sample_rate_pitch_fallback_enabled",
            ) {
                config.sample_rate_pitch_fallback_enabled = value;
                applied.push(
                    "core.providers.gpt_sovits.sample_rate_pitch_fallback_enabled".to_string(),
                );
            }

            if let Some(value) = string_setting(updates, "core.profiles.voice.default.model_name") {
                config.default_profile.model_name = non_empty(Some(&value));
                applied.push("core.profiles.voice.default.model_name".to_string());
            }
            if let Some(value) =
                string_setting(updates, "core.profiles.voice.default.reference_voice")
            {
                config.default_profile.ref_audio_path = non_empty(Some(&value));
                applied.push("core.profiles.voice.default.reference_voice".to_string());
            }
            if let Some(value) = string_setting(updates, "core.profiles.voice.default.style_prompt")
            {
                config.default_profile.prompt_text = non_empty(Some(&value));
                applied.push("core.profiles.voice.default.style_prompt".to_string());
            }
            if let Some(value) = string_setting(updates, "core.profiles.voice.default.language") {
                config.default_profile.text_lang = value;
                applied.push("core.profiles.voice.default.language".to_string());
            }
            let provider_prefix = "core.profiles.voice.default.provider_specific.";
            for (key, field, value) in updates.iter().filter_map(|(key, value)| {
                key.strip_prefix(provider_prefix)
                    .map(|field| (key, field, value))
            }) {
                apply_sovits_provider_specific_setting(&mut config.default_profile, field, value);
                applied.push(key.clone());
            }
            if applied
                .iter()
                .any(|key| key.starts_with("core.profiles.voice.default."))
            {
                let default_voice = config.default_voice.clone();
                let default_profile = config.default_profile.clone();
                config.voice_profiles.insert(default_voice, default_profile);
            }

            config.clone()
        };

        if applied.iter().any(|key| {
            matches!(
                key.as_str(),
                "core.providers.gpt_sovits.endpoint"
                    | "core.profiles.voice.default.provider_specific.gpt_weights_path"
                    | "core.profiles.voice.default.provider_specific.sovits_weights_path"
            )
        }) && let Err(error) = configure_remote_weights(&self.client, &configured).await
        {
            tracing::warn!(
                error = %error,
                endpoint = %configured.endpoint,
                "GPT-SoVITS runtime weight setup failed; keeping updated request config"
            );
        }

        if !applied.is_empty() {
            tracing::info!(module = self.name(), applied = ?applied, "runtime settings applied");
        }
        Ok(applied)
    }
}

#[async_trait]
impl TtsAdapter for GptSovitsTts {
    async fn synthesize_request(
        &self,
        tts: &TtsRequest,
        budget: ResponseAudioBudget,
    ) -> anyhow::Result<EncodedAudio> {
        let config = self.config.read().await.clone();
        let (request, prosody) = config.request(tts, 0);
        let reservation = budget.reserve(MAX_ENCODED_PAYLOAD_BYTES).await?;
        let response = self
            .client
            .post(&config.endpoint)
            .json(&request)
            .timeout(Duration::from_secs(config.request_timeout_sec.max(1)))
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = error_body(response, &reservation).await;
            anyhow::bail!("GPT-SoVITS API error {status}: {body}");
        }
        let mut data = read_body(response, &reservation, MAX_ENCODED_PAYLOAD_BYTES).await?;
        apply_pitch_shift_to_wav(&mut data, prosody)?;
        reservation.seal(data)
    }

    async fn synthesize_stream_request(
        &self,
        tts: &TtsRequest,
        budget: ResponseAudioBudget,
    ) -> anyhow::Result<TtsAudioStream> {
        let config = self.config.read().await.clone();
        let (request, prosody) = config.request(tts, 1);
        let mut header_reservation = if pitch_is_neutral(prosody.pitch_factor) {
            None
        } else {
            Some(budget.reserve(36).await?)
        };
        let first = budget.reserve(MAX_ENCODED_PAYLOAD_BYTES).await?;
        let response = self
            .client
            .post(&config.endpoint)
            .json(&request)
            .timeout(Duration::from_secs(config.request_timeout_sec.max(1)))
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = error_body(response, &first).await;
            anyhow::bail!("GPT-SoVITS API error {status}: {body}");
        }
        check_length(&response, MAX_TTS_STREAM_BYTES)?;
        let idle_timeout = Duration::from_secs(config.stream_idle_timeout_sec.max(1));
        Ok(Box::pin(async_stream::try_stream! {
            let mut chunks = response.bytes_stream();
            let mut first = Some(first);
            let mut pitch = StreamingWavPitchRewriter::new(prosody);
            let mut total = 0usize;
            loop {
                // At the exact cumulative limit only probe EOF. More input is
                // rejected before any application copy or retained payload.
                let reservation = match first.take() {
                    Some(first) => first,
                    None => budget.reserve(if total == MAX_TTS_STREAM_BYTES {0} else {MAX_ENCODED_PAYLOAD_BYTES}).await?,
                };
                let chunk = tokio::time::timeout(idle_timeout, chunks.next()).await
                    .map_err(|_| anyhow::anyhow!("GPT-SoVITS stream idle timeout after {}s", idle_timeout.as_secs()))?;
                let Some(chunk) = chunk else { pitch.finish()?; break; };
                let chunk = chunk?;
                if chunk.len() > MAX_ENCODED_PAYLOAD_BYTES {
                    Err(anyhow::anyhow!("TTS stream payload exceeds 1 MiB"))?;
                }
                if chunk.len() > MAX_TTS_STREAM_BYTES - total {
                    Err(anyhow::anyhow!("TTS stream exceeds 8 MiB cumulative bytes"))?;
                }
                total += chunk.len();
                let (used, header) = pitch.push(&chunk)?;
                let header = match header {
                    Some(header) => Some(header_reservation.take().expect("reserved before header copy").seal(header.to_vec())?),
                    None => None,
                };
                let data = if used < chunk.len() { Some(reservation.seal(chunk[used..].to_vec())?) } else { None };
                drop(chunk);
                if let Some(header) = header { yield header; }
                if let Some(data) = data { yield data; }
            }
        }))
    }

    async fn provider_adjustments(&self, request: &TtsRequest) -> Vec<TtsProviderAdjustment> {
        let config = self.config.read().await.clone();
        let mut adjustments = config.tts_capabilities().provider_adjustments(request);
        let provider_specific_adjustments = config.provider_adjustments(request);
        if provider_specific_adjustments
            .iter()
            .any(|adjustment| adjustment.path == "speech.prosody.pitch_shift")
        {
            adjustments.retain(|adjustment| adjustment.path != "speech.prosody.pitch_shift");
        }
        adjustments.extend(provider_specific_adjustments);
        for adjustment in &adjustments {
            tracing::info!(
                provider = adjustment.provider,
                path = adjustment.path,
                reason = adjustment.reason,
                from = %adjustment.from,
                to = %adjustment.to,
                "TTS provider degraded requested expression prosody"
            );
        }
        adjustments
    }

    async fn tts_capabilities(&self) -> TtsCapabilities {
        let config = self.config.read().await;
        config.tts_capabilities()
    }
}

#[cfg(test)]
mod tests {
    mod http;
    use super::*;
    use crate::expression::ProsodyControls;

    #[test]
    fn prosody_maps_to_sovits_generation_params_with_limits() {
        let mut config = test_config();
        config.prosody_limits.max_speed_factor = 1.1;
        config.prosody_limits.max_pitch_shift_semitones = 2.0;
        let request = TtsRequest {
            text: "hello".to_string(),
            style: Some("cheerful".to_string()),
            intensity: 0.8,
            voice_profile: None,
            prosody: ProsodyControls {
                rate: 1.4,
                pitch_shift: 0.5,
                energy: 0.9,
                pause_bias: -0.3,
            },
        };

        let (sovits, prosody) = config.request(&request, 0);

        assert_eq!(sovits.speed_factor, 1.1);
        assert!(sovits.temperature > 1.0);
        assert!(sovits.top_p > 0.94);
        assert!(sovits.repetition_penalty < config.default_profile.repetition_penalty);
        assert!(sovits.fragment_interval < config.default_profile.fragment_interval);
        assert_eq!(prosody.pitch_factor, 1.0);
        assert_eq!(prosody.pitch_shift_semitones, 0.0);
    }

    #[test]
    fn pitch_degradation_is_reported_as_provider_adjustment() {
        let mut config = test_config();
        config.prosody_limits.max_pitch_shift_semitones = 2.0;
        let request = TtsRequest {
            text: "hello".to_string(),
            style: Some("cheerful".to_string()),
            intensity: 0.8,
            voice_profile: None,
            prosody: ProsodyControls {
                rate: 1.0,
                pitch_shift: 0.5,
                energy: 0.5,
                pause_bias: 0.0,
            },
        };

        let adjustments = config.provider_adjustments(&request);

        assert_eq!(adjustments.len(), 1);
        assert_eq!(
            adjustments[0].reason,
            "provider_pitch_control_unavailable_sample_rate_fallback_disabled"
        );
        assert_eq!(adjustments[0].path, "speech.prosody.pitch_shift");
        assert_eq!(adjustments[0].from, serde_json::json!(1.0));
        assert_eq!(adjustments[0].to, serde_json::json!(0.0));
    }

    #[test]
    fn pitch_shift_mapping_requires_sample_rate_fallback_opt_in() {
        let mut config = test_config();
        config.sample_rate_pitch_fallback_enabled = true;
        config.prosody_limits.max_pitch_shift_semitones = 2.0;
        let request = TtsRequest {
            text: "hello".to_string(),
            style: Some("cheerful".to_string()),
            intensity: 0.8,
            voice_profile: None,
            prosody: ProsodyControls {
                rate: 1.0,
                pitch_shift: 0.5,
                energy: 0.5,
                pause_bias: 0.0,
            },
        };

        let (_sovits, prosody) = config.request(&request, 0);

        assert!(prosody.pitch_factor > 1.0);
        assert_eq!(prosody.pitch_shift_semitones, 1.0);
    }

    #[test]
    fn pause_bias_expands_fragment_interval() {
        let config = test_config();
        let request = TtsRequest {
            text: "hello".to_string(),
            style: None,
            intensity: 0.2,
            voice_profile: None,
            prosody: ProsodyControls {
                rate: 1.0,
                pitch_shift: 0.0,
                energy: 0.4,
                pause_bias: 0.8,
            },
        };

        let (sovits, _prosody) = config.request(&request, 0);

        assert!(sovits.fragment_interval > config.default_profile.fragment_interval);
    }

    #[test]
    fn pitch_shift_rewrites_wav_sample_rate() {
        let mut wav = minimal_wav_header(24_000);
        let prosody = SovitsProsodyMapping {
            top_k: 15,
            top_p: 1.0,
            temperature: 1.0,
            speed_factor: 1.0,
            fragment_interval: 0.3,
            repetition_penalty: 1.35,
            pitch_factor: 2.0,
            pitch_shift_semitones: 12.0,
            requested_pitch_shift_semitones: 12.0,
            sample_rate_pitch_fallback_enabled: true,
        };

        apply_pitch_shift_to_wav(&mut wav, prosody).unwrap();

        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 48_000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 96_000);
    }

    #[test]
    fn streaming_pitch_rewriter_buffers_until_wav_header_is_available() {
        let wav = minimal_wav_header(24_000);
        let prosody = SovitsProsodyMapping {
            top_k: 15,
            top_p: 1.0,
            temperature: 1.0,
            speed_factor: 1.0,
            fragment_interval: 0.3,
            repetition_penalty: 1.35,
            pitch_factor: 2.0,
            pitch_shift_semitones: 12.0,
            requested_pitch_shift_semitones: 12.0,
            sample_rate_pitch_fallback_enabled: true,
        };
        let mut rewriter = StreamingWavPitchRewriter::new(prosody);

        assert_eq!(rewriter.push(&wav[..12]).unwrap(), (12, None));
        let (used, rewritten) = rewriter.push(&wav[12..]).unwrap();
        assert_eq!(used, 24);
        let rewritten = rewritten.unwrap();
        rewriter.finish().unwrap();
        assert_eq!(
            u32::from_le_bytes(rewritten[24..28].try_into().unwrap()),
            48_000
        );
    }

    fn test_config() -> SovitsConfig {
        SovitsConfig {
            endpoint: "http://localhost:9880/tts".to_string(),
            default_voice: "default".to_string(),
            default_profile: SovitsVoiceProfile::default(),
            voice_profiles: BTreeMap::new(),
            expression_styles: BTreeMap::new(),
            prosody_limits: SovitsProsodyLimits::default(),
            sample_rate_pitch_fallback_enabled: false,
            request_timeout_sec: 60,
            stream_idle_timeout_sec: 10,
        }
    }

    fn minimal_wav_header(sample_rate: u32) -> Vec<u8> {
        let channels = 1u16;
        let bits_per_sample = 16u16;
        let block_align = channels * bits_per_sample / 8;
        let byte_rate = sample_rate * block_align as u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&bits_per_sample.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&0u32.to_le_bytes());
        wav
    }
}
