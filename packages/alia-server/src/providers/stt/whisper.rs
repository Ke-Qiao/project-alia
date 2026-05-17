//! Whisper.cpp STT Adapter
//!
//! Implements SttAdapter for whisper.cpp server with streaming support.
//! Connects to a whisper.cpp HTTP server for speech-to-text transcription.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::config::ServerConfig;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{AudioEncoding, SttAdapter, TranscribeChunk};

/// Whisper.cpp transcription response
#[derive(Debug, Deserialize)]
struct WhisperResponse {
    text: String,
}

/// Whisper.cpp streaming response segment
#[derive(Debug, Deserialize)]
struct WhisperSegment {
    text: String,
    #[serde(rename = "isPartial")]
    is_partial: Option<bool>,
}

/// Whisper.cpp STT adapter
pub struct WhisperCppStt {
    client: Client,
    config: Arc<RwLock<WhisperConfig>>,
}

#[derive(Debug, Clone)]
struct WhisperConfig {
    endpoint: String,
    language: String,
    request_timeout_sec: u64,
}

impl WhisperCppStt {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            config: Arc::new(RwLock::new(WhisperConfig {
                endpoint: "http://localhost:8080".to_string(),
                language: "auto".to_string(),
                request_timeout_sec: 30,
            })),
        }
    }

    fn encoding_to_mime(encoding: AudioEncoding) -> &'static str {
        match encoding {
            AudioEncoding::Pcm16kMono => "audio/wav",
            AudioEncoding::Opus => "audio/opus",
            AudioEncoding::Wav => "audio/wav",
        }
    }

    fn normalize_audio_for_upload(audio: &[u8], encoding: AudioEncoding) -> Vec<u8> {
        match encoding {
            AudioEncoding::Pcm16kMono => pcm16le_to_wav(audio, 16_000, 1),
            AudioEncoding::Opus | AudioEncoding::Wav => audio.to_vec(),
        }
    }
}

impl Default for WhisperCppStt {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for WhisperCppStt {
    fn name(&self) -> &'static str {
        "whisper"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let server_config: &ServerConfig = &ctx.config;

        let mut config = self.config.write().await;
        config.endpoint = server_config.providers.whisper_cpp.endpoint.clone();
        config.request_timeout_sec = server_config.providers.whisper_cpp.request_timeout_sec;

        tracing::info!(endpoint = %config.endpoint, "Whisper STT initialized");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let config = self.config.read().await.clone();

        // Try to connect to whisper.cpp server
        match self
            .client
            .get(format!("{}/health", config.endpoint))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => ModuleHealth::Healthy,
            Ok(resp) => {
                ModuleHealth::Unhealthy(format!("Whisper returned status {}", resp.status()))
            }
            Err(e) => ModuleHealth::Unhealthy(format!("Cannot connect to Whisper: {}", e)),
        }
    }
}

#[async_trait]
impl SttAdapter for WhisperCppStt {
    async fn transcribe(&self, audio: &[u8], encoding: AudioEncoding) -> anyhow::Result<String> {
        let config = self.config.read().await.clone();

        let upload_audio = Self::normalize_audio_for_upload(audio, encoding);
        let form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(upload_audio)
                    .file_name("audio.wav")
                    .mime_str(Self::encoding_to_mime(encoding))?,
            )
            .text("language", config.language.clone())
            .text("response_format", "json");

        let response = self
            .client
            .post(format!("{}/inference", config.endpoint))
            .multipart(form)
            .timeout(Duration::from_secs(config.request_timeout_sec.max(1)))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Whisper API error {}: {}", status, body);
        }

        let whisper_response: WhisperResponse = response.json().await?;
        Ok(whisper_response.text.trim().to_string())
    }

    async fn transcribe_stream(
        &self,
        audio_stream: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
        encoding: AudioEncoding,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<TranscribeChunk>> + Send>>> {
        let config = self.config.read().await.clone();
        let client = self.client.clone();
        let request_timeout = Duration::from_secs(config.request_timeout_sec.max(1));

        let stream = async_stream::stream! {
            let mut audio_stream = audio_stream;
            let mut accumulated = Vec::new();

            while let Some(chunk) = audio_stream.next().await {
                accumulated.extend(chunk);

                // Process audio in chunks (e.g., every ~2 seconds of audio)
                // For 16kHz 16-bit mono: 2 seconds = 64000 bytes
                if accumulated.len() >= 64000 {
                    let form = reqwest::multipart::Form::new()
                        .part(
                            "file",
                            reqwest::multipart::Part::bytes(Self::normalize_audio_for_upload(&accumulated, encoding))
                                .file_name("audio.wav")
                                .mime_str(Self::encoding_to_mime(encoding))
                                .unwrap(),
                        )
                        .text("language", config.language.clone())
                        .text("response_format", "json");

                    match client
                        .post(format!("{}/inference", config.endpoint))
                        .multipart(form)
                        .timeout(request_timeout)
                        .send()
                        .await
                    {
                        Ok(response) if response.status().is_success() => {
                            if let Ok(whisper_response) = response.json::<WhisperResponse>().await {
                                yield Ok(TranscribeChunk {
                                    text: whisper_response.text.trim().to_string(),
                                    is_final: false,
                                });
                            }
                        }
                        Ok(response) => {
                            let status = response.status();
                            yield Err(anyhow::anyhow!("Whisper error: {}", status));
                        }
                        Err(e) => {
                            yield Err(anyhow::anyhow!("Request error: {}", e));
                        }
                    }

                    accumulated.clear();
                }
            }

            // Process remaining audio
            if !accumulated.is_empty() {
                let form = reqwest::multipart::Form::new()
                    .part(
                        "file",
                        reqwest::multipart::Part::bytes(Self::normalize_audio_for_upload(&accumulated, encoding))
                            .file_name("audio.wav")
                            .mime_str(Self::encoding_to_mime(encoding))
                            .unwrap(),
                    )
                    .text("language", config.language.clone())
                    .text("response_format", "json");

                match client
                    .post(format!("{}/inference", config.endpoint))
                    .multipart(form)
                    .timeout(request_timeout)
                    .send()
                    .await
                {
                    Ok(response) if response.status().is_success() => {
                        if let Ok(whisper_response) = response.json::<WhisperResponse>().await {
                            yield Ok(TranscribeChunk {
                                text: whisper_response.text.trim().to_string(),
                                is_final: true,
                            });
                        }
                    }
                    Ok(response) => {
                        let status = response.status();
                        yield Err(anyhow::anyhow!("Whisper error: {}", status));
                    }
                    Err(e) => {
                        yield Err(anyhow::anyhow!("Request error: {}", e));
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

fn pcm16le_to_wav(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let byte_rate = sample_rate * channels as u32 * 2;
    let block_align = channels * 2;
    let mut wav = Vec::with_capacity(44 + pcm.len());

    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);

    wav
}
