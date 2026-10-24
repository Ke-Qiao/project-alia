//! One model service for profile lifecycle and Rig completion requests.
//! User history remains a text-only domain record; native transcripts stay in Rig.
use std::{
    collections::BTreeMap,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use rig_core::{
    DynModel,
    completion::{CompletionRequest, CompletionResponse, FinishReason},
    message::{AssistantContent, Message},
    operation::Completion,
    streaming::{CompletionStream, Item, StreamEvent},
};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::{
    config::{LlmProfileSection, LlmProviderProfileRef, LlmRequestOptionsSection, OpenAiApiFamily},
    modules::platform::lifecycle::{ModuleHealth, ModuleLifecycle},
    providers::adapter::{ChatMessage, LlmCapabilities},
};

pub type TextStream = Pin<Box<dyn Stream<Item = Result<String>> + Send>>;
pub const MODEL_TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_TEXT_BYTES: usize = 256 * 1024;

#[derive(Clone)]
enum Api {
    Chat,
    Responses,
    Anthropic { version: String },
    Gemini,
}

#[derive(Clone)]
struct Config {
    provider_id: String,
    api: Api,
    api_key: String,
    base_url: String,
    model: String,
    request: LlmRequestOptionsSection,
    params: BTreeMap<String, Value>,
    probe: bool,
}

struct State {
    config: Option<Config>,
    model: Option<DynModel<Completion>>,
    generation: u64,
}

pub struct LlmService {
    name: &'static str,
    profile_id: String,
    state: RwLock<State>,
    stopped: AtomicBool,
    lifecycle: Option<Box<dyn ModuleLifecycle>>,
}

impl LlmService {
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn noop() -> Self {
        Self {
            name: "noop_llm",
            profile_id: "noop".into(),
            state: RwLock::new(State {
                config: None,
                model: None,
                generation: 0,
            }),
            stopped: AtomicBool::new(false),
            lifecycle: None,
        }
    }

    pub fn from_model(name: &'static str, model: impl Into<DynModel<Completion>>) -> Self {
        Self {
            name,
            state: RwLock::new(State {
                config: None,
                model: Some(model.into()),
                generation: 0,
            }),
            ..Self::noop()
        }
    }

    /// Lifecycle ownership is independent of the model transport (also useful for local models).
    pub fn with_lifecycle(mut self, lifecycle: Box<dyn ModuleLifecycle>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    pub fn from_profile(
        id: &str,
        profile: &LlmProfileSection,
        provider: LlmProviderProfileRef<'_>,
    ) -> Result<Self> {
        let (name, api, api_key, base_url, probe) = match provider {
            LlmProviderProfileRef::Noop => return Ok(Self::noop()),
            LlmProviderProfileRef::OpenAi(config) => {
                ensure!(cfg!(feature = "llm-openai"), "profile requires llm-openai");
                (
                    "openai",
                    match config.api_family {
                        OpenAiApiFamily::ChatCompletions => Api::Chat,
                        OpenAiApiFamily::Responses => Api::Responses,
                    },
                    config.api_key.clone(),
                    config.base_url.clone(),
                    config.health_probe_enabled,
                )
            }
            LlmProviderProfileRef::Anthropic(config) => {
                ensure!(
                    cfg!(feature = "llm-anthropic"),
                    "profile requires llm-anthropic"
                );
                (
                    "anthropic",
                    Api::Anthropic {
                        version: config.version.clone(),
                    },
                    config.api_key.clone(),
                    config.base_url.clone(),
                    config.health_probe_enabled,
                )
            }
            LlmProviderProfileRef::Gemini(config) => {
                ensure!(cfg!(feature = "llm-gemini"), "profile requires llm-gemini");
                (
                    "gemini",
                    Api::Gemini,
                    config.api_key.clone(),
                    config.base_url.clone(),
                    config.health_probe_enabled,
                )
            }
        };
        let config = Config {
            provider_id: profile.provider.clone(),
            api,
            api_key,
            base_url,
            model: profile.model.clone(),
            request: profile.request.clone(),
            params: profile.provider_specific.clone(),
            probe,
        };
        let model = build_model(&config)?;
        Ok(Self {
            name,
            profile_id: id.into(),
            state: RwLock::new(State {
                config: Some(config),
                model: Some(model),
                generation: 0,
            }),
            stopped: AtomicBool::new(false),
            lifecycle: None,
        })
    }

    async fn request_model(
        &self,
        request: &mut CompletionRequest,
    ) -> Result<Option<DynModel<Completion>>> {
        ensure!(
            !self.stopped.load(Ordering::Acquire),
            "model service is stopped"
        );
        let state = self.state.read().await;
        if let Some(config) = &state.config {
            ensure!(
                !config.api_key.trim().is_empty(),
                "model credential not configured"
            );
            request.max_tokens = Some(
                request
                    .max_tokens
                    .unwrap_or(config.request.max_output_tokens as u64)
                    .min(config.request.max_output_tokens as u64),
            );
            request.temperature = request
                .temperature
                .or(Some(config.request.temperature as f64));
            request.options.top_p = request
                .options
                .top_p
                .or(config.request.top_p.map(f64::from));
            if request.options.stop.is_empty() {
                request.options.stop = config.request.stop_sequences.clone();
            }
            if request.output_schema.is_none() {
                request.output_schema = config
                    .request
                    .json_schema
                    .clone()
                    .map(|mut schema| {
                        // Rig reads the OpenAI response schema's name from its title.
                        if matches!(config.api, Api::Chat | Api::Responses) {
                            let object = schema
                                .as_object_mut()
                                .context("OpenAI response schema must be an object")?;
                            object.insert(
                                "title".into(),
                                Value::String(
                                    config
                                        .request
                                        .json_schema_name
                                        .clone()
                                        .unwrap_or_else(|| "alia_response".into()),
                                ),
                            );
                        }
                        serde_json::from_value(schema).context("invalid response schema")
                    })
                    .transpose()?;
            }
            let mut params: serde_json::Map<String, Value> =
                config.params.clone().into_iter().collect();
            if let Some(value) = &request.additional_params {
                ensure!(value.is_object(), "provider parameters must be an object");
                params.extend(value.as_object().unwrap().clone());
            }
            for reserved in [
                "model",
                "messages",
                "input",
                "instructions",
                "tools",
                "tool_choice",
                "stream",
                "max_tokens",
                "max_output_tokens",
                "max_completion_tokens",
                "contents",
                "systemInstruction",
                "system_instruction",
                "generationConfig",
                "generation_config",
                "toolConfig",
                "tool_config",
            ] {
                ensure!(
                    !params.contains_key(reserved),
                    "provider parameters cannot override controlled request field {reserved}"
                );
            }
            if let Some(value) = config.request.presence_penalty {
                params.insert("presence_penalty".into(), value.into());
            }
            if let Some(value) = config.request.frequency_penalty {
                params.insert("frequency_penalty".into(), value.into());
            }
            if config.request.json_mode && request.output_schema.is_none() {
                match config.api {
                    Api::Chat => {
                        params.insert(
                            "response_format".into(),
                            serde_json::json!({"type":"json_object"}),
                        );
                    }
                    Api::Responses => {
                        params
                            .entry("text")
                            .or_insert_with(|| serde_json::json!({}))
                            .as_object_mut()
                            .context("Responses text options must be an object")?
                            .insert("format".into(), serde_json::json!({"type":"json_object"}));
                    }
                    Api::Gemini => {
                        params.insert(
                            "generationConfig".into(),
                            serde_json::json!({"responseMimeType":"application/json"}),
                        );
                    }
                    Api::Anthropic { .. } => {
                        anyhow::bail!("Anthropic JSON output requires json_schema")
                    }
                }
            }
            request.additional_params = (!params.is_empty()).then_some(Value::Object(params));
        }
        request.record_telemetry_content = false;
        request.accept_unknown_finish_reasons = false;
        ensure!(
            serde_json::to_vec(&request)?.len() <= 4 * 1024 * 1024,
            "model request byte budget exceeded"
        );
        Ok(state.model.clone())
    }

    pub async fn completion(&self, mut request: CompletionRequest) -> Result<CompletionResponse> {
        let Some(model) = self.request_model(&mut request).await? else {
            return Ok(noop_response());
        };
        let response = tokio::time::timeout(MODEL_TIMEOUT, model.call(request))
            .await
            .context("model deadline exceeded")??;
        validate_response(&response)?;
        Ok(response)
    }

    pub async fn completion_stream(
        &self,
        mut request: CompletionRequest,
    ) -> Result<CompletionStream> {
        let model = self
            .request_model(&mut request)
            .await?
            .context("noop model has no completion stream")?;
        Ok(model.stream(request)?)
    }

    pub async fn text(&self, messages: &[ChatMessage]) -> Result<String> {
        let response = self.completion(text_request(messages)?).await?;
        ensure!(
            !response
                .choice
                .iter()
                .any(|part| matches!(part, AssistantContent::ToolCall(_))),
            "text task returned a tool call"
        );
        Ok(response.text())
    }

    pub async fn text_stream(&self, messages: &[ChatMessage]) -> Result<TextStream> {
        let mut request = text_request(messages)?;
        let Some(model) = self.request_model(&mut request).await? else {
            return Ok(Box::pin(futures::stream::once(async {
                Ok("LLM module not configured".into())
            })));
        };
        let mut stream = model.stream(request)?;
        let deadline = tokio::time::Instant::now() + MODEL_TIMEOUT;
        Ok(Box::pin(async_stream::try_stream! {
            let mut bytes = 0usize;
            while let Some(item) = tokio::time::timeout_at(deadline, stream.next()).await.context("model deadline exceeded")? {
                match item? {
                    Item::Event(StreamEvent::Text { text, .. }) => {
                        if text.len() > MAX_TEXT_BYTES.saturating_sub(bytes) { Err(anyhow::anyhow!("model text byte budget exceeded"))?; }
                        bytes += text.len();
                        yield text;
                    }
                    Item::Event(StreamEvent::Start { kind: rig_core::streaming::PartKind::ToolCall, .. }) => Err(anyhow::anyhow!("text task returned a tool call"))?,
                    Item::Unknown(_) => Err(anyhow::anyhow!("unsupported provider stream event"))?,
                    _ => {}
                }
            }
            let response = tokio::time::timeout_at(deadline, stream.finish()).await.context("model deadline exceeded")??;
            validate_response(&response)?;
        }))
    }

    pub async fn tool_choice(&self) -> Result<Option<rig_core::message::ToolChoice>> {
        use rig_core::message::{ToolChoice, ToolName};
        let state = self.state.read().await;
        let Some(value) = state
            .config
            .as_ref()
            .and_then(|config| config.request.tool_choice.as_deref())
        else {
            return Ok(None);
        };
        Ok(Some(match value {
            "auto" => ToolChoice::Auto,
            "none" => ToolChoice::None,
            "required" | "any" => ToolChoice::Required,
            name => ToolChoice::Specific {
                function_names: vec![ToolName::new(name)?],
            },
        }))
    }

    pub async fn llm_capabilities(&self) -> LlmCapabilities {
        let state = self.state.read().await;
        let mut result = LlmCapabilities::minimal(self.name);
        if state.model.is_some() {
            result.supports_streaming = true;
            result.supports_tool_calling = true;
            result.supports_streaming_tool_calls = true;
            result.supports_system_messages = true;
            result.supports_json_mode = true;
            result.supports_parallel_tool_calls = true;
        }
        if let Some(config) = &state.config {
            result.max_output_tokens = Some(config.request.max_output_tokens);
            result
                .provider_specific
                .insert("model".into(), config.model.clone().into());
        }
        result
    }
}

pub fn text_request(messages: &[ChatMessage]) -> Result<CompletionRequest> {
    // Domain user_id binds persisted source identity in this user's conversation.
    // It is not a provider author/tool name and never becomes native protocol metadata.
    let mut request = CompletionRequest::new(Message::user("Continue."));
    if !messages.is_empty() {
        request.chat_history = messages
            .iter()
            .map(|message| match message.role.as_str() {
                "system" => Ok(Message::system(&message.content)),
                "user" => Ok(Message::user(&message.content)),
                "assistant" => Ok(Message::assistant(&message.content)),
                _ => anyhow::bail!(
                    "domain conversation history cannot contain tool protocol messages"
                ),
            })
            .collect::<Result<_>>()?;
    }
    Ok(request)
}

pub fn validate_response(response: &CompletionResponse) -> Result<()> {
    ensure!(
        response.error.is_none() && response.aborted.is_none(),
        "model response did not complete successfully"
    );
    ensure!(
        matches!(
            response.finish_reason(),
            Some(FinishReason::Stop | FinishReason::ToolCalls)
        ),
        "model response has no successful terminal"
    );
    let mut ids = std::collections::HashSet::new();
    for content in &response.choice {
        if let AssistantContent::ToolCall(call) = content {
            ensure!(
                call.function.invalid_arguments.is_none(),
                "invalid tool arguments"
            );
            ensure!(ids.insert(call.id.clone()), "duplicate tool call identity");
            ensure!(!call.function.name.trim().is_empty(), "empty tool name");
        }
    }
    ensure!(
        response.text().len() <= MAX_TEXT_BYTES,
        "model text byte budget exceeded"
    );
    Ok(())
}

fn noop_response() -> CompletionResponse {
    CompletionResponse::new(
        vec![AssistantContent::text("LLM module not configured")],
        Default::default(),
        rig_core::message::Origin::new("alia.noop", "noop", "noop"),
        Value::Null,
    )
    .with_finish_reason(FinishReason::Stop)
}

#[cfg(any(
    feature = "llm-openai",
    feature = "llm-anthropic",
    feature = "llm-gemini"
))]
fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(MODEL_TIMEOUT)
        .build()?)
}

#[cfg(any(
    feature = "llm-openai",
    feature = "llm-anthropic",
    feature = "llm-gemini"
))]
fn build_model(config: &Config) -> Result<DynModel<Completion>> {
    let url = reqwest::Url::parse(&config.base_url).context("invalid model endpoint")?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "invalid model endpoint"
    );
    ensure!(
        !config.model.trim().is_empty() && config.request.max_output_tokens > 0,
        "invalid model profile"
    );
    let http = rig_reqwest::ReqwestClient::from(http_client()?);
    Ok(match &config.api {
        Api::Chat | Api::Responses => {
            let provider = rig_core::providers::openai::OpenAIConfig::new(config.api_key.clone())
                .with_base_url(config.base_url.clone())
                .connect(http);
            match config.api {
                Api::Chat => provider.chat(&config.model).erase(),
                _ => provider.responses(&config.model).erase(),
            }
        }
        Api::Anthropic { version } => {
            let mut provider =
                rig_core::providers::anthropic::wire::AnthropicConfig::new(config.api_key.clone())
                    .with_base_url(config.base_url.clone());
            provider.version = version.clone();
            provider.connect(http).completion(&config.model).erase()
        }
        Api::Gemini => {
            let root = config
                .base_url
                .trim_end_matches('/')
                .strip_suffix("/v1beta")
                .unwrap_or(config.base_url.trim_end_matches('/'));
            rig_core::providers::gemini::GeminiConfig::new(config.api_key.clone())
                .with_base_url(root)
                .connect(http)
                .completion(&config.model)
                .erase()
        }
    })
}
#[cfg(not(any(
    feature = "llm-openai",
    feature = "llm-anthropic",
    feature = "llm-gemini"
)))]
fn build_model(_: &Config) -> Result<DynModel<Completion>> {
    anyhow::bail!("no model transport enabled")
}

#[async_trait]
impl ModuleLifecycle for LlmService {
    fn name(&self) -> &'static str {
        self.name
    }
    async fn init(&mut self) -> Result<()> {
        if let Some(owner) = &mut self.lifecycle {
            owner.init().await?;
        }
        Ok(())
    }
    async fn start(&self) -> Result<()> {
        if let Some(owner) = &self.lifecycle {
            owner.start().await?;
        }
        self.stopped.store(false, Ordering::Release);
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        self.stopped.store(true, Ordering::Release);
        if let Some(owner) = &self.lifecycle {
            owner.stop().await?;
        }
        Ok(())
    }
    async fn health_check(&self) -> ModuleHealth {
        if self.stopped.load(Ordering::Acquire) {
            return ModuleHealth::Unhealthy("model service stopped".into());
        }
        let state = self.state.read().await;
        let Some(config) = state.config.clone() else {
            drop(state);
            return match &self.lifecycle {
                Some(owner) => owner.health_check().await,
                None => ModuleHealth::Healthy,
            };
        };
        let generation = state.generation;
        drop(state);
        if config.api_key.trim().is_empty() {
            return ModuleHealth::Unhealthy("model credential not configured".into());
        }
        if !config.probe {
            return ModuleHealth::Degraded("live model probe disabled".into());
        }
        let health = probe(&config).await;
        if self.stopped.load(Ordering::Acquire) {
            return ModuleHealth::Unhealthy("model service stopped".into());
        }
        if self.state.read().await.generation != generation {
            return ModuleHealth::Degraded("model configuration changed during probe".into());
        }
        health
    }
    async fn apply_settings(&self, updates: &BTreeMap<String, Value>) -> Result<Vec<String>> {
        use crate::settings::values::{f32_setting, string_setting, u64_setting};
        let mut applied = match &self.lifecycle {
            Some(owner) => owner.apply_settings(updates).await?,
            None => Vec::new(),
        };
        let mut state = self.state.write().await;
        let Some(mut config) = state.config.clone() else {
            return Ok(applied);
        };
        let owner_applied = applied.len();
        for (suffix, target) in [
            ("api_key", &mut config.api_key),
            ("base_url", &mut config.base_url),
        ] {
            let key = format!("core.providers.llm.{}.{suffix}", config.provider_id);
            if let Some(value) = string_setting(updates, &key) {
                *target = value;
                applied.push(key);
            }
        }
        let key = format!("core.profiles.llm.{}.model", self.profile_id);
        if let Some(value) = string_setting(updates, &key) {
            config.model = value;
            applied.push(key);
        }
        let key = format!("core.profiles.llm.{}.temperature", self.profile_id);
        if let Some(value) = f32_setting(updates, &key) {
            ensure!(value.is_finite(), "invalid temperature");
            config.request.temperature = value;
            applied.push(key);
        }
        let key = format!("core.profiles.llm.{}.max_output_tokens", self.profile_id);
        if let Some(value) = u64_setting(updates, &key) {
            config.request.max_output_tokens =
                u32::try_from(value).context("invalid token budget")?;
            applied.push(key);
        }
        let key = format!("core.profiles.llm.{}.top_p", self.profile_id);
        if let Some(value) = f32_setting(updates, &key) {
            ensure!((0.0..=1.0).contains(&value), "invalid top_p");
            config.request.top_p = Some(value);
            applied.push(key);
        }
        let key = format!("core.providers.llm.{}.api_family", config.provider_id);
        if let Some(value) = string_setting(updates, &key) {
            if matches!(config.api, Api::Chat | Api::Responses) {
                config.api = match value.as_str() {
                    "chat_completions" => Api::Chat,
                    "responses" => Api::Responses,
                    _ => anyhow::bail!("invalid OpenAI API family"),
                };
                applied.push(key);
            }
        }
        let key = format!(
            "core.providers.llm.{}.health_probe_enabled",
            config.provider_id
        );
        if let Some(value) = updates.get(&key).and_then(Value::as_bool) {
            config.probe = value;
            applied.push(key);
        }
        let key = format!("core.providers.llm.{}.version", config.provider_id);
        if let Api::Anthropic { version } = &mut config.api {
            if let Some(value) = string_setting(updates, &key) {
                *version = value;
                applied.push(key);
            }
        }
        if applied.len() > owner_applied {
            let model = build_model(&config)?;
            state.config = Some(config);
            state.model = Some(model);
            state.generation += 1;
        }
        Ok(applied)
    }
}

#[cfg(any(
    feature = "llm-openai",
    feature = "llm-anthropic",
    feature = "llm-gemini"
))]
async fn probe(config: &Config) -> ModuleHealth {
    let client = match http_client() {
        Ok(client) => client,
        Err(_) => return ModuleHealth::Degraded("model probe client unavailable".into()),
    };
    let endpoint = format!(
        "{}/models/{}",
        config.base_url.trim_end_matches('/'),
        config.model
    );
    let request = client.get(endpoint).timeout(Duration::from_secs(5));
    let request = match &config.api {
        Api::Chat | Api::Responses => request.bearer_auth(&config.api_key),
        Api::Anthropic { version } => request
            .header("x-api-key", &config.api_key)
            .header("anthropic-version", version),
        Api::Gemini => request.header("x-goog-api-key", &config.api_key),
    };
    match request.send().await {
        Ok(response) if response.status().is_success() => ModuleHealth::Healthy,
        Ok(_) => ModuleHealth::Degraded("model probe rejected".into()),
        Err(_) => ModuleHealth::Degraded("model probe request failed".into()),
    }
}
#[cfg(not(any(
    feature = "llm-openai",
    feature = "llm-anthropic",
    feature = "llm-gemini"
)))]
async fn probe(_: &Config) -> ModuleHealth {
    ModuleHealth::Unhealthy("no model transport enabled".into())
}

#[cfg(test)]
mod fixture;
#[cfg(test)]
mod tests;

#[cfg(all(
    test,
    any(
        feature = "llm-openai",
        feature = "llm-anthropic",
        feature = "llm-gemini"
    )
))]
mod http_tests;
