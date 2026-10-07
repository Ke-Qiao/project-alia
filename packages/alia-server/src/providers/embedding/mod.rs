//! Embedding adapters for text embedding.
//!
//! Available implementations:
//! - `OpenAiEmbedding`: OpenAI embedding models
//! - `NoopEmbedding`: No-op placeholder

pub(crate) mod profile_store;

#[cfg(feature = "embed-openai")]
pub mod openai;

#[cfg(feature = "embed-openai")]
pub use openai::OpenAiEmbedding;

use async_trait::async_trait;

use crate::modules::platform::lifecycle::ModuleLifecycle;
use crate::providers::adapter::EmbeddingAdapter;

pub struct NoopEmbedding;

#[async_trait]
impl ModuleLifecycle for NoopEmbedding {
    fn name(&self) -> &'static str {
        "noop_embedding"
    }
}

#[async_trait]
impl EmbeddingAdapter for NoopEmbedding {
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0; 1024]).collect())
    }

    fn dimension(&self) -> usize {
        1024
    }
}
