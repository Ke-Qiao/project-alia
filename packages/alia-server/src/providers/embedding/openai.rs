//! OpenAI Embedding Adapter
//!
//! Implements EmbeddingAdapter for OpenAI embedding models.
//! Supports text-embedding-3-small, text-embedding-3-large, text-embedding-ada-002.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::config::{LlmProfileKind, LlmProviderProfileRef, ServerConfig};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::EmbeddingAdapter;
use crate::settings::values::{string_setting, usize_setting};

/// OpenAI embedding request
#[derive(Debug, Serialize)]
struct EmbedRequest {
    model: String,
    input: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}

/// OpenAI embedding response
#[derive(Debug, Deserialize)]
struct EmbedResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

/// OpenAI embedding adapter
pub struct OpenAiEmbedding {
    client: Client,
    profile_id: String,
    config: Arc<RwLock<OpenAiEmbedConfig>>,
}

#[derive(Debug, Clone)]
struct OpenAiEmbedConfig {
    provider_id: String,
    api_key: String,
    base_url: String,
    model: String,
    dimension: usize,
}

impl OpenAiEmbedding {
    pub fn new() -> Self {
        Self::for_profile("embedding")
    }

    pub fn for_profile(profile_id: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            profile_id: profile_id.into(),
            config: Arc::new(RwLock::new(OpenAiEmbedConfig {
                provider_id: "openai".to_string(),
                api_key: String::new(),
                base_url: "https://api.openai.com/v1".to_string(),
                model: "text-embedding-3-small".to_string(),
                dimension: 1536,
            })),
        }
    }
}

fn openai_embedding_dimension(model: &str) -> usize {
    match model {
        "text-embedding-3-small" => 1536,
        "text-embedding-3-large" => 3072,
        "text-embedding-ada-002" => 1536,
        _ => 1536,
    }
}

impl Default for OpenAiEmbedding {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for OpenAiEmbedding {
    fn name(&self) -> &'static str {
        "openai_embedding"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let server_config: &ServerConfig = &ctx.config;
        let profile = server_config.profiles.llm_profile(&self.profile_id)?;
        if profile.kind != LlmProfileKind::Embedding {
            anyhow::bail!(
                "LLM profile '{}' is not an OpenAI embedding profile",
                self.profile_id
            );
        }
        let LlmProviderProfileRef::OpenAi(provider) =
            server_config.llm_provider_profile(&profile.provider)?
        else {
            anyhow::bail!(
                "LLM profile '{}' provider '{}' is not OpenAI-compatible",
                self.profile_id,
                profile.provider
            );
        };

        let mut config = self.config.write().await;
        config.provider_id = profile.provider.clone();
        config.api_key = provider.api_key.clone();
        config.base_url = provider.base_url.clone();
        config.model = profile.model.clone();
        config.dimension = profile
            .dimensions
            .unwrap_or_else(|| openai_embedding_dimension(&config.model));

        if config.api_key.is_empty() {
            tracing::warn!("OpenAI API key not configured for embedding");
        }

        tracing::info!(
            profile = %self.profile_id,
            provider = %config.provider_id,
            model = %config.model,
            dimension = config.dimension,
            "OpenAI Embedding initialized"
        );
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

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut applied = Vec::new();
        let mut config = self.config.write().await;

        let provider_api_key = format!("core.providers.llm.{}.api_key", config.provider_id);
        let legacy_api_key = "core.providers.openai.api_key";
        if let Some(value) = string_setting(updates, &provider_api_key).or_else(|| {
            (config.provider_id == "openai")
                .then(|| string_setting(updates, legacy_api_key))
                .flatten()
        }) {
            config.api_key = value;
            applied.push(if updates.contains_key(&provider_api_key) {
                provider_api_key
            } else {
                legacy_api_key.to_string()
            });
        }
        let provider_base_url = format!("core.providers.llm.{}.base_url", config.provider_id);
        let legacy_base_url = "core.providers.openai.base_url";
        if let Some(value) = string_setting(updates, &provider_base_url).or_else(|| {
            (config.provider_id == "openai")
                .then(|| string_setting(updates, legacy_base_url))
                .flatten()
        }) {
            config.base_url = value;
            applied.push(if updates.contains_key(&provider_base_url) {
                provider_base_url
            } else {
                legacy_base_url.to_string()
            });
        }
        let model_key = format!("core.profiles.llm.{}.model", self.profile_id);
        if let Some(value) = string_setting(updates, &model_key) {
            config.dimension = openai_embedding_dimension(&value);
            config.model = value;
            applied.push(model_key);
        }
        let dimensions_key = format!("core.profiles.llm.{}.dimensions", self.profile_id);
        if let Some(value) = usize_setting(updates, &dimensions_key) {
            config.dimension = value;
            applied.push(dimensions_key);
        }

        if !applied.is_empty() {
            tracing::info!(module = self.name(), applied = ?applied, "runtime settings applied");
        }
        Ok(applied)
    }
}

#[async_trait]
impl EmbeddingAdapter for OpenAiEmbedding {
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let config = self.config.read().await;

        let request = EmbedRequest {
            model: config.model.clone(),
            input: texts.to_vec(),
            dimensions: if config.model.starts_with("text-embedding-3") {
                Some(config.dimension)
            } else {
                None
            },
        };

        let response = self
            .client
            .post(format!("{}/embeddings", config.base_url))
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI embedding API error {}: {}", status, body);
        }

        let embed_response: EmbedResponse = response.json().await?;
        Ok(embed_response
            .data
            .into_iter()
            .map(|d| d.embedding)
            .collect())
    }

    fn dimension(&self) -> usize {
        self.config
            .try_read()
            .map(|config| config.dimension)
            .unwrap_or(1536)
    }
}
