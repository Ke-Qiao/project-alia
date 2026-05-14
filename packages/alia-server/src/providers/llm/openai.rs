//! OpenAI LLM Adapter
//!
//! Implements LlmAdapter for OpenAI GPT models with streaming support.
//! Supports OpenAI-compatible endpoints (including Azure OpenAI, vLLM, etc).

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::ServerConfig;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{ChatMessage, LlmAdapter};

/// OpenAI chat completion request
#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAiMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl From<&ChatMessage> for OpenAiMessage {
    fn from(msg: &ChatMessage) -> Self {
        Self {
            role: msg.role.clone(),
            content: msg.content.clone(),
            name: msg.name.clone(),
        }
    }
}

/// OpenAI chat completion response (non-streaming)
#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: OpenAiMessage,
}

/// OpenAI streaming response chunk
#[derive(Debug, Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: Delta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    content: Option<String>,
}

/// OpenAI LLM adapter
pub struct OpenAiLlm {
    client: Client,
    config: Arc<RwLock<OpenAiConfig>>,
}

#[derive(Debug, Clone)]
struct OpenAiConfig {
    api_key: String,
    base_url: String,
    model: String,
    temperature: f32,
}

impl OpenAiLlm {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            config: Arc::new(RwLock::new(OpenAiConfig {
                api_key: String::new(),
                base_url: "https://api.openai.com/v1".to_string(),
                model: "gpt-4o".to_string(),
                temperature: 0.7,
            })),
        }
    }

    async fn chat_internal(
        &self,
        messages: &[ChatMessage],
        stream: bool,
    ) -> anyhow::Result<reqwest::Response> {
        let config = self.config.read().await;

        let request = ChatRequest {
            model: config.model.clone(),
            messages: messages.iter().map(OpenAiMessage::from).collect(),
            temperature: Some(config.temperature),
            max_tokens: None,
            stream,
        };

        let response = self
            .client
            .post(format!("{}/chat/completions", config.base_url))
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API error {}: {}", status, body);
        }

        Ok(response)
    }
}

impl Default for OpenAiLlm {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for OpenAiLlm {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let server_config: &ServerConfig = &ctx.config;

        let mut config = self.config.write().await;
        config.api_key = server_config.llm.openai_api_key.clone();
        config.base_url = server_config.llm.openai_base_url.clone();
        config.model = server_config.llm.openai_model.clone();

        if config.api_key.is_empty() {
            tracing::warn!("OpenAI API key not configured");
        }

        tracing::info!(model = %config.model, base_url = %config.base_url, "OpenAI LLM initialized");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let config = self.config.read().await;
        if config.api_key.is_empty() {
            ModuleHealth::Unhealthy("API key not configured".to_string())
        } else {
            ModuleHealth::Healthy
        }
    }
}

#[async_trait]
impl LlmAdapter for OpenAiLlm {
    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String> {
        let response = self.chat_internal(messages, false).await?;
        let chat_response: ChatResponse = response.json().await?;

        chat_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| anyhow::anyhow!("No response from OpenAI"))
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
        let response = self.chat_internal(messages, true).await?;

        let stream = async_stream::stream! {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        buffer.push_str(&String::from_utf8_lossy(&chunk));

                        // Process complete SSE lines
                        while let Some(line_end) = buffer.find('\n') {
                            let line = buffer[..line_end].trim().to_string();
                            buffer = buffer[line_end + 1..].to_string();

                            if line.starts_with("data: ") {
                                let data = &line[6..];
                                if data == "[DONE]" {
                                    break;
                                }

                                if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                                    for choice in chunk.choices {
                                        if let Some(content) = choice.delta.content {
                                            yield Ok(content);
                                        }
                                    }
                                }
                            }
                        }
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
