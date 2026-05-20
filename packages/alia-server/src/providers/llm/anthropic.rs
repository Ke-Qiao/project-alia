//! Anthropic Messages API adapter.

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::config::{LlmRequestOptionsSection, ServerConfig};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    ChatMessage, LlmAdapter, LlmCapabilities, LlmResponse, LlmToolCall, ToolDefinition,
};
use crate::providers::llm::errors::{classify_http_error, classify_transport_error};
use crate::providers::llm::streaming::SseDecoder;
use crate::settings::values::{f32_setting, string_setting, u64_setting};

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum AnthropicContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
}

impl From<&ToolDefinition> for AnthropicTool {
    fn from(tool: &ToolDefinition) -> Self {
        Self {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.parameters.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

pub struct AnthropicLlm {
    client: Client,
    config: Arc<RwLock<AnthropicConfig>>,
}

#[derive(Debug, Clone)]
struct AnthropicConfig {
    api_key: String,
    base_url: String,
    model: String,
    version: String,
    request: LlmRequestOptionsSection,
    health_probe_enabled: bool,
}

impl AnthropicLlm {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            config: Arc::new(RwLock::new(AnthropicConfig {
                api_key: String::new(),
                base_url: "https://api.anthropic.com/v1".to_string(),
                model: "claude-sonnet-4-20250514".to_string(),
                version: "2023-06-01".to_string(),
                request: LlmRequestOptionsSection::default(),
                health_probe_enabled: false,
            })),
        }
    }

    async fn chat_internal(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolDefinition]>,
    ) -> anyhow::Result<AnthropicResponse> {
        let config = self.config.read().await;
        let tools = tools
            .filter(|tools| !tools.is_empty())
            .map(|tools| tools.iter().map(AnthropicTool::from).collect::<Vec<_>>());
        let (system, messages) = anthropic_messages(messages);
        let request = AnthropicRequest {
            model: config.model.clone(),
            max_tokens: config.request.max_output_tokens,
            messages,
            system,
            temperature: Some(config.request.temperature),
            top_p: config.request.top_p,
            stop_sequences: (!config.request.stop_sequences.is_empty())
                .then(|| config.request.stop_sequences.clone()),
            tools,
            tool_choice: anthropic_tool_choice(config.request.tool_choice.as_deref()),
            stream: None,
        };

        let response = self
            .client
            .post(format!(
                "{}/messages",
                config.base_url.trim_end_matches('/')
            ))
            .header("x-api-key", config.api_key.trim())
            .header("anthropic-version", config.version.trim())
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|error| classify_transport_error("anthropic", error))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(classify_http_error("anthropic", status, body));
        }

        Ok(response.json().await?)
    }

    async fn stream_internal(&self, messages: &[ChatMessage]) -> anyhow::Result<reqwest::Response> {
        self.stream_internal_with_tools(messages, None).await
    }

    async fn stream_internal_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolDefinition]>,
    ) -> anyhow::Result<reqwest::Response> {
        let config = self.config.read().await;
        let tools = tools
            .filter(|tools| !tools.is_empty())
            .map(|tools| tools.iter().map(AnthropicTool::from).collect::<Vec<_>>());
        let (system, messages) = anthropic_messages(messages);
        let request = AnthropicRequest {
            model: config.model.clone(),
            max_tokens: config.request.max_output_tokens,
            messages,
            system,
            temperature: Some(config.request.temperature),
            top_p: config.request.top_p,
            stop_sequences: (!config.request.stop_sequences.is_empty())
                .then(|| config.request.stop_sequences.clone()),
            tools,
            tool_choice: anthropic_tool_choice(config.request.tool_choice.as_deref()),
            stream: Some(true),
        };

        let response = self
            .client
            .post(format!(
                "{}/messages",
                config.base_url.trim_end_matches('/')
            ))
            .header("x-api-key", config.api_key.trim())
            .header("anthropic-version", config.version.trim())
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&request)
            .send()
            .await
            .map_err(|error| classify_transport_error("anthropic", error))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(classify_http_error("anthropic", status, body));
        }

        Ok(response)
    }
}

impl Default for AnthropicLlm {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for AnthropicLlm {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let server_config: &ServerConfig = &ctx.config;
        let mut config = self.config.write().await;
        config.api_key = server_config.providers.anthropic.api_key.clone();
        config.base_url = server_config.providers.anthropic.base_url.clone();
        config.model = server_config.providers.anthropic.chat_model.clone();
        config.version = server_config.providers.anthropic.version.clone();
        config.request = server_config.providers.anthropic.request.clone();
        config.health_probe_enabled = server_config.providers.anthropic.health_probe_enabled;
        if config.api_key.is_empty() {
            tracing::warn!("Anthropic API key not configured");
        }
        tracing::info!(model = %config.model, base_url = %config.base_url, "Anthropic LLM initialized");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let config = self.config.read().await;
        if config.api_key.trim().is_empty() {
            ModuleHealth::Unhealthy("API key not configured".to_string())
        } else if !config.health_probe_enabled {
            ModuleHealth::Degraded("API key configured; live provider probe disabled".to_string())
        } else {
            ModuleHealth::Degraded(
                "Anthropic live probe is disabled to avoid startup model calls".to_string(),
            )
        }
    }

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut applied = Vec::new();
        let mut config = self.config.write().await;

        if let Some(value) = string_setting(updates, "core.providers.anthropic.api_key") {
            config.api_key = value;
            applied.push("core.providers.anthropic.api_key".to_string());
        }
        if let Some(value) = string_setting(updates, "core.providers.anthropic.base_url") {
            config.base_url = value;
            applied.push("core.providers.anthropic.base_url".to_string());
        }
        if let Some(value) = string_setting(updates, "core.providers.anthropic.chat_model") {
            config.model = value;
            applied.push("core.providers.anthropic.chat_model".to_string());
        }
        if let Some(value) = string_setting(updates, "core.providers.anthropic.version") {
            config.version = value;
            applied.push("core.providers.anthropic.version".to_string());
        }
        if let Some(value) = u64_setting(updates, "core.providers.anthropic.max_output_tokens") {
            config.request.max_output_tokens = value.min(u32::MAX as u64) as u32;
            applied.push("core.providers.anthropic.max_output_tokens".to_string());
        }
        if let Some(value) = f32_setting(updates, "core.providers.anthropic.temperature") {
            config.request.temperature = value;
            applied.push("core.providers.anthropic.temperature".to_string());
        }
        if let Some(value) = f32_setting(updates, "core.providers.anthropic.top_p") {
            config.request.top_p = Some(value);
            applied.push("core.providers.anthropic.top_p".to_string());
        }

        if !applied.is_empty() {
            tracing::info!(module = self.name(), applied = ?applied, "runtime settings applied");
        }
        Ok(applied)
    }
}

#[async_trait]
impl LlmAdapter for AnthropicLlm {
    async fn llm_capabilities(&self) -> LlmCapabilities {
        let config = self.config.read().await;
        let mut capabilities = LlmCapabilities::minimal(self.name());
        capabilities.supports_streaming = true;
        capabilities.supports_tool_calling = true;
        capabilities.supports_streaming_tool_calls = true;
        capabilities.supports_system_messages = true;
        capabilities.supports_parallel_tool_calls = true;
        capabilities.max_output_tokens = Some(config.request.max_output_tokens);
        capabilities
            .provider_specific
            .insert("version".to_string(), serde_json::json!(config.version));
        capabilities
            .provider_specific
            .insert("model".to_string(), serde_json::json!(config.model));
        capabilities
    }

    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String> {
        let response = self.chat_internal(messages, None).await?;
        Ok(anthropic_text(&response.content))
    }

    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let response = self.chat_internal(messages, Some(tools)).await?;
        Ok(anthropic_response(response.content))
    }

    async fn chat_stream_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let response = self
            .stream_internal_with_tools(messages, Some(tools))
            .await?;
        let mut byte_stream = response.bytes_stream();
        let mut decoder = SseDecoder::default();
        let mut aggregate = AnthropicStreamAggregate::default();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = chunk_result?;
            for event in decoder.push(&chunk) {
                aggregate.apply_data(&event.data)?;
            }
        }

        Ok(aggregate.into_response())
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
        let response = self.stream_internal(messages).await?;
        let stream = async_stream::stream! {
            let mut byte_stream = response.bytes_stream();
            let mut decoder = SseDecoder::default();

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        for event in decoder.push(&chunk) {
                            match anthropic_stream_text(&event.data) {
                                Ok(Some(text)) if !text.is_empty() => yield Ok(text),
                                Ok(_) => {}
                                Err(error) => yield Err(error),
                            }
                        }
                    }
                    Err(error) => {
                        yield Err(anyhow::anyhow!("Anthropic stream error: {error}"));
                        break;
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

fn anthropic_messages(messages: &[ChatMessage]) -> (Option<String>, Vec<AnthropicMessage>) {
    let mut system = Vec::new();
    let mut converted = Vec::new();

    for message in messages {
        match message.role.as_str() {
            "system" => system.push(message.content.clone()),
            "tool" => converted.push(AnthropicMessage {
                role: "user".to_string(),
                content: vec![AnthropicContent::ToolResult {
                    tool_use_id: message.tool_call_id.clone().unwrap_or_default(),
                    content: message.content.clone(),
                }],
            }),
            "assistant" => {
                let mut content = Vec::new();
                if !message.content.trim().is_empty() {
                    content.push(AnthropicContent::Text {
                        text: message.content.clone(),
                    });
                }
                content.extend(
                    message
                        .tool_calls
                        .iter()
                        .map(|call| AnthropicContent::ToolUse {
                            id: call.id.clone(),
                            name: call.tool_name.clone(),
                            input: call.arguments.clone(),
                        }),
                );
                if !content.is_empty() {
                    converted.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content,
                    });
                }
            }
            _ => converted.push(AnthropicMessage {
                role: "user".to_string(),
                content: vec![AnthropicContent::Text {
                    text: message.content.clone(),
                }],
            }),
        }
    }

    let system = (!system.is_empty()).then(|| system.join("\n\n"));
    (system, converted)
}

fn anthropic_response(content: Vec<AnthropicContent>) -> LlmResponse {
    let mut response = LlmResponse::default();
    for block in content {
        match block {
            AnthropicContent::Text { text } => {
                if !response.content.is_empty() && !text.is_empty() {
                    response.content.push('\n');
                }
                response.content.push_str(&text);
            }
            AnthropicContent::ToolUse { id, name, input } => {
                response.tool_calls.push(LlmToolCall {
                    id,
                    tool_name: name,
                    arguments: input,
                });
            }
            AnthropicContent::ToolResult { .. } => {}
        }
    }
    response
}

fn anthropic_tool_choice(configured: Option<&str>) -> Option<Value> {
    configured.map(|choice| match choice {
        "auto" => serde_json::json!({ "type": "auto" }),
        "required" | "any" => serde_json::json!({ "type": "any" }),
        "none" => serde_json::json!({ "type": "none" }),
        name => serde_json::json!({ "type": "tool", "name": name }),
    })
}

fn anthropic_text(content: &[AnthropicContent]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            AnthropicContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn anthropic_stream_text(data: &str) -> anyhow::Result<Option<String>> {
    Ok(anthropic_stream_delta(data)?.text)
}

#[derive(Default)]
struct AnthropicStreamAggregate {
    content: String,
    tool_calls: HashMap<u64, AnthropicToolCallBuilder>,
}

#[derive(Default)]
struct AnthropicToolCallBuilder {
    id: String,
    name: String,
    partial_json: String,
}

impl AnthropicStreamAggregate {
    fn apply_data(&mut self, data: &str) -> anyhow::Result<()> {
        let delta = anthropic_stream_delta(data)?;
        if let Some(text) = delta.text {
            self.content.push_str(&text);
        }
        if let Some(tool_start) = delta.tool_start {
            let builder = self.tool_calls.entry(tool_start.index).or_default();
            builder.id = tool_start.id;
            builder.name = tool_start.name;
        }
        if let Some(tool_delta) = delta.tool_input_delta {
            self.tool_calls
                .entry(tool_delta.index)
                .or_default()
                .partial_json
                .push_str(&tool_delta.partial_json);
        }
        Ok(())
    }

    fn into_response(self) -> LlmResponse {
        let mut indexed = self.tool_calls.into_iter().collect::<Vec<_>>();
        indexed.sort_by_key(|(index, _)| *index);
        let tool_calls = indexed
            .into_iter()
            .filter(|(_, call)| !call.name.trim().is_empty())
            .map(|(index, call)| {
                let arguments = serde_json::from_str(if call.partial_json.trim().is_empty() {
                    "{}"
                } else {
                    &call.partial_json
                })
                .unwrap_or_else(|_| serde_json::json!({}));
                LlmToolCall {
                    id: if call.id.trim().is_empty() {
                        format!("anthropic_call_{index}")
                    } else {
                        call.id
                    },
                    tool_name: call.name,
                    arguments,
                }
            })
            .collect();
        LlmResponse {
            content: self.content,
            tool_calls,
        }
    }
}

#[derive(Default)]
struct AnthropicStreamDelta {
    text: Option<String>,
    tool_start: Option<AnthropicToolStart>,
    tool_input_delta: Option<AnthropicToolInputDelta>,
}

struct AnthropicToolStart {
    index: u64,
    id: String,
    name: String,
}

struct AnthropicToolInputDelta {
    index: u64,
    partial_json: String,
}

fn anthropic_stream_delta(data: &str) -> anyhow::Result<AnthropicStreamDelta> {
    if data.trim() == "[DONE]" {
        return Ok(AnthropicStreamDelta::default());
    }
    let value: Value = serde_json::from_str(data)?;
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match event_type {
        "content_block_start" => {
            let content_block = value.get("content_block").unwrap_or(&Value::Null);
            if content_block.get("type").and_then(Value::as_str) == Some("tool_use") {
                Ok(AnthropicStreamDelta {
                    tool_start: Some(AnthropicToolStart {
                        index: value.get("index").and_then(Value::as_u64).unwrap_or(0),
                        id: content_block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        name: content_block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }),
                    ..AnthropicStreamDelta::default()
                })
            } else {
                Ok(AnthropicStreamDelta::default())
            }
        }
        "content_block_delta" => {
            let delta = value.get("delta").unwrap_or(&Value::Null);
            if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
                Ok(AnthropicStreamDelta {
                    text: delta
                        .get("text")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    ..AnthropicStreamDelta::default()
                })
            } else if delta.get("type").and_then(Value::as_str) == Some("input_json_delta") {
                Ok(AnthropicStreamDelta {
                    tool_input_delta: Some(AnthropicToolInputDelta {
                        index: value.get("index").and_then(Value::as_u64).unwrap_or(0),
                        partial_json: delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }),
                    ..AnthropicStreamDelta::default()
                })
            } else {
                Ok(AnthropicStreamDelta::default())
            }
        }
        "error" => {
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown Anthropic stream error");
            anyhow::bail!("Anthropic stream error: {message}");
        }
        _ => Ok(AnthropicStreamDelta::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_stream_text_extracts_text_delta() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        assert_eq!(
            anthropic_stream_text(data).unwrap().as_deref(),
            Some("Hello")
        );
    }

    #[test]
    fn anthropic_stream_text_ignores_non_text_events() {
        let data = r#"{"type":"message_start","message":{"content":[]}}"#;
        assert!(anthropic_stream_text(data).unwrap().is_none());
    }

    #[test]
    fn anthropic_stream_aggregate_collects_tool_call_arguments() {
        let mut aggregate = AnthropicStreamAggregate::default();
        aggregate
            .apply_data(
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"search","input":{}}}"#,
            )
            .unwrap();
        aggregate
            .apply_data(
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"q\""}}"#,
            )
            .unwrap();
        aggregate
            .apply_data(
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":":\"moon\"}"}}"#,
            )
            .unwrap();
        let response = aggregate.into_response();
        assert_eq!(response.tool_calls[0].id, "toolu_1");
        assert_eq!(response.tool_calls[0].tool_name, "search");
        assert_eq!(
            response.tool_calls[0].arguments,
            serde_json::json!({"q": "moon"})
        );
    }
}
