use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::HomeAssistantPluginSection;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::plugins::{
    CompiledToolPlugin, PluginStorage, PluginStorageCollection, PluginStorageManifest,
    PluginStorageRetention, PluginStorageScopeKind, PluginToolContext, StorageScope,
};
use crate::providers::adapter::{ToolContext, ToolDefinition, ToolExecutor, ToolResult};

const PLUGIN_NAME: &str = "home_assistant";
const LIST_LIGHTS_TOOL: &str = "home_assistant_list_lights";
const CONTROL_LIGHT_TOOL: &str = "home_assistant_light_control";
const CONFIG_COLLECTION: &str = "config";
const ENTITY_CACHE_COLLECTION: &str = "entity_cache";
const CONNECTION_CONFIG_KEY: &str = "connection";
const LIGHT_CACHE_KEY: &str = "lights";
const LIGHT_CACHE_SCHEMA: &str = "alia.home_assistant.light_cache.v1";
const CONFIG_SCHEMA: &str = "alia.home_assistant.config.v1";
const DEFAULT_CACHE_TTL_SEC: u64 = 300;

#[derive(Default)]
pub struct HomeAssistantPlugin {
    runtime: Option<HomeAssistantRuntimeConfig>,
}

impl HomeAssistantPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    fn manifest(cache_ttl_sec: u64) -> PluginStorageManifest {
        PluginStorageManifest {
            collections: vec![
                PluginStorageCollection {
                    name: CONFIG_COLLECTION.to_string(),
                    scope: vec![PluginStorageScopeKind::Global],
                    schema_version: 1,
                    retention: PluginStorageRetention::RetainOnUninstall,
                    ttl_sec: None,
                    max_record_bytes: Some(16 * 1024),
                },
                PluginStorageCollection {
                    name: ENTITY_CACHE_COLLECTION.to_string(),
                    scope: vec![PluginStorageScopeKind::Global],
                    schema_version: 1,
                    retention: PluginStorageRetention::DeleteOnUninstall,
                    ttl_sec: Some(cache_ttl_sec.max(1)),
                    max_record_bytes: Some(256 * 1024),
                },
            ],
        }
    }
}

#[async_trait]
impl ModuleLifecycle for HomeAssistantPlugin {
    fn name(&self) -> &'static str {
        "home_assistant_plugin"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let runtime = HomeAssistantRuntimeConfig::from_config(&ctx.config.plugins.home_assistant)?;
        self.runtime = Some(runtime);
        let plugin_ctx = ctx.plugin_tool_context(
            PLUGIN_NAME,
            <Self as CompiledToolPlugin>::storage_manifest(self),
        )?;
        <Self as CompiledToolPlugin>::register(self, plugin_ctx).await
    }

    async fn health_check(&self) -> ModuleHealth {
        match self.runtime.as_ref() {
            Some(runtime) if runtime.access_token.is_empty() => ModuleHealth::Degraded(
                "Home Assistant token is not configured; tools will return auth errors".to_string(),
            ),
            Some(_) => ModuleHealth::Healthy,
            None => ModuleHealth::Unhealthy("not initialized".to_string()),
        }
    }
}

#[async_trait]
impl CompiledToolPlugin for HomeAssistantPlugin {
    fn name(&self) -> &'static str {
        PLUGIN_NAME
    }

    fn storage_manifest(&self) -> PluginStorageManifest {
        let cache_ttl_sec = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.cache_ttl_sec)
            .unwrap_or(DEFAULT_CACHE_TTL_SEC);
        Self::manifest(cache_ttl_sec)
    }

    async fn register(&self, ctx: PluginToolContext) -> anyhow::Result<()> {
        let runtime = self
            .runtime
            .as_ref()
            .context("home_assistant plugin runtime config is not initialized")?
            .clone();
        persist_config_snapshot(ctx.storage.as_ref(), &runtime).await?;

        let client = Arc::new(HomeAssistantClient::new(ctx.storage.clone(), runtime)?);
        ctx.tools
            .register_tool_executor(
                PLUGIN_NAME,
                Arc::new(HomeAssistantListLightsTool::new(client.clone())),
            )
            .await?;
        ctx.tools
            .register_tool_executor(
                PLUGIN_NAME,
                Arc::new(HomeAssistantLightControlTool::new(client)),
            )
            .await?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct HomeAssistantRuntimeConfig {
    endpoint: String,
    access_token: String,
    cache_ttl_sec: u64,
    request_timeout_sec: u64,
}

impl HomeAssistantRuntimeConfig {
    fn from_config(config: &HomeAssistantPluginSection) -> anyhow::Result<Self> {
        let endpoint = env_value("HOME_ASSISTANT_ENDPOINT")
            .unwrap_or_else(|| config.endpoint.trim().to_string());
        let endpoint = normalize_endpoint(&endpoint)?;
        let access_token =
            env_value(config.token_env.as_str()).unwrap_or_else(|| config.access_token.clone());
        Ok(Self {
            endpoint,
            access_token: access_token.trim().to_string(),
            cache_ttl_sec: config.cache_ttl_sec.max(1),
            request_timeout_sec: config.request_timeout_sec.clamp(1, 60),
        })
    }
}

fn env_value(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_endpoint(endpoint: &str) -> anyhow::Result<String> {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.is_empty() {
        anyhow::bail!("plugins.home_assistant.endpoint is required when the plugin is enabled");
    }
    let url = reqwest::Url::parse(endpoint)
        .map_err(|error| anyhow::anyhow!("invalid Home Assistant endpoint: {error}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("Home Assistant endpoint must not contain credentials");
    }
    match url.scheme() {
        "http" | "https" => Ok(endpoint.to_string()),
        scheme => anyhow::bail!(
            "unsupported Home Assistant endpoint scheme '{scheme}'; use http or https"
        ),
    }
}

async fn persist_config_snapshot(
    storage: &dyn PluginStorage,
    runtime: &HomeAssistantRuntimeConfig,
) -> anyhow::Result<()> {
    storage
        .put(
            CONFIG_COLLECTION,
            CONNECTION_CONFIG_KEY,
            StorageScope::Global,
            serde_json::json!({
                "schema": CONFIG_SCHEMA,
                "endpoint": runtime.endpoint,
                "token_configured": !runtime.access_token.is_empty(),
                "cache_ttl_sec": runtime.cache_ttl_sec,
                "request_timeout_sec": runtime.request_timeout_sec,
                "updated_at": Utc::now().to_rfc3339(),
            }),
        )
        .await?;
    Ok(())
}

#[derive(Clone)]
struct HomeAssistantClient {
    execution_revision: String,
    endpoint: String,
    access_token: String,
    http: reqwest::Client,
    storage: Arc<dyn PluginStorage>,
}

impl HomeAssistantClient {
    fn new(
        storage: Arc<dyn PluginStorage>,
        runtime: HomeAssistantRuntimeConfig,
    ) -> anyhow::Result<Self> {
        // Bind authorization to the actual immutable endpoint and credentials,
        // including environment overrides. Only this digest enters the ledger.
        let execution_revision = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&serde_json::json!({
                "implementation": "alia.compiled.home_assistant",
                "endpoint": runtime.endpoint,
                "access_token": runtime.access_token,
                "cache_ttl_sec": runtime.cache_ttl_sec,
                "request_timeout_sec": runtime.request_timeout_sec,
            }))?)
        );
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(runtime.request_timeout_sec))
            .user_agent("alia-server home_assistant plugin/0.1")
            .build()?;
        Ok(Self {
            execution_revision,
            endpoint: runtime.endpoint,
            access_token: runtime.access_token,
            http,
            storage,
        })
    }

    fn execution_revision(&self) -> String {
        self.execution_revision.clone()
    }

    async fn lights(&self, refresh_cache: bool) -> anyhow::Result<CachedLights> {
        if !refresh_cache && let Some(lights) = self.cached_lights().await? {
            return Ok(CachedLights {
                lights,
                from_cache: true,
            });
        }

        let lights = self.fetch_lights().await?;
        self.store_lights(&lights).await?;
        Ok(CachedLights {
            lights,
            from_cache: false,
        })
    }

    async fn cached_lights(&self) -> anyhow::Result<Option<Vec<HaLight>>> {
        let Some(record) = self
            .storage
            .get(
                ENTITY_CACHE_COLLECTION,
                LIGHT_CACHE_KEY,
                StorageScope::Global,
            )
            .await?
        else {
            return Ok(None);
        };
        let parsed = serde_json::from_value::<LightCacheDocument>(record.value).ok();
        Ok(parsed
            .filter(|doc| doc.schema == LIGHT_CACHE_SCHEMA)
            .map(|doc| doc.lights))
    }

    async fn store_lights(&self, lights: &[HaLight]) -> anyhow::Result<()> {
        self.storage
            .put(
                ENTITY_CACHE_COLLECTION,
                LIGHT_CACHE_KEY,
                StorageScope::Global,
                serde_json::to_value(LightCacheDocument {
                    schema: LIGHT_CACHE_SCHEMA.to_string(),
                    cached_at: Utc::now().to_rfc3339(),
                    lights: lights.to_vec(),
                })?,
            )
            .await?;
        Ok(())
    }

    async fn fetch_lights(&self) -> anyhow::Result<Vec<HaLight>> {
        let text = self
            .send(self.http.get(self.api_url("/api/states")?))
            .await
            .context("failed to list Home Assistant states")?;
        let states = serde_json::from_str::<Vec<HaState>>(&text)
            .map_err(|error| anyhow::anyhow!("invalid Home Assistant states response: {error}"))?;
        let mut lights = states
            .into_iter()
            .filter(|state| state.entity_id.starts_with("light."))
            .map(HaLight::from)
            .collect::<Vec<_>>();
        lights.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.entity_id.cmp(&right.entity_id))
        });
        Ok(lights)
    }

    async fn set_light_state(
        &self,
        entity_id: &str,
        state: LightTargetState,
    ) -> anyhow::Result<()> {
        let service = match state {
            LightTargetState::On => "turn_on",
            LightTargetState::Off => "turn_off",
        };
        self.send(
            self.http
                .post(self.api_url(&format!("/api/services/light/{service}"))?)
                .json(&serde_json::json!({ "entity_id": entity_id })),
        )
        .await
        .map(|_| ())
        .with_context(|| format!("failed to call Home Assistant light/{service}"))
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> anyhow::Result<String> {
        if self.access_token.is_empty() {
            anyhow::bail!(
                "Home Assistant access token is not configured; set plugins.home_assistant.access_token or HOME_ASSISTANT_TOKEN"
            );
        }
        let response = request.bearer_auth(&self.access_token).send().await?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let detail = text.trim();
            if detail.is_empty() {
                anyhow::bail!("Home Assistant API returned HTTP {status}");
            }
            anyhow::bail!("Home Assistant API returned HTTP {status}: {detail}");
        }
        Ok(text)
    }

    fn api_url(&self, path: &str) -> anyhow::Result<String> {
        let path = path.trim_start_matches('/');
        Ok(reqwest::Url::parse(&format!("{}/{path}", self.endpoint))?.to_string())
    }
}

#[derive(Debug, Clone)]
struct CachedLights {
    lights: Vec<HaLight>,
    from_cache: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct HaLight {
    entity_id: String,
    name: String,
    state: String,
}

impl From<HaState> for HaLight {
    fn from(value: HaState) -> Self {
        let name = value
            .attributes
            .friendly_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| fallback_light_name(&value.entity_id));
        Self {
            entity_id: value.entity_id,
            name,
            state: value.state,
        }
    }
}

#[derive(Debug, Deserialize)]
struct HaState {
    entity_id: String,
    state: String,
    #[serde(default)]
    attributes: HaAttributes,
}

#[derive(Debug, Default, Deserialize)]
struct HaAttributes {
    friendly_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LightCacheDocument {
    schema: String,
    cached_at: String,
    lights: Vec<HaLight>,
}

struct HomeAssistantListLightsTool {
    client: Arc<HomeAssistantClient>,
}

impl HomeAssistantListLightsTool {
    fn new(client: Arc<HomeAssistantClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolExecutor for HomeAssistantListLightsTool {
    fn implementation_revision(&self) -> Option<String> {
        Some(self.client.execution_revision())
    }
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: LIST_LIGHTS_TOOL.to_string(),
            description: "List Home Assistant light entities and cache them in plugin storage."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "refresh_cache": {
                        "type": "boolean",
                        "description": "When true, bypass the cached light list and refresh from Home Assistant."
                    }
                }
            }),
            routing_hints: vec![
                "home assistant".to_string(),
                "lights".to_string(),
                "list lights".to_string(),
                "available lights".to_string(),
                "smart home".to_string(),
                "灯".to_string(),
                "灯光".to_string(),
                "列出灯".to_string(),
                "有哪些灯".to_string(),
                "家居".to_string(),
            ],
            source: PLUGIN_NAME.to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let refresh_cache = arguments
            .get("refresh_cache")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        match self.client.lights(refresh_cache).await {
            Ok(cached) => Ok(ToolResult {
                call_id: String::new(),
                success: true,
                output: serde_json::json!({
                    "lights": cached.lights,
                    "count": cached.lights.len(),
                    "from_cache": cached.from_cache,
                }),
                display: Some(format!(
                    "Found {} Home Assistant light{}{}",
                    cached.lights.len(),
                    if cached.lights.len() == 1 { "" } else { "s" },
                    if cached.from_cache { " from cache" } else { "" }
                )),
            }),
            Err(error) => Ok(tool_error("home_assistant_list_failed", error)),
        }
    }
}

struct HomeAssistantLightControlTool {
    client: Arc<HomeAssistantClient>,
}

impl HomeAssistantLightControlTool {
    fn new(client: Arc<HomeAssistantClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolExecutor for HomeAssistantLightControlTool {
    fn implementation_revision(&self) -> Option<String> {
        Some(self.client.execution_revision())
    }
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: CONTROL_LIGHT_TOOL.to_string(),
            description: "Turn a Home Assistant light on or off by entity_id or friendly name."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "state": {
                        "type": "string",
                        "enum": ["on", "off"],
                        "description": "Target light state."
                    },
                    "entity_id": {
                        "type": "string",
                        "description": "Exact Home Assistant light entity id, for example light.living_room."
                    },
                    "name": {
                        "type": "string",
                        "description": "Friendly light name when entity_id is unknown, for example Living Room Lamp."
                    },
                    "refresh_cache": {
                        "type": "boolean",
                        "description": "When true, refresh Home Assistant light entities before matching."
                    }
                },
                "required": ["state"]
            }),
            routing_hints: vec![
                "home assistant".to_string(),
                "light".to_string(),
                "lights".to_string(),
                "turn on light".to_string(),
                "turn off light".to_string(),
                "lamp".to_string(),
                "smart home".to_string(),
                "灯".to_string(),
                "灯光".to_string(),
                "开灯".to_string(),
                "关灯".to_string(),
                "打开灯".to_string(),
                "关闭灯".to_string(),
                "照明".to_string(),
                "家居".to_string(),
            ],
            source: PLUGIN_NAME.to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let state = match LightTargetState::from_arguments(&arguments) {
            Ok(state) => state,
            Err(error) => return Ok(tool_error("invalid_light_state", error)),
        };
        let refresh_cache = arguments
            .get("refresh_cache")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let cached = match self.client.lights(refresh_cache).await {
            Ok(cached) => cached,
            Err(error) => return Ok(tool_error("home_assistant_list_failed", error)),
        };

        let light = match resolve_light(&cached.lights, &arguments) {
            Ok(light) => light,
            Err(LightResolveError::NotFound(_)) if cached.from_cache => {
                match self.client.lights(true).await {
                    Ok(refreshed) => match resolve_light(&refreshed.lights, &arguments) {
                        Ok(light) => light,
                        Err(error) => return Ok(resolve_error_result(error, refreshed.lights)),
                    },
                    Err(error) => return Ok(tool_error("home_assistant_refresh_failed", error)),
                }
            }
            Err(error) => return Ok(resolve_error_result(error, cached.lights)),
        };

        if let Err(error) = self.client.set_light_state(&light.entity_id, state).await {
            return Ok(tool_error("home_assistant_control_failed", error));
        }

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "entity_id": light.entity_id,
                "name": light.name,
                "state": state.as_str(),
            }),
            display: Some(format!("Turned {} {}", state.display_word(), light.name)),
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum LightTargetState {
    On,
    Off,
}

impl LightTargetState {
    fn from_arguments(arguments: &serde_json::Value) -> anyhow::Result<Self> {
        match arguments
            .get("state")
            .and_then(serde_json::Value::as_str)
            .map(|state| state.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("on") | Some("turn_on") | Some("open") => Ok(Self::On),
            Some("off") | Some("turn_off") | Some("close") => Ok(Self::Off),
            Some(other) => anyhow::bail!("unsupported light state '{other}'; use 'on' or 'off'"),
            None => anyhow::bail!("state is required"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
        }
    }

    fn display_word(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

#[derive(Debug)]
enum LightResolveError {
    MissingTarget,
    NotFound(String),
    Ambiguous(Vec<HaLight>),
}

fn resolve_light(
    lights: &[HaLight],
    arguments: &serde_json::Value,
) -> Result<HaLight, LightResolveError> {
    let target = arguments
        .get("entity_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| arguments.get("name").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let Some(target) = target else {
        return match lights {
            [only] => Ok(only.clone()),
            _ => Err(LightResolveError::MissingTarget),
        };
    };

    if let Some(exact) = lights.iter().find(|light| light.entity_id == target) {
        return Ok(exact.clone());
    }

    let target_key = normalized_match_key(target);
    let exact = lights
        .iter()
        .filter(|light| light.matches_exact_key(&target_key))
        .cloned()
        .collect::<Vec<_>>();
    if exact.len() == 1 {
        return Ok(exact[0].clone());
    }
    if exact.len() > 1 {
        return Err(LightResolveError::Ambiguous(exact));
    }

    let contains = lights
        .iter()
        .filter(|light| light.matches_partial_key(&target_key))
        .cloned()
        .collect::<Vec<_>>();
    match contains.len() {
        1 => Ok(contains[0].clone()),
        0 => Err(LightResolveError::NotFound(target.to_string())),
        _ => Err(LightResolveError::Ambiguous(contains)),
    }
}

impl HaLight {
    fn matches_exact_key(&self, target_key: &str) -> bool {
        normalized_match_key(&self.name) == target_key
            || normalized_match_key(object_id(&self.entity_id)) == target_key
            || normalized_match_key(&self.entity_id) == target_key
    }

    fn matches_partial_key(&self, target_key: &str) -> bool {
        if target_key.is_empty() {
            return false;
        }
        normalized_match_key(&self.name).contains(target_key)
            || normalized_match_key(object_id(&self.entity_id)).contains(target_key)
            || normalized_match_key(&self.entity_id).contains(target_key)
    }
}

fn resolve_error_result(error: LightResolveError, lights: Vec<HaLight>) -> ToolResult {
    match error {
        LightResolveError::MissingTarget => ToolResult {
            call_id: String::new(),
            success: false,
            output: serde_json::json!({
                "error": "light_target_required",
                "available_lights": lights,
            }),
            display: Some("Provide a Home Assistant light entity_id or name".to_string()),
        },
        LightResolveError::NotFound(target) => ToolResult {
            call_id: String::new(),
            success: false,
            output: serde_json::json!({
                "error": "light_not_found",
                "target": target,
                "available_lights": lights,
            }),
            display: Some(format!("No Home Assistant light matched '{target}'")),
        },
        LightResolveError::Ambiguous(matches) => ToolResult {
            call_id: String::new(),
            success: false,
            output: serde_json::json!({
                "error": "light_match_ambiguous",
                "matches": matches,
            }),
            display: Some("Multiple Home Assistant lights matched; provide entity_id".to_string()),
        },
    }
}

fn tool_error(code: &str, error: anyhow::Error) -> ToolResult {
    ToolResult {
        call_id: String::new(),
        success: false,
        output: serde_json::json!({
            "error": code,
            "message": error.to_string(),
        }),
        display: Some(format!("Home Assistant tool failed: {error}")),
    }
}

fn fallback_light_name(entity_id: &str) -> String {
    object_id(entity_id).replace('_', " ")
}

fn object_id(entity_id: &str) -> &str {
    entity_id
        .rsplit_once('.')
        .map(|(_, id)| id)
        .unwrap_or(entity_id)
}

fn normalized_match_key(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(entity_id: &str, name: &str) -> HaLight {
        HaLight {
            entity_id: entity_id.to_string(),
            name: name.to_string(),
            state: "off".to_string(),
        }
    }

    #[test]
    fn endpoint_normalization_requires_http_url() {
        assert_eq!(
            normalize_endpoint("http://homeassistant.local:8123/").unwrap(),
            "http://homeassistant.local:8123"
        );
        assert!(normalize_endpoint("").is_err());
        assert!(normalize_endpoint("file:///tmp/ha").is_err());
        assert!(normalize_endpoint("http://user:pass@homeassistant.local:8123").is_err());
    }

    #[test]
    fn light_resolution_accepts_entity_id_and_friendly_name() {
        let lights = vec![
            light("light.living_room_lamp", "Living Room Lamp"),
            light("light.bedroom", "Bedroom"),
        ];

        let by_entity =
            resolve_light(&lights, &serde_json::json!({"entity_id": "light.bedroom"})).unwrap();
        assert_eq!(by_entity.entity_id, "light.bedroom");

        let by_name = resolve_light(&lights, &serde_json::json!({"name": "living room"})).unwrap();
        assert_eq!(by_name.entity_id, "light.living_room_lamp");
    }

    #[test]
    fn light_resolution_reports_ambiguity() {
        let lights = vec![
            light("light.living_room_lamp", "Living Room Lamp"),
            light("light.living_room_strip", "Living Room Strip"),
        ];

        let error =
            resolve_light(&lights, &serde_json::json!({"name": "living room"})).unwrap_err();
        assert!(matches!(error, LightResolveError::Ambiguous(matches) if matches.len() == 2));
    }

    #[test]
    fn cache_document_round_trips() {
        let doc = LightCacheDocument {
            schema: LIGHT_CACHE_SCHEMA.to_string(),
            cached_at: "now".to_string(),
            lights: vec![light("light.kitchen", "Kitchen")],
        };
        let value = serde_json::to_value(&doc).unwrap();
        let parsed: LightCacheDocument = serde_json::from_value(value).unwrap();

        assert_eq!(parsed.schema, LIGHT_CACHE_SCHEMA);
        assert_eq!(parsed.lights[0].entity_id, "light.kitchen");
    }
}
