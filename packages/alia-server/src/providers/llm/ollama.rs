//! Ollama LLM Adapter
//!
//! Implements LlmAdapter for local Ollama models with streaming support.
//! Supports any model available in local Ollama instance.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::ServerConfig;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    ChatMessage, LlmAdapter, LlmResponse, LlmToolCall, ToolDefinition,
};

/// Ollama chat request
#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_ctx: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<OllamaToolCall>,
}

impl From<&ChatMessage> for OllamaMessage {
    fn from(msg: &ChatMessage) -> Self {
        let content = if let Some(ref name) = msg.name {
            format!("{}: {}", name, msg.content)
        } else {
            msg.content.clone()
        };

        Self {
            role: msg.role.clone(),
            content,
            tool_calls: msg.tool_calls.iter().map(OllamaToolCall::from).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct OllamaTool {
    r#type: &'static str,
    function: OllamaFunctionDefinition,
}

#[derive(Debug, Serialize)]
struct OllamaFunctionDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl From<&ToolDefinition> for OllamaTool {
    fn from(tool: &ToolDefinition) -> Self {
        Self {
            r#type: "function",
            function: OllamaFunctionDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OllamaToolCall {
    #[serde(default)]
    id: String,
    function: OllamaToolFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OllamaToolFunctionCall {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

impl From<&LlmToolCall> for OllamaToolCall {
    fn from(call: &LlmToolCall) -> Self {
        Self {
            id: call.id.clone(),
            function: OllamaToolFunctionCall {
                name: call.tool_name.clone(),
                arguments: call.arguments.clone(),
            },
        }
    }
}

impl From<&OllamaToolCall> for LlmToolCall {
    fn from(call: &OllamaToolCall) -> Self {
        let arguments = match &call.function.arguments {
            serde_json::Value::String(raw) => {
                serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.clone()))
            }
            value => value.clone(),
        };

        Self {
            id: call.id.clone(),
            tool_name: call.function.name.clone(),
            arguments,
        }
    }
}

/// Ollama chat response (non-streaming)
#[derive(Debug, Deserialize)]
struct OllamaResponse {
    message: OllamaMessage,
    done: bool,
}

/// Ollama streaming response chunk
#[derive(Debug, Deserialize)]
struct OllamaStreamChunk {
    message: Option<OllamaMessage>,
    done: bool,
}

/// Ollama LLM adapter
pub struct OllamaLlm {
    client: Client,
    config: Arc<RwLock<OllamaConfig>>,
}

#[derive(Debug, Clone)]
struct OllamaConfig {
    endpoint: String,
    model: String,
    temperature: f32,
}

impl OllamaLlm {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            config: Arc::new(RwLock::new(OllamaConfig {
                endpoint: "http://localhost:11434".to_string(),
                model: "qwen2.5:7b".to_string(),
                temperature: 0.7,
            })),
        }
    }

    async fn chat_internal(
        &self,
        messages: &[ChatMessage],
        stream: bool,
        tools: Option<&[ToolDefinition]>,
    ) -> anyhow::Result<reqwest::Response> {
        let config = self.config.read().await;
        let tools = tools
            .filter(|tools| !tools.is_empty())
            .map(|tools| tools.iter().map(OllamaTool::from).collect::<Vec<_>>());

        let request = OllamaRequest {
            model: config.model.clone(),
            messages: messages.iter().map(OllamaMessage::from).collect(),
            stream,
            tools,
            options: Some(OllamaOptions {
                temperature: Some(config.temperature),
                num_ctx: None,
            }),
        };

        let response = self
            .client
            .post(format!("{}/api/chat", config.endpoint))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Ollama API error {}: {}", status, body);
        }

        Ok(response)
    }
}

impl Default for OllamaLlm {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for OllamaLlm {
    fn name(&self) -> &'static str {
        "ollama"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let server_config: &ServerConfig = &ctx.config;

        let mut config = self.config.write().await;
        config.endpoint = server_config.llm.ollama_endpoint.clone();
        config.model = server_config.llm.ollama_model.clone();

        tracing::info!(model = %config.model, endpoint = %config.endpoint, "Ollama LLM initialized");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let config = self.config.read().await;

        // Try to connect to Ollama
        match self
            .client
            .get(format!("{}/api/version", config.endpoint))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => ModuleHealth::Healthy,
            Ok(resp) => {
                ModuleHealth::Unhealthy(format!("Ollama returned status {}", resp.status()))
            }
            Err(e) => ModuleHealth::Unhealthy(format!("Cannot connect to Ollama: {}", e)),
        }
    }
}

#[async_trait]
impl LlmAdapter for OllamaLlm {
    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String> {
        let response = self.chat_internal(messages, false, None).await?;
        let ollama_response: OllamaResponse = response.json().await?;
        Ok(ollama_response.message.content)
    }

    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let response = self.chat_internal(messages, false, Some(tools)).await?;
        let ollama_response: OllamaResponse = response.json().await?;
        Ok(LlmResponse {
            content: ollama_response.message.content,
            tool_calls: ollama_response
                .message
                .tool_calls
                .iter()
                .map(LlmToolCall::from)
                .collect(),
        })
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
        let response = self.chat_internal(messages, true, None).await?;

        let stream = async_stream::stream! {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        buffer.push_str(&String::from_utf8_lossy(&chunk));

                        // Process complete JSON lines (Ollama uses NDJSON)
                        while let Some(line_end) = buffer.find('\n') {
                            let line = buffer[..line_end].trim().to_string();
                            buffer = buffer[line_end + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }

                            if let Ok(chunk) = serde_json::from_str::<OllamaStreamChunk>(&line) {
                                if let Some(msg) = chunk.message {
                                    if !msg.content.is_empty() {
                                        yield Ok(msg.content);
                                    }
                                }
                                if chunk.done {
                                    break;
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
