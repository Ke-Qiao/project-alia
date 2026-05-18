#[cfg(feature = "llm-ollama")]
pub mod multi_layer;

use async_trait::async_trait;

use crate::module::ModuleLifecycle;
use crate::providers::adapter::{RouteDecision, RouterInput, RouterStrategy};

#[cfg(feature = "llm-ollama")]
pub use multi_layer::MultiLayerRouter;

/// Passthrough router: always routes to the major LLM.
pub struct PassthroughRouter;

#[async_trait]
impl ModuleLifecycle for PassthroughRouter {
    fn name(&self) -> &'static str {
        "passthrough"
    }
}

#[async_trait]
impl RouterStrategy for PassthroughRouter {
    async fn route(&self, _input: &RouterInput) -> anyhow::Result<RouteDecision> {
        Ok(RouteDecision::MajorLlm {
            system_prompt: None,
        })
    }
}
