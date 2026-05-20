//! Google Gemini `generateContent` adapter.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::RwLock;

use crate::config::{LlmProfileKind, LlmRequestOptionsSection, ServerConfig};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    ChatMessage, LlmAdapter, LlmCapabilities, LlmResponse, LlmToolCall, ToolDefinition,
};
use crate::providers::llm::errors::{classify_http_error, classify_transport_error};
use crate::providers::llm::streaming::SseDecoder;
use crate::settings::values::{f32_setting, string_setting, u64_setting};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_response: Option<GeminiFunctionResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFunctionResponse {
    name: String,
    response: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    temperature: f32,
    max_output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop_sequences: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_schema: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiTool {
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: Value,
}

impl From<&ToolDefinition> for GeminiFunctionDeclaration {
    fn from(tool: &ToolDefinition) -> Self {
        Self {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(Debug, Deserialize)]
struct StreamGenerateContentResponse {
    #[serde(default)]
    candidates: Vec<GeminiStreamCandidate>,
}

#[derive(Debug, Deserialize)]
struct GeminiStreamCandidate {
    content: Option<GeminiContent>,
}

pub struct GeminiLlm {
    client: Client,
    profile_id: String,
    config: Arc<RwLock<GeminiConfig>>,
}

#[derive(Debug, Clone)]
struct GeminiConfig {
    api_key: String,
    base_url: String,
    model: String,
    request: LlmRequestOptionsSection,
    health_probe_enabled: bool,
}

impl GeminiLlm {
    pub fn new() -> Self {
        Self::for_profile("minor")
    }

    pub fn for_profile(profile_id: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            profile_id: profile_id.into(),
            config: Arc::new(RwLock::new(GeminiConfig {
                api_key: String::new(),
                base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
                model: "gemini-2.5-flash".to_string(),
                request: LlmRequestOptionsSection::default(),
                health_probe_enabled: false,
            })),
        }
    }

    async fn chat_internal(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolDefinition]>,
    ) -> anyhow::Result<GenerateContentResponse> {
        let config = self.config.read().await;
        let (system_instruction, contents) = gemini_contents(messages);
        let tools = tools.filter(|tools| !tools.is_empty()).map(|tools| {
            vec![GeminiTool {
                function_declarations: tools.iter().map(GeminiFunctionDeclaration::from).collect(),
            }]
        });
        let request = GenerateContentRequest {
            contents,
            system_instruction,
            generation_config: Some(gemini_generation_config(&config.request)),
            tools,
            tool_config: gemini_tool_config(config.request.tool_choice.as_deref()),
        };

        let response = self
            .client
            .post(gemini_url(&config.base_url, &config.model))
            .header("x-goog-api-key", config.api_key.trim())
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|error| classify_transport_error("gemini", error))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(classify_http_error("gemini", status, body));
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
        let (system_instruction, contents) = gemini_contents(messages);
        let tools = tools.filter(|tools| !tools.is_empty()).map(|tools| {
            vec![GeminiTool {
                function_declarations: tools.iter().map(GeminiFunctionDeclaration::from).collect(),
            }]
        });
        let request = GenerateContentRequest {
            contents,
            system_instruction,
            generation_config: Some(gemini_generation_config(&config.request)),
            tools,
            tool_config: gemini_tool_config(config.request.tool_choice.as_deref()),
        };

        let response = self
            .client
            .post(gemini_stream_url(&config.base_url, &config.model))
            .header("x-goog-api-key", config.api_key.trim())
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&request)
            .send()
            .await
            .map_err(|error| classify_transport_error("gemini", error))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(classify_http_error("gemini", status, body));
        }

        Ok(response)
    }
}

impl Default for GeminiLlm {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for GeminiLlm {
    fn name(&self) -> &'static str {
        "gemini"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let server_config: &ServerConfig = &ctx.config;
        let profile = server_config.profiles.llm_profile(&self.profile_id)?;
        if profile.provider != "gemini" || profile.kind != LlmProfileKind::Chat {
            anyhow::bail!(
                "LLM profile '{}' is not a Gemini chat profile",
                self.profile_id
            );
        }
        let mut config = self.config.write().await;
        config.api_key = server_config.providers.gemini.api_key.clone();
        config.base_url = server_config.providers.gemini.base_url.clone();
        config.model = profile.model.clone();
        config.request = profile.request.clone();
        config.health_probe_enabled = server_config.providers.gemini.health_probe_enabled;
        if config.api_key.is_empty() {
            tracing::warn!("Gemini API key not configured");
        }
        tracing::info!(
            profile = %self.profile_id,
            model = %config.model,
            base_url = %config.base_url,
            "Gemini LLM initialized"
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
                    config.model.trim_start_matches("models/")
                ))
                .header("x-goog-api-key", config.api_key.trim())
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => ModuleHealth::Healthy,
                Ok(response) => ModuleHealth::Degraded(format!(
                    "Gemini model probe returned {}",
                    response.status()
                )),
                Err(error) => ModuleHealth::Degraded(format!("Gemini probe failed: {error}")),
            }
        }
    }

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut applied = Vec::new();
        let mut config = self.config.write().await;

        if let Some(value) = string_setting(updates, "core.providers.gemini.api_key") {
            config.api_key = value;
            applied.push("core.providers.gemini.api_key".to_string());
        }
        if let Some(value) = string_setting(updates, "core.providers.gemini.base_url") {
            config.base_url = value;
            applied.push("core.providers.gemini.base_url".to_string());
        }
        let model_key = format!("core.profiles.llm.{}.model", self.profile_id);
        if let Some(value) = string_setting(updates, &model_key) {
            config.model = value;
            applied.push(model_key);
        }
        let max_tokens_key = format!("core.profiles.llm.{}.max_output_tokens", self.profile_id);
        if let Some(value) = u64_setting(updates, &max_tokens_key) {
            config.request.max_output_tokens = value.min(u32::MAX as u64) as u32;
            applied.push(max_tokens_key);
        }
        let temperature_key = format!("core.profiles.llm.{}.temperature", self.profile_id);
        if let Some(value) = f32_setting(updates, &temperature_key) {
            config.request.temperature = value;
            applied.push(temperature_key);
        }
        let top_p_key = format!("core.profiles.llm.{}.top_p", self.profile_id);
        if let Some(value) = f32_setting(updates, &top_p_key) {
            config.request.top_p = Some(value);
            applied.push(top_p_key);
        }

        if !applied.is_empty() {
            tracing::info!(module = self.name(), applied = ?applied, "runtime settings applied");
        }
        Ok(applied)
    }
}

#[async_trait]
impl LlmAdapter for GeminiLlm {
    async fn llm_capabilities(&self) -> LlmCapabilities {
        let config = self.config.read().await;
        let mut capabilities = LlmCapabilities::minimal(self.name());
        capabilities.supports_streaming = true;
        capabilities.supports_tool_calling = true;
        capabilities.supports_streaming_tool_calls = true;
        capabilities.supports_system_messages = true;
        capabilities.supports_parallel_tool_calls = true;
        capabilities.max_output_tokens = Some(config.request.max_output_tokens);
        capabilities.provider_specific.insert(
            "api_family".to_string(),
            serde_json::json!("generate_content"),
        );
        capabilities
            .provider_specific
            .insert("model".to_string(), serde_json::json!(config.model));
        capabilities
    }

    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String> {
        let response = self.chat_internal(messages, None).await?;
        Ok(gemini_response(response).content)
    }

    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let response = self.chat_internal(messages, Some(tools)).await?;
        Ok(gemini_response(response))
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
        let mut aggregate = GeminiStreamAggregate::default();

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
                            match gemini_stream_text(&event.data) {
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
                    Err(error) => {
                        yield Err(anyhow::anyhow!("Gemini stream error: {error}"));
                        break;
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

fn gemini_url(base_url: &str, model: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let model = model.trim_start_matches("models/");
    format!("{base}/models/{model}:generateContent")
}

fn gemini_stream_url(base_url: &str, model: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let model = model.trim_start_matches("models/");
    format!("{base}/models/{model}:streamGenerateContent?alt=sse")
}

fn gemini_generation_config(options: &LlmRequestOptionsSection) -> GeminiGenerationConfig {
    GeminiGenerationConfig {
        temperature: options.temperature,
        max_output_tokens: options.max_output_tokens,
        top_p: options.top_p,
        stop_sequences: options.stop_sequences.clone(),
        response_mime_type: (options.json_mode || options.json_schema.is_some())
            .then(|| "application/json".to_string()),
        response_schema: options.json_schema.clone(),
    }
}

fn gemini_tool_config(configured: Option<&str>) -> Option<Value> {
    configured.map(|choice| match choice {
        "none" => serde_json::json!({ "functionCallingConfig": { "mode": "NONE" } }),
        "required" | "any" => serde_json::json!({ "functionCallingConfig": { "mode": "ANY" } }),
        "auto" => serde_json::json!({ "functionCallingConfig": { "mode": "AUTO" } }),
        name => serde_json::json!({
            "functionCallingConfig": {
                "mode": "ANY",
                "allowedFunctionNames": [name],
            }
        }),
    })
}

fn gemini_contents(messages: &[ChatMessage]) -> (Option<GeminiContent>, Vec<GeminiContent>) {
    let mut system = Vec::new();
    let mut contents = Vec::new();

    for message in messages {
        match message.role.as_str() {
            "system" => system.push(message.content.clone()),
            "assistant" => {
                let mut parts = Vec::new();
                if !message.content.trim().is_empty() {
                    parts.push(text_part(message.content.clone()));
                }
                parts.extend(message.tool_calls.iter().map(|call| GeminiPart {
                    text: None,
                    function_call: Some(GeminiFunctionCall {
                        name: call.tool_name.clone(),
                        args: call.arguments.clone(),
                    }),
                    function_response: None,
                }));
                if !parts.is_empty() {
                    contents.push(GeminiContent {
                        role: "model".to_string(),
                        parts,
                    });
                }
            }
            "tool" => contents.push(GeminiContent {
                role: "user".to_string(),
                parts: vec![GeminiPart {
                    text: None,
                    function_call: None,
                    function_response: Some(GeminiFunctionResponse {
                        name: message.name.clone().unwrap_or_else(|| "tool".to_string()),
                        response: tool_response_payload(&message.content),
                    }),
                }],
            }),
            _ => contents.push(GeminiContent {
                role: "user".to_string(),
                parts: vec![text_part(message.content.clone())],
            }),
        }
    }

    let system_instruction = (!system.is_empty()).then(|| GeminiContent {
        role: "user".to_string(),
        parts: vec![text_part(system.join("\n\n"))],
    });
    (system_instruction, contents)
}

fn text_part(text: String) -> GeminiPart {
    GeminiPart {
        text: Some(text),
        function_call: None,
        function_response: None,
    }
}

fn tool_response_payload(content: &str) -> Value {
    match serde_json::from_str(content) {
        Ok(Value::Object(object)) => Value::Object(object),
        Ok(value) => json!({ "result": value }),
        Err(_) => {
            let mut object = Map::new();
            object.insert("result".to_string(), Value::String(content.to_string()));
            Value::Object(object)
        }
    }
}

fn gemini_response(response: GenerateContentResponse) -> LlmResponse {
    let mut output = LlmResponse::default();
    let Some(candidate) = response.candidates.into_iter().next() else {
        return output;
    };
    for (index, part) in candidate.content.parts.into_iter().enumerate() {
        if let Some(text) = part.text {
            if !output.content.is_empty() && !text.is_empty() {
                output.content.push('\n');
            }
            output.content.push_str(&text);
        }
        if let Some(call) = part.function_call {
            output.tool_calls.push(LlmToolCall {
                id: format!("gemini_call_{}_{}", index, call.name),
                tool_name: call.name,
                arguments: if call.args.is_null() {
                    json!({})
                } else {
                    call.args
                },
            });
        }
    }
    output
}

fn gemini_stream_text(data: &str) -> anyhow::Result<Vec<String>> {
    Ok(gemini_stream_delta(data)?.text)
}

#[derive(Default)]
struct GeminiStreamAggregate {
    content: String,
    tool_calls: Vec<LlmToolCall>,
}

impl GeminiStreamAggregate {
    fn apply_data(&mut self, data: &str) -> anyhow::Result<()> {
        let delta = gemini_stream_delta(data)?;
        for text in delta.text {
            self.content.push_str(&text);
        }
        self.tool_calls.extend(delta.tool_calls);
        Ok(())
    }

    fn into_response(self) -> LlmResponse {
        LlmResponse {
            content: self.content,
            tool_calls: self.tool_calls,
        }
    }
}

#[derive(Default)]
struct GeminiStreamDelta {
    text: Vec<String>,
    tool_calls: Vec<LlmToolCall>,
}

fn gemini_stream_delta(data: &str) -> anyhow::Result<GeminiStreamDelta> {
    if data.trim() == "[DONE]" {
        return Ok(GeminiStreamDelta::default());
    }
    let chunk: StreamGenerateContentResponse = serde_json::from_str(data)?;
    let mut delta = GeminiStreamDelta::default();
    for part in chunk
        .candidates
        .into_iter()
        .filter_map(|candidate| candidate.content)
        .flat_map(|content| content.parts)
    {
        if let Some(text) = part.text {
            delta.text.push(text);
        }
        if let Some(call) = part.function_call {
            let index = delta.tool_calls.len();
            delta.tool_calls.push(LlmToolCall {
                id: format!("gemini_call_{}_{}", index, call.name),
                tool_name: call.name,
                arguments: if call.args.is_null() {
                    json!({})
                } else {
                    call.args
                },
            });
        }
    }
    Ok(delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_stream_url_uses_sse_endpoint() {
        assert_eq!(
            gemini_stream_url(
                "https://generativelanguage.googleapis.com/v1beta",
                "models/gemini-2.5-flash"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn gemini_stream_text_extracts_candidate_parts() {
        let data = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hel"},{"text":"lo"}]}}]}"#;
        assert_eq!(gemini_stream_text(data).unwrap(), vec!["Hel", "lo"]);
    }

    #[test]
    fn gemini_stream_aggregate_collects_function_calls() {
        let mut aggregate = GeminiStreamAggregate::default();
        aggregate
            .apply_data(
                r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"search","args":{"q":"moon"}}}]}}]}"#,
            )
            .unwrap();
        let response = aggregate.into_response();
        assert_eq!(response.tool_calls[0].tool_name, "search");
        assert_eq!(
            response.tool_calls[0].arguments,
            serde_json::json!({"q": "moon"})
        );
    }
}
