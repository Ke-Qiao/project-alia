//! GPT-SoVITS TTS Adapter
//!
//! Implements TtsAdapter for GPT-SoVITS server with streaming support.
//! Connects to GPT-SoVITS HTTP API for high-quality voice synthesis.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::config::ServerConfig;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::TtsAdapter;

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
    if let Some(path) = &config.gpt_weights_path {
        set_remote_weights(
            client,
            &config.endpoint,
            "set_gpt_weights",
            "weights_path",
            path,
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
) -> anyhow::Result<()> {
    let base = endpoint.trim_end_matches("/tts").trim_end_matches('/');
    let response = client
        .get(format!("{base}/{path}"))
        .query(&[(param, value)])
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

        let mut config = self.config.write().await;
        config.endpoint = server_config.tts.gpt_sovits_endpoint.clone();
        config.model_name = non_empty(server_config.tts.gpt_sovits_model_name.as_deref());
        config.gpt_weights_path =
            non_empty(server_config.tts.gpt_sovits_gpt_weights_path.as_deref());
        config.sovits_weights_path =
            non_empty(server_config.tts.gpt_sovits_sovits_weights_path.as_deref());
        config.ref_audio_path = server_config
            .tts
            .gpt_sovits_ref_audio_path
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .cloned();
        config.aux_ref_audio_paths = server_config
            .tts
            .gpt_sovits_aux_ref_audio_paths
            .iter()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .collect();
        config.prompt_text = server_config
            .tts
            .gpt_sovits_prompt_text
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .cloned();
        config.text_lang = server_config.tts.gpt_sovits_text_lang.clone();
        config.prompt_lang = server_config.tts.gpt_sovits_prompt_lang.clone();
        config.text_split_method = server_config.tts.gpt_sovits_text_split_method.clone();
        config.batch_size = server_config.tts.gpt_sovits_batch_size;
        config.batch_threshold = server_config.tts.gpt_sovits_batch_threshold;
        config.split_bucket = server_config.tts.gpt_sovits_split_bucket;
        config.speed_factor = server_config.tts.gpt_sovits_speed_factor;
        config.fragment_interval = server_config.tts.gpt_sovits_fragment_interval;
        config.seed = server_config.tts.gpt_sovits_seed;
        config.parallel_infer = server_config.tts.gpt_sovits_parallel_infer;
        config.repetition_penalty = server_config.tts.gpt_sovits_repetition_penalty;
        config.sample_steps = server_config.tts.gpt_sovits_sample_steps;
        config.super_sampling = server_config.tts.gpt_sovits_super_sampling;
        config.overlap_length = server_config.tts.gpt_sovits_overlap_length;
        config.min_chunk_length = server_config.tts.gpt_sovits_min_chunk_length;

        configure_remote_weights(&self.client, &config).await?;

        tracing::info!(
            endpoint = %config.endpoint,
            model = config.model_name.as_deref().unwrap_or("unspecified"),
            "GPT-SoVITS TTS initialized"
        );
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let config = self.config.read().await;

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
}

#[async_trait]
impl TtsAdapter for GptSovitsTts {
    async fn synthesize(&self, text: &str) -> anyhow::Result<Vec<u8>> {
        let config = self.config.read().await;

        let request = config.request(text, 0);

        let response = self
            .client
            .post(&config.endpoint)
            .json(&request)
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
        let config = self.config.read().await;

        let request = config.request(text, 1);

        let response = self
            .client
            .post(&config.endpoint)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GPT-SoVITS API error {}: {}", status, body);
        }

        let stream = async_stream::stream! {
            let mut byte_stream = response.bytes_stream();

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        yield Ok(chunk.to_vec());
                    }
                    Err(e) => {
                        yield Err(anyhow::anyhow!("Stream error: {}", e));
                        break;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}
