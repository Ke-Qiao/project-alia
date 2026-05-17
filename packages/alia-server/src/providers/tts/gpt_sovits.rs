//! GPT-SoVITS TTS Adapter
//!
//! Implements TtsAdapter for GPT-SoVITS server with streaming support.
//! Connects to GPT-SoVITS HTTP API for high-quality voice synthesis.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::config::ServerConfig;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::TtsAdapter;
use crate::settings::values::{
    bool_setting, f32_setting, i32_setting, optional_string_setting, string_setting,
    string_vec_setting, u32_setting, u64_setting,
};

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
    request_timeout_sec: u64,
    stream_idle_timeout_sec: u64,
}

impl GptSovitsTts {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            config: Arc::new(RwLock::new(SovitsConfig {
                endpoint: "http://localhost:9880".to_string(),
                model_name: None,
                gpt_weights_path: None,
                sovits_weights_path: None,
                ref_audio_path: None,
                aux_ref_audio_paths: Vec::new(),
                prompt_text: None,
                text_lang: "zh".to_string(),
                prompt_lang: "zh".to_string(),
                text_split_method: "cut5".to_string(),
                batch_size: 1,
                batch_threshold: 0.75,
                split_bucket: true,
                speed_factor: 1.0,
                fragment_interval: 0.3,
                seed: -1,
                parallel_infer: true,
                repetition_penalty: 1.35,
                sample_steps: 32,
                super_sampling: false,
                overlap_length: 2,
                min_chunk_length: 16,
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

impl SovitsConfig {
    fn request(&self, text: &str, streaming_mode: u8) -> SovitsRequest {
        SovitsRequest {
            text: text.to_string(),
            text_lang: Some(self.text_lang.clone()),
            ref_audio_path: self.ref_audio_path.clone(),
            aux_ref_audio_paths: self.aux_ref_audio_paths.clone(),
            prompt_text: self.prompt_text.clone(),
            prompt_lang: Some(self.prompt_lang.clone()),
            top_k: 15,
            top_p: 1.0,
            temperature: 1.0,
            text_split_method: self.text_split_method.clone(),
            batch_size: self.batch_size,
            batch_threshold: self.batch_threshold,
            split_bucket: self.split_bucket,
            speed_factor: self.speed_factor,
            fragment_interval: self.fragment_interval,
            seed: self.seed,
            parallel_infer: self.parallel_infer,
            repetition_penalty: self.repetition_penalty,
            sample_steps: self.sample_steps,
            super_sampling: self.super_sampling,
            media_type: Some("wav"),
            streaming_mode: Some(streaming_mode),
            overlap_length: self.overlap_length,
            min_chunk_length: self.min_chunk_length,
        }
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

async fn configure_remote_weights(client: &Client, config: &SovitsConfig) -> anyhow::Result<()> {
    let timeout = Duration::from_secs(config.request_timeout_sec.max(1));
    if let Some(path) = &config.gpt_weights_path {
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
    if let Some(path) = &config.sovits_weights_path {
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
    let base = endpoint.trim_end_matches("/tts").trim_end_matches('/');
    let response = client
        .get(format!("{base}/{path}"))
        .query(&[(param, value)])
        .timeout(timeout)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
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
            config.model_name = non_empty(voice_config.model_name.as_deref());
            config.gpt_weights_path = non_empty(voice_config.gpt_weights_path.as_deref());
            config.sovits_weights_path = non_empty(voice_config.sovits_weights_path.as_deref());
            config.ref_audio_path = voice_config
                .ref_audio_path
                .as_ref()
                .filter(|value| !value.trim().is_empty())
                .cloned();
            config.aux_ref_audio_paths = voice_config
                .aux_ref_audio_paths
                .iter()
                .filter(|value| !value.trim().is_empty())
                .cloned()
                .collect();
            config.prompt_text = voice_config
                .prompt_text
                .as_ref()
                .filter(|value| !value.trim().is_empty())
                .cloned();
            config.text_lang = voice_config.text_lang.clone();
            config.prompt_lang = voice_config.prompt_lang.clone();
            config.text_split_method = voice_config.text_split_method.clone();
            config.batch_size = voice_config.batch_size;
            config.batch_threshold = voice_config.batch_threshold;
            config.split_bucket = voice_config.split_bucket;
            config.speed_factor = voice_config.speed_factor;
            config.fragment_interval = voice_config.fragment_interval;
            config.seed = voice_config.seed;
            config.parallel_infer = voice_config.parallel_infer;
            config.repetition_penalty = voice_config.repetition_penalty;
            config.sample_steps = voice_config.sample_steps;
            config.super_sampling = voice_config.super_sampling;
            config.overlap_length = voice_config.overlap_length;
            config.min_chunk_length = voice_config.min_chunk_length;
            config.request_timeout_sec = provider_config.request_timeout_sec;
            config.stream_idle_timeout_sec = provider_config.stream_idle_timeout_sec;
            config.clone()
        };

        if let Err(error) = configure_remote_weights(&self.client, &configured).await {
            tracing::warn!(
                error = %error,
                endpoint = %configured.endpoint,
                model = configured.model_name.as_deref().unwrap_or("unspecified"),
                "GPT-SoVITS remote weight setup failed; continuing with the model already loaded by the TTS server"
            );
        }

        tracing::info!(
            endpoint = %configured.endpoint,
            model = configured.model_name.as_deref().unwrap_or("unspecified"),
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

            if let Some(value) =
                optional_string_setting(updates, "core.profiles.voice.default.model_name")
            {
                config.model_name = value;
                applied.push("core.profiles.voice.default.model_name".to_string());
            }
            if let Some(value) =
                optional_string_setting(updates, "core.profiles.voice.default.gpt_weights_path")
            {
                config.gpt_weights_path = value;
                applied.push("core.profiles.voice.default.gpt_weights_path".to_string());
            }
            if let Some(value) =
                optional_string_setting(updates, "core.profiles.voice.default.sovits_weights_path")
            {
                config.sovits_weights_path = value;
                applied.push("core.profiles.voice.default.sovits_weights_path".to_string());
            }
            if let Some(value) =
                optional_string_setting(updates, "core.profiles.voice.default.ref_audio_path")
            {
                config.ref_audio_path = value;
                applied.push("core.profiles.voice.default.ref_audio_path".to_string());
            }
            if let Some(value) =
                string_vec_setting(updates, "core.profiles.voice.default.aux_ref_audio_paths")
            {
                config.aux_ref_audio_paths = value;
                applied.push("core.profiles.voice.default.aux_ref_audio_paths".to_string());
            }
            if let Some(value) =
                optional_string_setting(updates, "core.profiles.voice.default.prompt_text")
            {
                config.prompt_text = value;
                applied.push("core.profiles.voice.default.prompt_text".to_string());
            }
            if let Some(value) = string_setting(updates, "core.profiles.voice.default.text_lang") {
                config.text_lang = value;
                applied.push("core.profiles.voice.default.text_lang".to_string());
            }
            if let Some(value) = string_setting(updates, "core.profiles.voice.default.prompt_lang")
            {
                config.prompt_lang = value;
                applied.push("core.profiles.voice.default.prompt_lang".to_string());
            }
            if let Some(value) =
                string_setting(updates, "core.profiles.voice.default.text_split_method")
            {
                config.text_split_method = value;
                applied.push("core.profiles.voice.default.text_split_method".to_string());
            }
            if let Some(value) = u32_setting(updates, "core.profiles.voice.default.batch_size") {
                config.batch_size = value;
                applied.push("core.profiles.voice.default.batch_size".to_string());
            }
            if let Some(value) = f32_setting(updates, "core.profiles.voice.default.batch_threshold")
            {
                config.batch_threshold = value;
                applied.push("core.profiles.voice.default.batch_threshold".to_string());
            }
            if let Some(value) = bool_setting(updates, "core.profiles.voice.default.split_bucket") {
                config.split_bucket = value;
                applied.push("core.profiles.voice.default.split_bucket".to_string());
            }
            if let Some(value) = f32_setting(updates, "core.profiles.voice.default.speed_factor") {
                config.speed_factor = value;
                applied.push("core.profiles.voice.default.speed_factor".to_string());
            }
            if let Some(value) =
                f32_setting(updates, "core.profiles.voice.default.fragment_interval")
            {
                config.fragment_interval = value;
                applied.push("core.profiles.voice.default.fragment_interval".to_string());
            }
            if let Some(value) = i32_setting(updates, "core.profiles.voice.default.seed") {
                config.seed = value;
                applied.push("core.profiles.voice.default.seed".to_string());
            }
            if let Some(value) = bool_setting(updates, "core.profiles.voice.default.parallel_infer")
            {
                config.parallel_infer = value;
                applied.push("core.profiles.voice.default.parallel_infer".to_string());
            }
            if let Some(value) =
                f32_setting(updates, "core.profiles.voice.default.repetition_penalty")
            {
                config.repetition_penalty = value;
                applied.push("core.profiles.voice.default.repetition_penalty".to_string());
            }
            if let Some(value) = u32_setting(updates, "core.profiles.voice.default.sample_steps") {
                config.sample_steps = value;
                applied.push("core.profiles.voice.default.sample_steps".to_string());
            }
            if let Some(value) = bool_setting(updates, "core.profiles.voice.default.super_sampling")
            {
                config.super_sampling = value;
                applied.push("core.profiles.voice.default.super_sampling".to_string());
            }
            if let Some(value) = u32_setting(updates, "core.profiles.voice.default.overlap_length")
            {
                config.overlap_length = value;
                applied.push("core.profiles.voice.default.overlap_length".to_string());
            }
            if let Some(value) =
                u32_setting(updates, "core.profiles.voice.default.min_chunk_length")
            {
                config.min_chunk_length = value;
                applied.push("core.profiles.voice.default.min_chunk_length".to_string());
            }

            config.clone()
        };

        if applied.iter().any(|key| {
            matches!(
                key.as_str(),
                "core.providers.gpt_sovits.endpoint"
                    | "core.profiles.voice.default.gpt_weights_path"
                    | "core.profiles.voice.default.sovits_weights_path"
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
    async fn synthesize(&self, text: &str) -> anyhow::Result<Vec<u8>> {
        let config = self.config.read().await.clone();

        let request = config.request(text, 0);

        let response = self
            .client
            .post(&config.endpoint)
            .json(&request)
            .timeout(Duration::from_secs(config.request_timeout_sec.max(1)))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GPT-SoVITS API error {}: {}", status, body);
        }

        let audio_bytes = response.bytes().await?;
        Ok(audio_bytes.to_vec())
    }

    async fn synthesize_stream(
        &self,
        text: &str,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<Vec<u8>>> + Send>>> {
        let config = self.config.read().await.clone();

        let request = config.request(text, 1);

        let response = self
            .client
            .post(&config.endpoint)
            .json(&request)
            .timeout(Duration::from_secs(config.request_timeout_sec.max(1)))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GPT-SoVITS API error {}: {}", status, body);
        }

        let idle_timeout = Duration::from_secs(config.stream_idle_timeout_sec.max(1));
        let stream = async_stream::stream! {
            let mut byte_stream = response.bytes_stream();

            loop {
                match tokio::time::timeout(idle_timeout, byte_stream.next()).await {
                    Ok(Some(Ok(chunk))) => {
                        yield Ok(chunk.to_vec());
                    }
                    Ok(Some(Err(e))) => {
                        yield Err(anyhow::anyhow!("Stream error: {}", e));
                        break;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        yield Err(anyhow::anyhow!(
                            "GPT-SoVITS stream idle timeout after {}s",
                            idle_timeout.as_secs()
                        ));
                        break;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}
