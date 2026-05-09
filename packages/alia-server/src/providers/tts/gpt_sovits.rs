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
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    streaming: Option<bool>,
}

/// GPT-SoVITS TTS adapter
pub struct GptSovitsTts {
    client: Client,
    config: Arc<RwLock<SovitsConfig>>,
}

#[derive(Debug, Clone)]
struct SovitsConfig {
    endpoint: String,
    ref_audio_path: Option<String>,
    prompt_text: Option<String>,
    language: String,
}

impl GptSovitsTts {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            config: Arc::new(RwLock::new(SovitsConfig {
                endpoint: "http://localhost:9880".to_string(),
                ref_audio_path: None,
                prompt_text: None,
                language: "zh".to_string(),
            })),
        }
    }
}

impl Default for GptSovitsTts {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for GptSovitsTts {
    fn name(&self) -> &'static str {
        "gpt_sovits"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let server_config: &ServerConfig = &ctx.config;

        let mut config = self.config.write().await;
        config.endpoint = server_config.ai.gpt_sovits_tts_endpoint.clone();
        // ref_audio_path and prompt_text can be added to config if needed

        tracing::info!(endpoint = %config.endpoint, "GPT-SoVITS TTS initialized");
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

        let request = SovitsRequest {
            text: text.to_string(),
            text_lang: Some(config.language.clone()),
            ref_audio_path: config.ref_audio_path.clone(),
            prompt_text: config.prompt_text.clone(),
            prompt_lang: Some(config.language.clone()),
            streaming: Some(false),
        };

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

        let request = SovitsRequest {
            text: text.to_string(),
            text_lang: Some(config.language.clone()),
            ref_audio_path: config.ref_audio_path.clone(),
            prompt_text: config.prompt_text.clone(),
            prompt_lang: Some(config.language.clone()),
            streaming: Some(true),
        };

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
