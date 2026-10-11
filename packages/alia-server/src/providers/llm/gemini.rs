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

use crate::config::{
    GeminiProviderSection, LlmProfileKind, LlmProfileSection, LlmRequestOptionsSection,
};
use crate::modules::platform::lifecycle::{ModuleHealth, ModuleLifecycle};
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
    provider_id: String,
    api_key: String,
    base_url: String,
    model: String,
    request: LlmRequestOptionsSection,
    health_probe_enabled: bool,
}

impl GeminiLlm {
    pub fn new(
        profile_id: impl Into<String>,
        profile: &LlmProfileSection,
        provider: &GeminiProviderSection,
    ) -> anyhow::Result<Self> {
        let profile_id = profile_id.into();
        anyhow::ensure!(
            profile.kind == LlmProfileKind::Chat,
            "LLM profile '{}' is not a Gemini chat profile",
            profile_id
        );
        Ok(Self {
            client: Client::new(),
            profile_id,
            config: Arc::new(RwLock::new(GeminiConfig {
                provider_id: profile.provider.clone(),
                api_key: provider.api_key.clone(),
                base_url: provider.base_url.clone(),
                model: profile.model.clone(),
                request: profile.request.clone(),
                health_probe_enabled: provider.health_probe_enabled,
            })),
        })
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

#[async_trait]
impl ModuleLifecycle for GeminiLlm {
    fn name(&self) -> &'static str {
        "gemini"
    }

    async fn init(&mut self) -> anyhow::Result<()> {
        let config = self.config.read().await;
        if config.api_key.is_empty() {
            tracing::warn!("Gemini API key not configured");
        }
        tracing::info!(
            profile = %self.profile_id,
            provider = %config.provider_id,
            model = %config.model,
            base_url = %config.base_url,
            "Gemini LLM initialized"
        );
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        // Probe deadlines and settings ownership are independent of generation.
        let config = self.config.read().await.clone();
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
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => ModuleHealth::Healthy,
                Ok(response) => ModuleHealth::Degraded(format!(
                    "Gemini model probe returned {}",
                    response.status()
                )),
                Err(error) => {
                    // Raw transport errors may contain credential-bearing URLs.
                    let failure = if error.is_timeout() {
                        "timed out after 5s"
                    } else if error.is_connect() {
                        "connection failed"
                    } else {
                        "request failed"
                    };
                    ModuleHealth::Degraded(format!("Gemini probe {failure}"))
                }
            }
        }
    }

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut applied = Vec::new();
        let mut config = self.config.write().await;

        let provider_api_key = format!("core.providers.llm.{}.api_key", config.provider_id);
        if let Some(value) = string_setting(updates, &provider_api_key) {
            config.api_key = value;
            applied.push(provider_api_key);
        }
        let provider_base_url = format!("core.providers.llm.{}.base_url", config.provider_id);
        if let Some(value) = string_setting(updates, &provider_base_url) {
            config.base_url = value;
            applied.push(provider_base_url);
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
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    fn test_llm() -> GeminiLlm {
        let mut profile = crate::config::ProfilesSection::default()
            .llm_profile("major")
            .unwrap()
            .clone();
        profile.provider = "gemini".into();
        GeminiLlm::new("major", &profile, &GeminiProviderSection::default()).unwrap()
    }

    async fn accept_probe(listener: &TcpListener) -> (TcpStream, String) {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0; 1];
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            assert!(request.len() <= 8192);
        }
        (socket, String::from_utf8(request).unwrap())
    }

    #[tokio::test]
    async fn health_probe_default_disabled_and_missing_key_do_not_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let llm = test_llm();
        {
            let mut config = llm.config.write().await;
            assert!(!config.health_probe_enabled);
            config.base_url = format!("http://{}/v1beta", listener.local_addr().unwrap());
        }
        assert!(matches!(
            llm.health_check().await,
            ModuleHealth::Unhealthy(message) if message == "API key not configured"
        ));
        llm.config.write().await.api_key = "local-health-secret".into();
        assert!(matches!(
            llm.health_check().await,
            ModuleHealth::Degraded(message)
                if message == "API key configured; live provider probe disabled"
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn health_probe_preserves_http_status_without_disclosing_request_or_body() {
        for status in ["200 OK", "401 Unauthorized", "404 Not Found"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let llm = test_llm();
            {
                let mut config = llm.config.write().await;
                config.api_key = "local-health-secret".into();
                config.base_url = format!(
                    "http://{}/private-url-secret/v1beta/",
                    listener.local_addr().unwrap()
                );
                config.model = "models/test-model".into();
                config.health_probe_enabled = true;
            }
            let peer = async {
                let (mut socket, request) = accept_probe(&listener).await;
                assert!(
                    request.starts_with(
                        "GET /private-url-secret/v1beta/models/test-model HTTP/1.1\r\n"
                    )
                );
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("\r\nx-goog-api-key: local-health-secret\r\n")
                );
                let body = "private-response-secret";
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            };
            let (health, ()) = tokio::time::timeout(Duration::from_secs(2), async {
                tokio::join!(llm.health_check(), peer)
            })
            .await
            .unwrap();
            if status == "200 OK" {
                assert!(matches!(health, ModuleHealth::Healthy));
            } else {
                let ModuleHealth::Degraded(message) = health else {
                    panic!("HTTP rejection must be degraded");
                };
                assert_eq!(message, format!("Gemini model probe returned {status}"));
                for secret in [
                    "local-health-secret",
                    "private-url-secret",
                    "private-response-secret",
                    "x-goog-api-key",
                ] {
                    assert!(!message.contains(secret));
                }
            }
        }
    }

    #[tokio::test]
    async fn stalled_health_probe_times_out_without_blocking_settings_updates() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let llm = test_llm();
        {
            let mut config = llm.config.write().await;
            config.api_key = "local-health-secret".into();
            config.base_url = format!(
                "http://{}/private-url-secret/v1beta",
                listener.local_addr().unwrap()
            );
            config.model = "test-model".into();
            config.health_probe_enabled = true;
        }
        let started = Instant::now();
        let probe = llm.health_check();
        tokio::pin!(probe);
        let (_socket, request) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                request = accept_probe(&listener) => request,
                _ = &mut probe => panic!("probe must wait for the stalled peer"),
            }
        })
        .await
        .unwrap();
        assert!(request.contains("/models/test-model HTTP/1.1"));
        let applied = tokio::time::timeout(
            Duration::from_millis(500),
            llm.apply_settings(&BTreeMap::from([
                (
                    "core.providers.llm.gemini.api_key".into(),
                    json!("next-health-secret"),
                ),
                (
                    "core.profiles.llm.major.model".into(),
                    json!("models/next-model"),
                ),
            ])),
        )
        .await
        .expect("a stalled probe must not hold the settings read lock")
        .unwrap();
        assert_eq!(applied.len(), 2);
        let health = tokio::time::timeout(Duration::from_secs(7), &mut probe)
            .await
            .expect("the independent provider deadline must finish the probe");
        assert!(matches!(
            health,
            ModuleHealth::Degraded(message) if message == "Gemini probe timed out after 5s"
        ));
        assert!(started.elapsed() >= Duration::from_millis(4500));
        assert!(started.elapsed() < Duration::from_secs(7));

        let peer = async {
            let (mut socket, request) = accept_probe(&listener).await;
            assert!(request.contains("/models/next-model HTTP/1.1"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("\r\nx-goog-api-key: next-health-secret\r\n")
            );
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        };
        let (health, ()) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(llm.health_check(), peer)
        })
        .await
        .unwrap();
        assert!(matches!(health, ModuleHealth::Healthy));
    }

    #[tokio::test]
    async fn cancelled_health_probe_releases_its_stalled_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let llm = test_llm();
        {
            let mut config = llm.config.write().await;
            config.api_key = "local-health-secret".into();
            config.base_url = format!("http://{}/v1beta", listener.local_addr().unwrap());
            config.health_probe_enabled = true;
        }
        let mut socket = {
            let probe = llm.health_check();
            tokio::pin!(probe);
            tokio::time::timeout(Duration::from_secs(2), async {
                tokio::select! {
                    (socket, _) = accept_probe(&listener) => socket,
                    _ = &mut probe => panic!("probe must wait for the stalled peer"),
                }
            })
            .await
            .unwrap()
        };
        let mut byte = [0; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
            .await
            .expect("dropping the probe must close the pending request")
            .unwrap();
        assert_eq!(closed, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn health_probe_connection_refusal_is_distinct_and_redacted() {
        struct SocketPath(std::path::PathBuf);
        impl Drop for SocketPath {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let path =
            std::env::temp_dir().join(format!("alia-{}.sock", uuid::Uuid::new_v4().simple()));
        let reserved = tokio::net::UnixSocket::new_stream().unwrap();
        reserved.bind(&path).unwrap();
        let _socket_path = SocketPath(path.clone());
        let refused = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::UnixStream::connect(&path),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::ConnectionRefused);
        let mut llm = test_llm();
        llm.client = Client::builder().unix_socket(path).build().unwrap();
        {
            let mut config = llm.config.write().await;
            config.api_key = "local-health-secret".into();
            config.base_url = "http://localhost/private-url-secret/v1beta".into();
            config.health_probe_enabled = true;
        }
        let health = tokio::time::timeout(Duration::from_secs(2), llm.health_check())
            .await
            .unwrap();
        assert!(matches!(
            health,
            ModuleHealth::Degraded(message) if message == "Gemini probe connection failed"
        ));
        drop(reserved);
    }

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
