//! Whisper.cpp STT Adapter
//!
//! Implements SttAdapter for whisper.cpp server with streaming support.
//! Connects to a whisper.cpp HTTP server for speech-to-text transcription.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::config::ServerConfig;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    AudioEncoding, SttAdapter, SttCapabilities, TranscribeChunk, TranscriptionResult,
    TranscriptionSegment, TranscriptionWord,
};
use crate::settings::values::{string_setting, u64_setting};

/// Whisper.cpp transcription response
#[derive(Debug, Deserialize)]
struct WhisperResponse {
    text: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    segments: Vec<WhisperResponseSegment>,
}

#[derive(Debug, Deserialize)]
struct WhisperResponseSegment {
    text: String,
    #[serde(default)]
    start: Option<f64>,
    #[serde(default)]
    end: Option<f64>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    probability: Option<f32>,
    #[serde(default)]
    avg_logprob: Option<f32>,
    #[serde(default)]
    no_speech_prob: Option<f32>,
    #[serde(default)]
    words: Vec<WhisperResponseWord>,
}

#[derive(Debug, Deserialize)]
struct WhisperResponseWord {
    #[serde(alias = "text")]
    #[serde(default)]
    word: String,
    #[serde(default)]
    start: Option<f64>,
    #[serde(default)]
    end: Option<f64>,
    #[serde(default)]
    probability: Option<f32>,
    #[serde(default)]
    confidence: Option<f32>,
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

    async fn whisper_inference(
        &self,
        audio: &[u8],
        encoding: AudioEncoding,
    ) -> anyhow::Result<WhisperResponse> {
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
            .text("response_format", "verbose_json")
            .text("token_timestamps", "true");

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

        Ok(response.json().await?)
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

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut applied = Vec::new();
        let mut config = self.config.write().await;

        if let Some(value) = string_setting(updates, "core.providers.whisper_cpp.endpoint") {
            config.endpoint = value;
            applied.push("core.providers.whisper_cpp.endpoint".to_string());
        }
        if let Some(value) = string_setting(updates, "core.providers.whisper_cpp.language") {
            config.language = value;
            applied.push("core.providers.whisper_cpp.language".to_string());
        }
        if let Some(value) = u64_setting(updates, "core.providers.whisper_cpp.request_timeout_sec")
        {
            config.request_timeout_sec = value;
            applied.push("core.providers.whisper_cpp.request_timeout_sec".to_string());
        }

        if !applied.is_empty() {
            tracing::info!(module = self.name(), applied = ?applied, "runtime settings applied");
        }
        Ok(applied)
    }
}

#[async_trait]
impl SttAdapter for WhisperCppStt {
    async fn transcribe_result(
        &self,
        audio: &[u8],
        encoding: AudioEncoding,
    ) -> anyhow::Result<TranscriptionResult> {
        let whisper_response = self.whisper_inference(audio, encoding).await?;
        Ok(TranscriptionResult {
            text: whisper_response.text.trim().to_string(),
            provider: self.name().to_string(),
            confidence: None,
            language: whisper_response
                .language
                .filter(|value| !value.trim().is_empty()),
            segments: whisper_response
                .segments
                .into_iter()
                .map(whisper_segment_to_transcription_segment)
                .filter(|segment| !segment.text.is_empty())
                .collect(),
            alternatives: Vec::new(),
        })
    }

    async fn stt_capabilities(&self) -> SttCapabilities {
        SttCapabilities {
            provider: self.name().to_string(),
            supports_confidence: false,
            supports_word_confidence: true,
            supports_segment_quality: true,
            supports_segments: true,
            supports_alternatives: false,
            supports_language: true,
            supports_timestamps: true,
        }
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
                        .text("response_format", "verbose_json")
                        .text("token_timestamps", "true");

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
                    .text("response_format", "verbose_json")
                    .text("token_timestamps", "true");

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

fn seconds_to_ms(seconds: f64) -> u64 {
    (seconds.max(0.0) * 1000.0).round() as u64
}

fn whisper_segment_to_transcription_segment(
    segment: WhisperResponseSegment,
) -> TranscriptionSegment {
    TranscriptionSegment {
        text: segment.text.trim().to_string(),
        start_ms: segment.start.map(seconds_to_ms),
        end_ms: segment.end.map(seconds_to_ms),
        confidence: segment.confidence.or(segment.probability),
        avg_logprob: segment.avg_logprob,
        no_speech_prob: segment.no_speech_prob,
        words: segment
            .words
            .into_iter()
            .filter_map(|word| {
                let text = word.word.trim().to_string();
                (!text.is_empty()).then_some(TranscriptionWord {
                    text,
                    start_ms: word.start.map(seconds_to_ms),
                    end_ms: word.end.map(seconds_to_ms),
                    confidence: word.confidence.or(word.probability),
                })
            })
            .collect(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_verbose_json_segment_quality_metadata() {
        let response: WhisperResponse = serde_json::from_str(
            r#"{
              "text": " turn on the living room light",
              "language": "en",
              "segments": [
                {
                  "text": " living room lite",
                  "start": 0.4,
                  "end": 1.1,
                  "avg_logprob": -0.92,
                  "no_speech_prob": 0.03,
                  "words": [
                    {"word": "living", "start": 0.4, "end": 0.7, "probability": 0.91},
                    {"word": "lite", "start": 0.9, "end": 1.1, "probability": 0.31}
                  ]
                }
              ]
            }"#,
        )
        .expect("verbose_json response should parse");

        assert_eq!(response.language.as_deref(), Some("en"));
        let segment = whisper_segment_to_transcription_segment(
            response
                .segments
                .into_iter()
                .next()
                .expect("sample contains one segment"),
        );
        assert_eq!(segment.text, "living room lite");
        assert_eq!(segment.start_ms, Some(400));
        assert_eq!(segment.end_ms, Some(1100));
        assert_eq!(segment.avg_logprob, Some(-0.92));
        assert_eq!(segment.no_speech_prob, Some(0.03));
        assert_eq!(segment.words.len(), 2);
        assert_eq!(segment.words[1].text, "lite");
        assert_eq!(segment.words[1].confidence, Some(0.31));
    }
}
