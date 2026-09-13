//! OpenAI Embedding Adapter
//!
//! Implements EmbeddingAdapter for OpenAI embedding models.
//! Supports text-embedding-3-small, text-embedding-3-large, text-embedding-ada-002.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::config::{LlmProfileKind, LlmProviderProfileRef, ServerConfig};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    EmbeddingAdapter, EmbeddingProfile, EmbeddingVector, ProfiledEmbeddings,
    embedding_model_dimension, embedding_text_digest,
};

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
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    index: usize,
    embedding: Vec<f32>,
}

/// OpenAI embedding adapter
pub struct OpenAiEmbedding {
    client: Client,
    request_timeout: Duration,
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
            request_timeout: Duration::from_secs(30),
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

impl OpenAiEmbedding {
    fn profile(config: &OpenAiEmbedConfig) -> anyhow::Result<EmbeddingProfile> {
        let endpoint = config.base_url.trim_end_matches('/');
        let url = reqwest::Url::parse(endpoint)?;
        anyhow::ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && matches!(url.scheme(), "http" | "https"),
            "invalid embedding endpoint identity"
        );
        let profile = EmbeddingProfile {
            provider: config.provider_id.clone(),
            endpoint: endpoint.to_string(),
            model: config.model.clone(),
            revision: "request-config.v1".into(),
            dimensions: config.dimension,
            distance: "cosine".into(),
            vector_type: "vector".into(),
        };
        profile.validate()?;
        Ok(profile)
    }

    fn update_config(
        &self,
        config: &mut OpenAiEmbedConfig,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut applied = Vec::new();
        let api_key = format!("core.providers.llm.{}.api_key", config.provider_id);
        if let Some(value) = updates.get(&api_key) {
            config.api_key = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{api_key} must be a string"))?
                .to_owned();
            applied.push(api_key);
        }
        let base_url = format!("core.providers.llm.{}.base_url", config.provider_id);
        if let Some(value) = updates.get(&base_url) {
            config.base_url = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{base_url} must be a string"))?
                .to_owned();
            applied.push(base_url);
        }
        let model = format!("core.profiles.llm.{}.model", self.profile_id);
        if let Some(value) = updates.get(&model) {
            let value = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{model} must be a string"))?;
            if value != config.model {
                config.dimension = openai_embedding_dimension(value);
            }
            config.model = value.to_owned();
            applied.push(model);
        }
        let dimensions = format!("core.profiles.llm.{}.dimensions", self.profile_id);
        if let Some(value) = updates.get(&dimensions) {
            config.dimension = usize::try_from(
                value
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("{dimensions} must be a positive integer"))?,
            )?;
            applied.push(dimensions);
        }
        Ok(applied)
    }
}

fn openai_embedding_dimension(model: &str) -> usize {
    embedding_model_dimension(model)
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
        let mut current = self.config.write().await;
        let mut next = current.clone();
        let applied = self.update_config(&mut next, updates)?;
        Self::profile(&next)?;
        *current = next;
        if !applied.is_empty() {
            tracing::info!(module = self.name(), applied = ?applied, "runtime settings applied");
        }
        Ok(applied)
    }
}

#[async_trait]
impl EmbeddingAdapter for OpenAiEmbedding {
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(self
            .embed_profiled(texts)
            .await?
            .map(|batch| {
                batch
                    .vectors
                    .into_iter()
                    .map(|vector| vector.values)
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn embed_profiled(&self, texts: &[String]) -> anyhow::Result<Option<ProfiledEmbeddings>> {
        if texts.is_empty() {
            return Ok(None);
        }

        // The request and descriptor share one immutable configuration snapshot.
        // Drop the settings lock before awaiting any network operation.
        let config = self.config.read().await.clone();
        let profile = Self::profile(&config)?;
        let endpoint = &profile.endpoint;

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
            .post(format!("{endpoint}/embeddings"))
            // Covers connection, response headers and the entire response body.
            .timeout(self.request_timeout)
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

        let mut embed_response: EmbedResponse = response.json().await?;
        anyhow::ensure!(
            embed_response
                .model
                .as_ref()
                .is_none_or(|model| model == &profile.model),
            "embedding response model differs from request profile"
        );
        anyhow::ensure!(
            embed_response.data.len() == texts.len(),
            "embedding result count differs from request"
        );
        embed_response.data.sort_by_key(|row| row.index);
        let vectors = embed_response
            .data
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                anyhow::ensure!(
                    row.index == index,
                    "embedding indexes are missing or duplicated"
                );
                let vector = EmbeddingVector {
                    input_digest: embedding_text_digest(&texts[index]),
                    values: row.embedding,
                };
                vector.validate(&profile)?;
                Ok(vector)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Some(ProfiledEmbeddings { profile, vectors }))
    }

    async fn preview_profile_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Option<EmbeddingProfile>> {
        let mut next = self.config.read().await.clone();
        self.update_config(&mut next, updates)?;
        Ok(Some(Self::profile(&next)?))
    }

    fn dimension(&self) -> usize {
        self.config
            .try_read()
            .map(|config| config.dimension)
            .unwrap_or(1536)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, routing::post};
    use axum::{
        body::{Body, Bytes},
        response::{IntoResponse, Response},
    };
    use futures::StreamExt;
    use serde_json::json;
    use std::time::Duration;
    use tokio::sync::{Mutex, Notify};
    use tokio_util::sync::CancellationToken;

    #[derive(Clone, Default)]
    struct Endpoint {
        entered: Arc<Notify>,
        release: Arc<Notify>,
        requests: Arc<Mutex<Vec<Value>>>,
        hold: bool,
    }
    async fn response(State(state): State<Endpoint>, Json(request): Json<Value>) -> Response {
        state.requests.lock().await.push(request.clone());
        state.entered.notify_one();
        if request["input"][0].as_str() == Some("held body") {
            let body = futures::stream::once(async {
                Ok::<_, std::convert::Infallible>(Bytes::from_static(b"{"))
            })
            .chain(futures::stream::once(async move {
                state.release.notified().await;
                Ok::<_, std::convert::Infallible>(Bytes::from_static(b"}"))
            }));
            return Body::from_stream(body).into_response();
        }
        if state.hold {
            state.release.notified().await;
        }
        let dimensions = request["dimensions"].as_u64().unwrap_or(3) as usize;
        let mut data = request["input"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, _)| json!({"index":index,"embedding":vec![(index+1) as f32;dimensions]}))
            .collect::<Vec<_>>();
        let mut model = request["model"].clone();
        match request["input"][0].as_str().unwrap() {
            "wrong model" => model = json!("a different model"),
            "wrong dimensions" => data[0]["embedding"] = json!([1.0, 1.0]),
            "zero vector" => data[0]["embedding"] = json!(vec![0.0; dimensions]),
            "duplicate index" => data[1]["index"] = json!(0),
            "missing result" => {
                data.pop();
            }
            _ => {}
        }
        data.reverse();
        Json(json!({"model":model,"data":data})).into_response()
    }
    async fn server(state: Endpoint) -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let child = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/embeddings", post(response))
                    .with_state(state),
            )
            .with_graceful_shutdown(child.cancelled_owned())
            .await
            .unwrap();
        });
        (format!("http://{address}"), stop, task)
    }
    async fn adapter(endpoint: &str) -> Arc<OpenAiEmbedding> {
        let adapter = Arc::new(OpenAiEmbedding::new());
        {
            let mut config = adapter.config.write().await;
            config.base_url = endpoint.into();
            config.dimension = 3;
            config.api_key = "test".into();
        }
        adapter
    }

    #[tokio::test]
    async fn held_request_keeps_its_original_profile_and_settings_do_not_wait_for_network() {
        let endpoint = Endpoint {
            hold: true,
            ..Default::default()
        };
        let (url, stop, server) = server(endpoint.clone()).await;
        let adapter = adapter(&url).await;
        let worker = adapter.clone();
        let request = tokio::spawn(async move {
            worker
                .embed_profiled(&["first input".into(), "second input".into()])
                .await
                .unwrap()
                .unwrap()
        });
        tokio::time::timeout(Duration::from_secs(2), endpoint.entered.notified())
            .await
            .unwrap();
        let updates = BTreeMap::from([
            (
                "core.profiles.llm.embedding.model".into(),
                json!("text-embedding-3-large"),
            ),
            ("core.profiles.llm.embedding.dimensions".into(), json!(4)),
        ]);
        let preview = adapter
            .preview_profile_settings(&updates)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(preview.dimensions, 4);
        assert_eq!(
            adapter
                .preview_profile_settings(&BTreeMap::new())
                .await
                .unwrap()
                .unwrap()
                .dimensions,
            3
        );
        tokio::time::timeout(Duration::from_millis(500), adapter.apply_settings(&updates))
            .await
            .expect("a held HTTP request must not hold the configuration lock")
            .unwrap();
        endpoint.release.notify_one();
        let batch = request.await.unwrap();
        assert_eq!(batch.profile.model, "text-embedding-3-small");
        assert_eq!(batch.profile.dimensions, 3);
        assert_ne!(batch.profile.id().unwrap(), preview.id().unwrap());
        assert_eq!(
            batch.vectors[0].input_digest,
            embedding_text_digest("first input")
        );
        assert_eq!(batch.vectors[0].values, vec![1.0; 3]);
        assert_eq!(
            batch.vectors[1].input_digest,
            embedding_text_digest("second input")
        );
        assert_eq!(batch.vectors[1].values, vec![2.0; 3]);
        assert_eq!(
            endpoint.requests.lock().await[0]["model"],
            "text-embedding-3-small"
        );
        assert_eq!(
            adapter
                .preview_profile_settings(&BTreeMap::new())
                .await
                .unwrap()
                .unwrap(),
            preview
        );
        stop.cancel();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn malformed_provider_replies_never_acquire_a_profile() {
        let (url, stop, server) = server(Endpoint::default()).await;
        let adapter = adapter(&url).await;
        for input in [
            "wrong model",
            "wrong dimensions",
            "zero vector",
            "duplicate index",
            "missing result",
        ] {
            assert!(
                adapter
                    .embed_profiled(&[input.into(), "second".into()])
                    .await
                    .is_err(),
                "{input}"
            );
        }
        stop.cancel();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn total_request_deadline_covers_a_stalled_body_and_next_request_recovers() {
        let endpoint = Endpoint::default();
        let (url, stop, server) = server(endpoint.clone()).await;
        let mut adapter = adapter(&url).await;
        Arc::get_mut(&mut adapter).unwrap().request_timeout = Duration::from_millis(100);
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            adapter.embed_profiled(&["held body".into()]),
        )
        .await
        .expect("the request must reach its own total deadline")
        .unwrap_err();
        assert!(
            error
                .downcast_ref::<reqwest::Error>()
                .is_some_and(reqwest::Error::is_timeout)
        );
        endpoint.release.notify_one();
        let next = adapter
            .embed_profiled(&["next request".into()])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next.vectors.len(), 1);
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn preview_is_pure_and_credential_rotation_does_not_change_vector_identity() {
        let adapter = adapter("https://api.openai.com/v1").await;
        let original = adapter
            .preview_profile_settings(&BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        let reaffirm = BTreeMap::from([(
            "core.profiles.llm.embedding.model".into(),
            json!(original.model),
        )]);
        assert_eq!(
            adapter
                .preview_profile_settings(&reaffirm)
                .await
                .unwrap()
                .unwrap(),
            original,
            "reaffirming the current model must preserve explicit dimensions"
        );
        adapter.apply_settings(&reaffirm).await.unwrap();
        let credentials =
            BTreeMap::from([("core.providers.llm.openai.api_key".into(), json!("rotated"))]);
        assert_eq!(
            adapter
                .preview_profile_settings(&credentials)
                .await
                .unwrap()
                .unwrap()
                .id()
                .unwrap(),
            original.id().unwrap()
        );
        let invalid = BTreeMap::from([(
            "core.providers.llm.openai.base_url".into(),
            json!("https://user:secret@example.test?key=secret"),
        )]);
        assert!(adapter.preview_profile_settings(&invalid).await.is_err());
        assert!(adapter.apply_settings(&invalid).await.is_err());
        assert_eq!(
            adapter
                .preview_profile_settings(&BTreeMap::new())
                .await
                .unwrap()
                .unwrap(),
            original
        );
    }

    #[tokio::test]
    async fn identified_settings_reject_wrong_types_and_leave_configuration_unchanged() {
        let adapter = OpenAiEmbedding::new();
        let original = adapter
            .preview_profile_settings(&BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        for (key, value) in [
            ("core.profiles.llm.embedding.model", json!(true)),
            ("core.providers.llm.openai.base_url", json!(false)),
            ("core.providers.llm.openai.api_key", Value::Null),
            ("core.profiles.llm.embedding.dimensions", json!(-1)),
            ("core.profiles.llm.embedding.dimensions", json!("1536")),
            ("core.profiles.llm.embedding.dimensions", json!(0)),
        ] {
            let updates = BTreeMap::from([(key.to_string(), value)]);
            assert!(
                adapter.preview_profile_settings(&updates).await.is_err(),
                "{key}"
            );
            assert!(adapter.apply_settings(&updates).await.is_err(), "{key}");
            assert_eq!(
                adapter
                    .preview_profile_settings(&BTreeMap::new())
                    .await
                    .unwrap()
                    .unwrap(),
                original
            );
        }
        let unrelated = BTreeMap::from([("unrelated.module.setting".into(), json!(true))]);
        assert_eq!(
            adapter
                .preview_profile_settings(&unrelated)
                .await
                .unwrap()
                .unwrap(),
            original
        );
        assert!(adapter.apply_settings(&unrelated).await.unwrap().is_empty());
    }
}
