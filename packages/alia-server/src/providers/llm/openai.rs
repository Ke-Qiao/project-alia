//! OpenAI LLM Adapter
//!
//! Implements LlmAdapter for OpenAI GPT models with streaming support.
//! Supports OpenAI-compatible endpoints (including Azure OpenAI, vLLM, etc).

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::config::{LlmProfileKind, LlmRequestOptionsSection, OpenAiApiFamily, ServerConfig};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    ChatMessage, LlmAdapter, LlmCapabilities, LlmResponse, LlmToolCall, ToolDefinition,
};
use crate::providers::llm::errors::{classify_http_error, classify_transport_error};
use crate::providers::llm::streaming::SseDecoder;
use crate::settings::values::{f32_setting, string_setting, u64_setting};

/// OpenAI chat completion request
#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<Value>,
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
    #[serde(default)]
    tool_calls: Vec<OpenAiStreamToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<OpenAiStreamToolFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<Value>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct ResponsesTool {
    r#type: &'static str,
    name: String,
    description: String,
    parameters: Value,
}

impl From<&ToolDefinition> for ResponsesTool {
    fn from(tool: &ToolDefinition) -> Self {
        Self {
            r#type: "function",
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        }
    }
}

/// OpenAI LLM adapter
pub struct OpenAiLlm {
    client: Client,
    profile_id: String,
    config: Arc<RwLock<OpenAiConfig>>,
}

#[derive(Debug, Clone)]
struct OpenAiConfig {
    api_key: String,
    base_url: String,
    model: String,
    api_family: OpenAiApiFamily,
    request: LlmRequestOptionsSection,
    health_probe_enabled: bool,
}

impl OpenAiLlm {
    pub fn new() -> Self {
        Self::for_profile("major")
    }

    pub fn for_profile(profile_id: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            profile_id: profile_id.into(),
            config: Arc::new(RwLock::new(OpenAiConfig {
                api_key: String::new(),
                base_url: "https://api.openai.com/v1".to_string(),
                model: "gpt-4o".to_string(),
                api_family: OpenAiApiFamily::ChatCompletions,
                request: LlmRequestOptionsSection::default(),
                health_probe_enabled: false,
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
            temperature: Some(config.request.temperature),
            max_tokens: Some(config.request.max_output_tokens),
            top_p: config.request.top_p,
            presence_penalty: config.request.presence_penalty,
            frequency_penalty: config.request.frequency_penalty,
            stop: (!config.request.stop_sequences.is_empty())
                .then(|| config.request.stop_sequences.clone()),
            stream,
            tool_choice: openai_chat_tool_choice(
                tools.as_ref(),
                config.request.tool_choice.as_deref(),
            ),
            tools,
            response_format: openai_chat_response_format(&config.request),
        };

        let response = self
            .client
            .post(format!(
                "{}/chat/completions",
                config.base_url.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|error| classify_transport_error("openai", error))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(classify_http_error("openai", status, body));
        }

        Ok(response)
    }

    async fn responses_internal(
        &self,
        messages: &[ChatMessage],
        stream: bool,
        tools: Option<&[ToolDefinition]>,
    ) -> anyhow::Result<reqwest::Response> {
        let config = self.config.read().await;
        let tools = tools
            .filter(|tools| !tools.is_empty())
            .map(|tools| tools.iter().map(ResponsesTool::from).collect::<Vec<_>>());
        let (instructions, input) = responses_input(messages);
        let request = ResponsesRequest {
            model: config.model.clone(),
            input,
            instructions,
            max_output_tokens: Some(config.request.max_output_tokens),
            temperature: Some(config.request.temperature),
            top_p: config.request.top_p,
            tools,
            tool_choice: openai_responses_tool_choice(config.request.tool_choice.as_deref()),
            text: openai_responses_text_format(&config.request),
            stream,
        };

        let response = self
            .client
            .post(format!(
                "{}/responses",
                config.base_url.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|error| classify_transport_error("openai", error))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(classify_http_error("openai", status, body));
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
        let profile = server_config.profiles.llm_profile(&self.profile_id)?;
        if profile.provider != "openai" || profile.kind != LlmProfileKind::Chat {
            anyhow::bail!(
                "LLM profile '{}' is not an OpenAI chat profile",
                self.profile_id
            );
        }

        let mut config = self.config.write().await;
        config.api_key = server_config.providers.openai.api_key.clone();
        config.base_url = server_config.providers.openai.base_url.clone();
        config.model = profile.model.clone();
        config.api_family = server_config.providers.openai.api_family;
        config.request = profile.request.clone();
        config.health_probe_enabled = server_config.providers.openai.health_probe_enabled;

        if config.api_key.is_empty() {
            tracing::warn!("OpenAI API key not configured");
        }

        tracing::info!(
            profile = %self.profile_id,
            model = %config.model,
            base_url = %config.base_url,
            "OpenAI LLM initialized"
        );
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let config = self.config.read().await;
        if config.api_key.trim().is_empty() {
            ModuleHealth::Unhealthy("API key not configured".to_string())
        } else if !config.health_probe_enabled {
            ModuleHealth::Degraded("API key configured; live provider probe disabled".to_string())
        } else {
            match self
                .client
                .get(format!(
                    "{}/models/{}",
                    config.base_url.trim_end_matches('/'),
                    config.model
                ))
                .header("Authorization", format!("Bearer {}", config.api_key.trim()))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => ModuleHealth::Healthy,
                Ok(response) => ModuleHealth::Degraded(format!(
                    "OpenAI model probe returned {}",
                    response.status()
                )),
                Err(error) => ModuleHealth::Degraded(format!("OpenAI probe failed: {error}")),
            }
        }
    }

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut applied = Vec::new();
        let mut config = self.config.write().await;

        if let Some(value) = string_setting(updates, "core.providers.openai.api_key") {
            config.api_key = value;
            applied.push("core.providers.openai.api_key".to_string());
        }
        if let Some(value) = string_setting(updates, "core.providers.openai.base_url") {
            config.base_url = value;
            applied.push("core.providers.openai.base_url".to_string());
        }
        let model_key = format!("core.profiles.llm.{}.model", self.profile_id);
        if let Some(value) = string_setting(updates, &model_key) {
            config.model = value;
            applied.push(model_key);
        }
        let temperature_key = format!("core.profiles.llm.{}.temperature", self.profile_id);
        if let Some(value) = f32_setting(updates, &temperature_key) {
            config.request.temperature = value;
            applied.push(temperature_key);
        }
        let max_tokens_key = format!("core.profiles.llm.{}.max_output_tokens", self.profile_id);
        if let Some(value) = u64_setting(updates, &max_tokens_key) {
            config.request.max_output_tokens = value.min(u32::MAX as u64) as u32;
            applied.push(max_tokens_key);
        }
        let top_p_key = format!("core.profiles.llm.{}.top_p", self.profile_id);
        if let Some(value) = f32_setting(updates, &top_p_key) {
            config.request.top_p = Some(value);
            applied.push(top_p_key);
        }
        if let Some(value) = string_setting(updates, "core.providers.openai.api_family") {
            config.api_family = match value.as_str() {
                "responses" => OpenAiApiFamily::Responses,
                _ => OpenAiApiFamily::ChatCompletions,
            };
            applied.push("core.providers.openai.api_family".to_string());
        }

        if !applied.is_empty() {
            tracing::info!(module = self.name(), applied = ?applied, "runtime settings applied");
        }
        Ok(applied)
    }
}

#[async_trait]
impl LlmAdapter for OpenAiLlm {
    async fn llm_capabilities(&self) -> LlmCapabilities {
        let config = self.config.read().await;
        let mut capabilities = LlmCapabilities::minimal(self.name());
        capabilities.supports_streaming = true;
        capabilities.supports_tool_calling = true;
        capabilities.supports_streaming_tool_calls =
            config.api_family == OpenAiApiFamily::ChatCompletions;
        capabilities.supports_system_messages = true;
        capabilities.supports_json_mode = true;
        capabilities.supports_vision = true;
        capabilities.supports_parallel_tool_calls = true;
        capabilities.max_output_tokens = Some(config.request.max_output_tokens);
        capabilities.provider_specific.insert(
            "api_family".to_string(),
            serde_json::json!(match config.api_family {
                OpenAiApiFamily::ChatCompletions => "chat_completions",
                OpenAiApiFamily::Responses => "responses",
            }),
        );
        capabilities
            .provider_specific
            .insert("model".to_string(), serde_json::json!(config.model));
        capabilities
    }

    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String> {
        if self.config.read().await.api_family == OpenAiApiFamily::Responses {
            let response = self.responses_internal(messages, false, None).await?;
            let value: Value = response.json().await?;
            return Ok(parse_responses_output(&value).content);
        }

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
        if self.config.read().await.api_family == OpenAiApiFamily::Responses {
            let response = self
                .responses_internal(messages, false, Some(tools))
                .await?;
            let value: Value = response.json().await?;
            return Ok(parse_responses_output(&value));
        }

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

    async fn chat_stream_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if self.config.read().await.api_family == OpenAiApiFamily::Responses {
            return self.chat_with_tools(messages, tools).await;
        }
        let response = self.chat_internal(messages, true, Some(tools)).await?;
        let mut byte_stream = response.bytes_stream();
        let mut decoder = SseDecoder::default();
        let mut aggregate = OpenAiStreamAggregate::default();

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
        let family = self.config.read().await.api_family;
        let response = if family == OpenAiApiFamily::Responses {
            self.responses_internal(messages, true, None).await?
        } else {
            self.chat_internal(messages, true, None).await?
        };

        let stream = async_stream::stream! {
            let mut byte_stream = response.bytes_stream();
            let mut decoder = SseDecoder::default();

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        for event in decoder.push(&chunk) {
                            let parsed = if family == OpenAiApiFamily::Responses {
                                openai_responses_stream_text(&event.data)
                            } else {
                                openai_stream_text(&event.data)
                            };
                            match parsed {
                                Ok(pieces) => {
                                    for piece in pieces {
                                        if !piece.is_empty() {
                                            yield Ok(piece);
                                        }
                                    }
                                }
                                Err(error) => yield Err(error),
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

#[derive(Default)]
struct OpenAiStreamAggregate {
    content: String,
    tool_calls: HashMap<usize, OpenAiToolCallBuilder>,
}

#[derive(Default)]
struct OpenAiToolCallBuilder {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl OpenAiStreamAggregate {
    fn apply_data(&mut self, data: &str) -> anyhow::Result<()> {
        if data.trim() == "[DONE]" {
            return Ok(());
        }
        let chunk: StreamChunk = serde_json::from_str(data)?;
        for choice in chunk.choices {
            if let Some(content) = choice.delta.content {
                self.content.push_str(&content);
            }
            for call in choice.delta.tool_calls {
                let builder = self.tool_calls.entry(call.index).or_default();
                if let Some(id) = call.id {
                    builder.id = Some(id);
                }
                if let Some(function) = call.function {
                    if let Some(name) = function.name {
                        builder.name = Some(name);
                    }
                    if let Some(arguments) = function.arguments {
                        builder.arguments.push_str(&arguments);
                    }
                }
            }
        }
        Ok(())
    }

    fn into_response(self) -> LlmResponse {
        let mut indexed = self.tool_calls.into_iter().collect::<Vec<_>>();
        indexed.sort_by_key(|(index, _)| *index);
        let tool_calls = indexed
            .into_iter()
            .filter_map(|(index, call)| {
                let tool_name = call.name?;
                let arguments = serde_json::from_str(if call.arguments.trim().is_empty() {
                    "{}"
                } else {
                    &call.arguments
                })
                .unwrap_or_else(|_| serde_json::json!({}));
                Some(LlmToolCall {
                    id: call.id.unwrap_or_else(|| format!("openai_call_{index}")),
                    tool_name,
                    arguments,
                })
            })
            .collect();
        LlmResponse {
            content: self.content,
            tool_calls,
        }
    }
}

fn openai_stream_text(data: &str) -> anyhow::Result<Vec<String>> {
    if data.trim() == "[DONE]" {
        return Ok(Vec::new());
    }
    let chunk: StreamChunk = serde_json::from_str(data)?;
    Ok(chunk
        .choices
        .into_iter()
        .filter_map(|choice| choice.delta.content)
        .collect())
}

fn openai_responses_stream_text(data: &str) -> anyhow::Result<Vec<String>> {
    if data.trim() == "[DONE]" {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_str(data)?;
    if value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "response.output_text.delta")
    {
        return Ok(value
            .get("delta")
            .and_then(Value::as_str)
            .map(|text| vec![text.to_string()])
            .unwrap_or_default());
    }
    Ok(Vec::new())
}

fn responses_input(messages: &[ChatMessage]) -> (Option<String>, Vec<Value>) {
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "system" => instructions.push(message.content.clone()),
            "tool" => input.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id.clone().unwrap_or_default(),
                "output": message.content,
            })),
            "assistant" | "user" => {
                if !message.content.trim().is_empty() {
                    input.push(serde_json::json!({
                        "role": message.role,
                        "content": message.content,
                    }));
                }
            }
            _ => input.push(serde_json::json!({
                "role": "user",
                "content": message.content,
            })),
        }
    }
    (
        (!instructions.is_empty()).then(|| instructions.join("\n\n")),
        input,
    )
}

fn parse_responses_output(value: &Value) -> LlmResponse {
    let mut response = LlmResponse::default();
    let Some(output) = value.get("output").and_then(Value::as_array) else {
        return response;
    };
    for item in output {
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "message" => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if let Some(text) = part
                            .get("text")
                            .or_else(|| part.get("content"))
                            .and_then(Value::as_str)
                        {
                            if !response.content.is_empty() && !text.is_empty() {
                                response.content.push('\n');
                            }
                            response.content.push_str(text);
                        }
                    }
                }
            }
            "function_call" => {
                let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .and_then(|raw| serde_json::from_str(raw).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                response.tool_calls.push(LlmToolCall {
                    id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    tool_name: name.to_string(),
                    arguments,
                });
            }
            _ => {}
        }
    }
    response
}

fn openai_chat_tool_choice(
    tools: Option<&Vec<OpenAiTool>>,
    configured: Option<&str>,
) -> Option<&'static str> {
    if tools.is_none() {
        return None;
    }
    match configured {
        Some("none") => Some("none"),
        Some("required") => Some("required"),
        _ => Some("auto"),
    }
}

fn openai_responses_tool_choice(configured: Option<&str>) -> Option<Value> {
    configured.map(|choice| match choice {
        "none" | "auto" | "required" => Value::String(choice.to_string()),
        name => serde_json::json!({ "type": "function", "name": name }),
    })
}

fn openai_chat_response_format(options: &LlmRequestOptionsSection) -> Option<Value> {
    if let Some(schema) = &options.json_schema {
        return Some(serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": options.json_schema_name.as_deref().unwrap_or("alia_response"),
                "schema": schema,
            }
        }));
    }
    options
        .json_mode
        .then(|| serde_json::json!({ "type": "json_object" }))
}

fn openai_responses_text_format(options: &LlmRequestOptionsSection) -> Option<Value> {
    if let Some(schema) = &options.json_schema {
        return Some(serde_json::json!({
            "format": {
                "type": "json_schema",
                "name": options.json_schema_name.as_deref().unwrap_or("alia_response"),
                "schema": schema,
            }
        }));
    }
    options
        .json_mode
        .then(|| serde_json::json!({ "format": { "type": "json_object" } }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_stream_text_extracts_delta_content() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        assert_eq!(openai_stream_text(data).unwrap(), vec!["Hello"]);
    }

    #[test]
    fn openai_stream_text_ignores_done_marker() {
        assert!(openai_stream_text("[DONE]").unwrap().is_empty());
    }

    #[test]
    fn responses_stream_text_extracts_delta() {
        let data = r#"{"type":"response.output_text.delta","delta":"Hi"}"#;
        assert_eq!(openai_responses_stream_text(data).unwrap(), vec!["Hi"]);
    }

    #[test]
    fn parses_responses_output_text_and_function_call() {
        let value = serde_json::json!({
            "output": [
                {"type":"message","content":[{"type":"output_text","text":"Done"}]},
                {"type":"function_call","call_id":"call_1","name":"memory_store","arguments":"{\"value\":1}"}
            ]
        });
        let response = parse_responses_output(&value);
        assert_eq!(response.content, "Done");
        assert_eq!(response.tool_calls[0].tool_name, "memory_store");
        assert_eq!(response.tool_calls[0].arguments["value"], 1);
    }

    #[test]
    fn openai_stream_aggregate_collects_tool_call_arguments() {
        let mut aggregate = OpenAiStreamAggregate::default();
        aggregate
            .apply_data(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"search","arguments":"{\"q\""}}]}}]}"#,
            )
            .unwrap();
        aggregate
            .apply_data(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"moon\"}"}}]}}]}"#,
            )
            .unwrap();
        let response = aggregate.into_response();
        assert_eq!(response.tool_calls[0].id, "call_1");
        assert_eq!(response.tool_calls[0].tool_name, "search");
        assert_eq!(
            response.tool_calls[0].arguments,
            serde_json::json!({"q": "moon"})
        );
    }
}
