//! mem0-backed memory client.
//!
//! The hosted mem0 API uses provider-native string identifiers and asynchronous
//! writes. We keep those details here so the rest of Alia can stay on the
//! provider-neutral `MemoryBackend` contract.

use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use async_trait::async_trait;
use reqwest::{RequestBuilder, StatusCode};
use serde_json::{Map, Value, json};
use tokio::time::sleep;
use uuid::Uuid;

use crate::config::{Mem0ProviderSection, MemorySection};
use crate::module::lifecycle::{ModuleContext, ModuleHealth};
use crate::providers::adapter::{
    MemoryCapabilities, MemoryConversationIngest, MemoryListOpts, MemoryNode, MemoryPatch,
    RetrievalOpts,
};

use super::external::ExternalMemoryClient;

const ALIA_LAYER_KEY: &str = "alia_memory_layer";
const ALIA_NODE_TYPE_KEY: &str = "alia_node_type";
const ALIA_IMPORTANCE_KEY: &str = "alia_importance";
const HEALTH_PROBE_USER_ID: Uuid = Uuid::from_u128(0x9a1a0000_0000_5000_8000_000000000001);

#[derive(Debug, Clone)]
pub struct Mem0Config {
    pub base_url: String,
    pub api_key: String,
    pub user_id_prefix: String,
    pub app_id: Option<String>,
    pub agent_id: Option<String>,
    pub run_id: Option<String>,
    pub custom_instructions: Option<String>,
    pub infer: bool,
    pub conversation_infer: bool,
    pub request_timeout: Duration,
    pub page_size: u32,
    pub add_poll_timeout: Duration,
}

impl Mem0Config {
    pub fn from_config(_policy: &MemorySection, config: &Mem0ProviderSection) -> Self {
        Self {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            user_id_prefix: config.user_id_prefix.clone(),
            app_id: config.app_id.clone(),
            agent_id: config.agent_id.clone(),
            run_id: config.run_id.clone(),
            custom_instructions: config.custom_instructions.clone(),
            infer: config.infer,
            conversation_infer: config.conversation_infer,
            request_timeout: Duration::from_secs(config.request_timeout_sec.max(1)),
            page_size: config.page_size.clamp(1, 500),
            add_poll_timeout: Duration::from_millis(config.add_poll_timeout_ms),
        }
    }
}

pub struct Mem0Client {
    config: Mem0Config,
    http: reqwest::Client,
}

impl Mem0Client {
    pub fn new(config: Mem0Config) -> Self {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { config, http }
    }

    fn base_url(&self) -> String {
        self.config.base_url.trim_end_matches('/').to_string()
    }

    fn scoped_user_id(&self, user_id: Uuid) -> String {
        format!("{}{}", self.config.user_id_prefix, user_id)
    }

    fn requires_api_key(&self) -> bool {
        self.base_url().contains("api.mem0.ai")
    }

    fn request(&self, builder: RequestBuilder) -> RequestBuilder {
        let builder = builder
            .header("accept", "application/json")
            .header("content-type", "application/json");

        if self.config.api_key.trim().is_empty() {
            builder
        } else {
            builder.header(
                "authorization",
                format!("Token {}", self.config.api_key.trim()),
            )
        }
    }

    fn ensure_ready(&self) -> anyhow::Result<()> {
        if self.requires_api_key() && self.config.api_key.trim().is_empty() {
            bail!("MEM0_API_KEY is required when using hosted mem0");
        }
        Ok(())
    }

    async fn send_json(&self, request: RequestBuilder) -> anyhow::Result<Value> {
        self.ensure_ready()?;
        let response = self
            .request(request)
            .send()
            .await
            .context("failed to call mem0 API")?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            bail!("mem0 API error {status}: {body}");
        }

        if body.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&body)
            .with_context(|| format!("failed to parse mem0 response: {body}"))
    }

    async fn send_optional_json(&self, request: RequestBuilder) -> anyhow::Result<Option<Value>> {
        self.ensure_ready()?;
        let response = self
            .request(request)
            .send()
            .await
            .context("failed to call mem0 API")?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            bail!("mem0 API error {status}: {body}");
        }
        if body.trim().is_empty() {
            return Ok(Some(Value::Null));
        }
        Ok(Some(serde_json::from_str(&body).with_context(|| {
            format!("failed to parse mem0 response: {body}")
        })?))
    }

    fn add_scope_fields(&self, body: &mut Map<String, Value>, user_id: Uuid) {
        self.add_scope_fields_with_run_id(body, user_id, None);
    }

    fn add_scope_fields_with_run_id(
        &self,
        body: &mut Map<String, Value>,
        user_id: Uuid,
        run_id: Option<&str>,
    ) {
        body.insert(
            "user_id".to_string(),
            Value::String(self.scoped_user_id(user_id)),
        );
        if let Some(app_id) = clean_optional(&self.config.app_id) {
            body.insert("app_id".to_string(), Value::String(app_id.to_string()));
        }
        if let Some(agent_id) = clean_optional(&self.config.agent_id) {
            body.insert("agent_id".to_string(), Value::String(agent_id.to_string()));
        }
        if let Some(run_id) = run_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| clean_optional(&self.config.run_id))
        {
            body.insert("run_id".to_string(), Value::String(run_id.to_string()));
        }
    }

    fn filters(&self, user_id: Uuid) -> Value {
        json!({ "user_id": self.scoped_user_id(user_id) })
    }

    async fn poll_add_event(&self, event_id: &str) -> anyhow::Result<Option<String>> {
        if event_id.is_empty() || self.config.add_poll_timeout.is_zero() {
            return Ok(None);
        }

        let deadline = Instant::now() + self.config.add_poll_timeout;
        while Instant::now() < deadline {
            let url = format!("{}/v1/event/{}/", self.base_url(), event_id);
            let event = self
                .send_optional_json(self.http.get(url))
                .await?
                .unwrap_or(Value::Null);
            if let Some(id) = extract_memory_id(&event) {
                return Ok(Some(id));
            }

            let status = event
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if matches!(
                status.as_str(),
                "failed" | "error" | "cancelled" | "canceled"
            ) {
                bail!("mem0 add event failed: {event}");
            }

            sleep(Duration::from_millis(250)).await;
        }

        Ok(None)
    }
}

#[async_trait]
impl ExternalMemoryClient for Mem0Client {
    fn name(&self) -> &'static str {
        "mem0_memory"
    }

    async fn init(&mut self, _ctx: &ModuleContext) -> anyhow::Result<()> {
        if self.requires_api_key() && self.config.api_key.trim().is_empty() {
            tracing::warn!("mem0 memory selected but MEM0_API_KEY is empty");
        } else {
            tracing::info!(base_url = %self.base_url(), "mem0 memory initialized");
        }
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if self.requires_api_key() && self.config.api_key.trim().is_empty() {
            ModuleHealth::Unhealthy("MEM0_API_KEY is missing".to_string())
        } else {
            match self
                .list(
                    HEALTH_PROBE_USER_ID,
                    MemoryListOpts {
                        max_results: Some(1),
                        layers: None,
                    },
                )
                .await
            {
                Ok(_) => ModuleHealth::Healthy,
                Err(error) => ModuleHealth::Unhealthy(format!("mem0 probe failed: {error}")),
            }
        }
    }

    async fn store(&self, user_id: Uuid, node: MemoryNode) -> anyhow::Result<String> {
        let mut metadata = object_metadata(node.metadata);
        metadata.insert(ALIA_LAYER_KEY.to_string(), Value::String(node.memory_layer));
        metadata.insert(
            ALIA_NODE_TYPE_KEY.to_string(),
            Value::String(node.node_type),
        );
        metadata.insert(ALIA_IMPORTANCE_KEY.to_string(), json!(node.importance));

        let mut body = Map::new();
        self.add_scope_fields(&mut body, user_id);
        body.insert(
            "messages".to_string(),
            json!([{ "role": "user", "content": node.content }]),
        );
        body.insert("metadata".to_string(), Value::Object(metadata));
        body.insert("infer".to_string(), Value::Bool(self.config.infer));
        if let Some(custom_instructions) = clean_optional(&self.config.custom_instructions) {
            body.insert(
                "custom_instructions".to_string(),
                Value::String(custom_instructions.to_string()),
            );
        }

        let url = format!("{}/v3/memories/add/", self.base_url());
        let response = self.send_json(self.http.post(url).json(&body)).await?;
        if let Some(id) = extract_memory_id(&response) {
            return Ok(id);
        }
        let event_id = extract_string_field(&response, &["event_id", "id", "task_id"])
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        if let Some(memory_id) = self.poll_add_event(&event_id).await? {
            return Ok(memory_id);
        }
        Ok(event_id)
    }

    async fn list(&self, user_id: Uuid, opts: MemoryListOpts) -> anyhow::Result<Vec<MemoryNode>> {
        let limit = opts
            .max_results
            .unwrap_or(self.config.page_size)
            .clamp(1, self.config.page_size.max(1));
        let url = format!("{}/v3/memories/", self.base_url());
        let body = json!({ "filters": self.filters(user_id) });
        let response = self
            .send_json(
                self.http
                    .post(url)
                    .query(&[("page", 1_u32), ("page_size", limit)])
                    .json(&body),
            )
            .await?;

        Ok(limit_and_filter(
            parse_memory_nodes(&response),
            opts.layers,
            limit,
        ))
    }

    async fn get(&self, _user_id: Uuid, node_id: &str) -> anyhow::Result<Option<MemoryNode>> {
        let url = format!("{}/v1/memories/{}", self.base_url(), node_id);
        Ok(self
            .send_optional_json(self.http.get(url))
            .await?
            .and_then(|value| parse_memory_node(&value)))
    }

    async fn retrieve(
        &self,
        user_id: Uuid,
        query: &str,
        opts: RetrievalOpts,
    ) -> anyhow::Result<Vec<MemoryNode>> {
        let limit = opts.max_results.unwrap_or(10).clamp(1, 50);
        let mut body = Map::new();
        body.insert("query".to_string(), Value::String(query.to_string()));
        body.insert("filters".to_string(), self.filters(user_id));
        body.insert("top_k".to_string(), json!(limit));
        body.insert("limit".to_string(), json!(limit));
        if let Some(min_similarity) = opts.min_similarity {
            body.insert("threshold".to_string(), json!(min_similarity));
        }

        let url = format!("{}/v3/memories/search/", self.base_url());
        let response = self
            .send_json(self.http.post(url).json(&Value::Object(body)))
            .await?;
        Ok(limit_and_filter(
            parse_memory_nodes(&response),
            opts.layers,
            limit,
        ))
    }

    async fn update(&self, node_id: &str, patch: MemoryPatch) -> anyhow::Result<()> {
        let mut body = Map::new();
        if let Some(content) = patch.content {
            body.insert("text".to_string(), Value::String(content));
        }
        if let Some(metadata) = patch.metadata {
            body.insert(
                "metadata".to_string(),
                Value::Object(object_metadata(metadata)),
            );
        }
        if let Some(importance) = patch.importance {
            let metadata = body
                .entry("metadata".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(object) = metadata {
                object.insert(ALIA_IMPORTANCE_KEY.to_string(), json!(importance));
            }
        }
        if body.is_empty() {
            return Ok(());
        }

        let url = format!("{}/v1/memories/{}", self.base_url(), node_id);
        self.send_json(self.http.put(url).json(&Value::Object(body)))
            .await?;
        Ok(())
    }

    async fn delete(&self, node_id: &str) -> anyhow::Result<()> {
        let url = format!("{}/v1/memories/{}", self.base_url(), node_id);
        self.send_optional_json(self.http.delete(url)).await?;
        Ok(())
    }

    async fn clear(&self, user_id: Uuid) -> anyhow::Result<u64> {
        let nodes = self
            .list(
                user_id,
                MemoryListOpts {
                    max_results: Some(self.config.page_size),
                    layers: None,
                },
            )
            .await?;
        let mut deleted = 0;
        for node in nodes {
            if let Some(id) = node.id {
                self.delete(&id).await?;
                deleted += 1;
            }
        }
        Ok(deleted)
    }

    async fn ingest_conversation(
        &self,
        user_id: Uuid,
        ingest: MemoryConversationIngest,
    ) -> anyhow::Result<Option<String>> {
        let messages = ingest
            .messages
            .into_iter()
            .filter(|message| !message.content.trim().is_empty())
            .map(|message| {
                let mut item = Map::new();
                item.insert("role".to_string(), Value::String(message.role));
                item.insert("content".to_string(), Value::String(message.content));
                if let Some(name) = message.name
                    && !name.trim().is_empty()
                {
                    item.insert("name".to_string(), Value::String(name));
                }
                Value::Object(item)
            })
            .collect::<Vec<_>>();

        if messages.is_empty() {
            return Ok(None);
        }

        let mut body = Map::new();
        self.add_scope_fields_with_run_id(&mut body, user_id, ingest.run_id.as_deref());
        body.insert("messages".to_string(), Value::Array(messages));
        body.insert(
            "infer".to_string(),
            Value::Bool(self.config.conversation_infer),
        );
        body.insert(
            "metadata".to_string(),
            Value::Object(object_metadata(ingest.metadata)),
        );
        if let Some(custom_instructions) = clean_optional(&self.config.custom_instructions) {
            body.insert(
                "custom_instructions".to_string(),
                Value::String(custom_instructions.to_string()),
            );
        }

        let url = format!("{}/v3/memories/add/", self.base_url());
        let response = self
            .send_json(self.http.post(url).json(&Value::Object(body)))
            .await?;
        let event_id = extract_memory_id(&response)
            .or_else(|| extract_string_field(&response, &["event_id", "id", "task_id"]));
        if let Some(event_id) = event_id.as_deref()
            && let Some(memory_id) = self.poll_add_event(event_id).await?
        {
            return Ok(Some(memory_id));
        }
        Ok(event_id)
    }

    async fn memory_capabilities(&self) -> MemoryCapabilities {
        let mut capabilities = MemoryCapabilities::minimal(self.name());
        capabilities.supports_conversation_ingest = true;
        capabilities.supports_provider_infer = self.config.infer || self.config.conversation_infer;
        capabilities.supports_search_filters = true;
        capabilities.supports_layer_filters = true;
        capabilities.supports_similarity_threshold = true;
        capabilities.supports_graph = false;
        capabilities.supports_relationship_edges = false;
        capabilities.async_writes = !self.config.add_poll_timeout.is_zero();
        capabilities.max_content_chars = Some(16_000);
        capabilities.max_metadata_bytes = Some(64 * 1024);
        capabilities.max_batch_messages = Some(64);
        capabilities
            .provider_specific
            .insert("api_family".to_string(), json!("mem0_v1_v3"));
        capabilities
            .provider_specific
            .insert("store_infer_enabled".to_string(), json!(self.config.infer));
        capabilities.provider_specific.insert(
            "conversation_infer_enabled".to_string(),
            json!(self.config.conversation_infer),
        );
        capabilities.provider_specific.insert(
            "delete_by_user_implemented_as_list_delete".to_string(),
            json!(true),
        );
        capabilities
    }
}

fn clean_optional(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn object_metadata(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object,
        Value::Null => Map::new(),
        other => {
            let mut object = Map::new();
            object.insert("value".to_string(), other);
            object
        }
    }
}

fn limit_and_filter(
    mut nodes: Vec<MemoryNode>,
    layers: Option<Vec<String>>,
    limit: u32,
) -> Vec<MemoryNode> {
    if let Some(layers) = layers {
        nodes.retain(|node| layers.contains(&node.memory_layer));
    }
    nodes.truncate(limit as usize);
    nodes
}

pub fn parse_memory_nodes(value: &Value) -> Vec<MemoryNode> {
    candidate_arrays(value)
        .into_iter()
        .flat_map(|items| items.iter().filter_map(parse_memory_node))
        .collect()
}

fn candidate_arrays(value: &Value) -> Vec<&Vec<Value>> {
    let mut arrays = Vec::new();
    if let Some(array) = value.as_array() {
        arrays.push(array);
    }
    for key in ["results", "memories", "items", "data"] {
        if let Some(array) = value.get(key).and_then(Value::as_array) {
            arrays.push(array);
        }
    }
    arrays
}

fn parse_memory_node(value: &Value) -> Option<MemoryNode> {
    let metadata = value
        .get("metadata")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let content = extract_string_field(value, &["memory", "content", "text", "value"])?;
    let layer = metadata
        .get(ALIA_LAYER_KEY)
        .or_else(|| metadata.get("memory_layer"))
        .and_then(Value::as_str)
        .unwrap_or("core")
        .to_string();
    let node_type = metadata
        .get(ALIA_NODE_TYPE_KEY)
        .or_else(|| metadata.get("node_type"))
        .and_then(Value::as_str)
        .unwrap_or("fact")
        .to_string();
    let importance = metadata
        .get(ALIA_IMPORTANCE_KEY)
        .or_else(|| value.get("importance"))
        .or_else(|| value.get("score"))
        .and_then(Value::as_f64)
        .unwrap_or(0.5)
        .clamp(0.0, 1.0) as f32;

    Some(MemoryNode {
        id: extract_memory_id(value),
        memory_layer: layer,
        node_type,
        content,
        metadata,
        importance,
    })
}

pub fn extract_memory_id(value: &Value) -> Option<String> {
    extract_string_field(value, &["memory_id", "id", "uuid"])
        .or_else(|| value.get("memory").and_then(extract_memory_id))
        .or_else(|| value.get("result").and_then(extract_memory_id))
        .or_else(|| {
            candidate_arrays(value)
                .into_iter()
                .flat_map(|items| items.iter())
                .find_map(extract_memory_id)
        })
}

fn extract_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            if !text.trim().is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}
