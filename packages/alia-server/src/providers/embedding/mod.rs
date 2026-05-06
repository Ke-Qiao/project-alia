use async_trait::async_trait;

use crate::providers::adapter::EmbeddingAdapter;
use crate::module::ModuleLifecycle;

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
