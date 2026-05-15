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
use crate::providers::adapter::{
    ChatMessage, LlmAdapter, LlmResponse, LlmToolCall, ToolDefinition,
};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAiMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OpenAiToolCall>,
}

impl From<&ChatMessage> for OpenAiMessage {
    fn from(msg: &ChatMessage) -> Self {
        Self {
            role: msg.role.clone(),
            content: (!msg.content.is_empty()).then(|| msg.content.clone()),
            name: (msg.role != "tool").then(|| msg.name.clone()).flatten(),
            tool_call_id: msg.tool_call_id.clone(),
            tool_calls: msg.tool_calls.iter().map(OpenAiToolCall::from).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct OpenAiTool {
    r#type: &'static str,
    function: OpenAiFunctionDefinition,
}

#[derive(Debug, Serialize)]
struct OpenAiFunctionDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl From<&ToolDefinition> for OpenAiTool {
    fn from(tool: &ToolDefinition) -> Self {
        Self {
            r#type: "function",
            function: OpenAiFunctionDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiToolCall {
    pub id: String,
    pub r#type: String,
    pub function: OpenAiToolFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiToolFunctionCall {
    pub name: String,
    pub arguments: String,
}

impl From<&LlmToolCall> for OpenAiToolCall {
    fn from(call: &LlmToolCall) -> Self {
        Self {
            id: call.id.clone(),
            r#type: "function".to_string(),
            function: OpenAiToolFunctionCall {
                name: call.tool_name.clone(),
                arguments: call.arguments.to_string(),
            },
        }
    }
}

impl TryFrom<&OpenAiToolCall> for LlmToolCall {
    type Error = anyhow::Error;

    fn try_from(call: &OpenAiToolCall) -> Result<Self, Self::Error> {
        let raw_arguments = if call.function.arguments.trim().is_empty() {
            "{}"
        } else {
            &call.function.arguments
        };
        let arguments = serde_json::from_str(raw_arguments)
            .map_err(|error| anyhow::anyhow!("invalid OpenAI tool call arguments: {error}"))?;
        Ok(Self {
            id: call.id.clone(),
            tool_name: call.function.name.clone(),
            arguments,
        })
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
        tools: Option<&[ToolDefinition]>,
    ) -> anyhow::Result<reqwest::Response> {
        let config = self.config.read().await;
        let tools = tools
            .filter(|tools| !tools.is_empty())
            .map(|tools| tools.iter().map(OpenAiTool::from).collect::<Vec<_>>());

        let request = ChatRequest {
            model: config.model.clone(),
            messages: messages.iter().map(OpenAiMessage::from).collect(),
            temperature: Some(config.temperature),
            max_tokens: None,
            stream,
            tool_choice: tools.as_ref().map(|_| "auto"),
            tools,
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
        let response = self.chat_internal(messages, false, None).await?;
        let chat_response: ChatResponse = response.json().await?;

        chat_response
            .choices
            .first()
            .map(|c| c.message.content.clone().unwrap_or_default())
            .ok_or_else(|| anyhow::anyhow!("No response from OpenAI"))
    }

    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let response = self.chat_internal(messages, false, Some(tools)).await?;
        let chat_response: ChatResponse = response.json().await?;
        let message = chat_response
            .choices
            .first()
            .map(|c| &c.message)
            .ok_or_else(|| anyhow::anyhow!("No response from OpenAI"))?;

        let mut tool_calls = Vec::with_capacity(message.tool_calls.len());
        for call in &message.tool_calls {
            tool_calls.push(LlmToolCall::try_from(call)?);
        }

        Ok(LlmResponse {
            content: message.content.clone().unwrap_or_default(),
            tool_calls,
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
