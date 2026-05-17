//! Ollama Embedding Adapter
//!
//! Implements EmbeddingAdapter for local Ollama embedding models.
//! Supports models like nomic-embed-text, mxbai-embed-large, etc.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::config::ServerConfig;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::EmbeddingAdapter;
use crate::settings::values::string_setting;

/// Ollama embedding request
#[derive(Debug, Serialize)]
struct EmbedRequest {
    model: String,
    input: Vec<String>,
}

/// Ollama embedding response
#[derive(Debug, Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

/// Ollama embedding adapter
pub struct OllamaEmbedding {
    client: Client,
    config: Arc<RwLock<OllamaEmbedConfig>>,
}

#[derive(Debug, Clone)]
struct OllamaEmbedConfig {
    endpoint: String,
    model: String,
    dimension: usize,
}

impl OllamaEmbedding {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            config: Arc::new(RwLock::new(OllamaEmbedConfig {
                endpoint: "http://localhost:11434".to_string(),
                model: "nomic-embed-text".to_string(),
                dimension: 768, // nomic-embed-text default
            })),
        }
    }
}

fn ollama_embedding_dimension(model: &str) -> usize {
    match model {
        "nomic-embed-text" => 768,
        "mxbai-embed-large" => 1024,
        "snowflake-arctic-embed" => 1024,
        "all-minilm" => 384,
        "bge-m3" => 1024,
        _ => 768,
    }
}

impl Default for OllamaEmbedding {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for OllamaEmbedding {
    fn name(&self) -> &'static str {
        "ollama_embedding"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let server_config: &ServerConfig = &ctx.config;

        let mut config = self.config.write().await;
        config.endpoint = server_config.providers.ollama.endpoint.clone();
        config.model = server_config.providers.ollama.embedding_model.clone();

        // Set dimension based on model
        config.dimension = ollama_embedding_dimension(&config.model);

        tracing::info!(model = %config.model, endpoint = %config.endpoint, dimension = config.dimension, "Ollama Embedding initialized");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let config = self.config.read().await;

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

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut applied = Vec::new();
        let mut config = self.config.write().await;

        if let Some(value) = string_setting(updates, "core.providers.ollama.endpoint") {
            config.endpoint = value;
            applied.push("core.providers.ollama.endpoint".to_string());
        }
        if let Some(value) = string_setting(updates, "core.providers.ollama.embedding_model") {
            config.dimension = ollama_embedding_dimension(&value);
            config.model = value;
            applied.push("core.providers.ollama.embedding_model".to_string());
        }

        if !applied.is_empty() {
            tracing::info!(module = self.name(), applied = ?applied, "runtime settings applied");
        }
        Ok(applied)
    }
}

#[async_trait]
impl EmbeddingAdapter for OllamaEmbedding {
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let config = self.config.read().await;

        let request = EmbedRequest {
            model: config.model.clone(),
            input: texts.to_vec(),
        };

        let response = self
            .client
            .post(format!("{}/api/embed", config.endpoint))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Ollama embedding API error {}: {}", status, body);
        }

        let embed_response: EmbedResponse = response.json().await?;
        Ok(embed_response.embeddings)
    }

    fn dimension(&self) -> usize {
        // Use a runtime default, actual value is set during init
        768
    }
}
