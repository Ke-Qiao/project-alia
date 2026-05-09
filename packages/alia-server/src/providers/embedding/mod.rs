//! Embedding adapters for text embedding.
//!
//! Available implementations:
//! - `OllamaEmbedding`: Local Ollama embedding models
//! - `OpenAiEmbedding`: OpenAI embedding models
//! - `NoopEmbedding`: No-op placeholder

#[cfg(feature = "embed-ollama")]
pub mod ollama;
#[cfg(feature = "embed-openai")]
pub mod openai;

#[cfg(feature = "embed-ollama")]
pub use ollama::OllamaEmbedding;
#[cfg(feature = "embed-openai")]
pub use openai::OpenAiEmbedding;

use async_trait::async_trait;

use crate::module::ModuleLifecycle;
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
