use async_trait::async_trait;

use crate::providers::adapter::{RouteDecision, RouterInput, RouterStrategy};
use crate::module::ModuleLifecycle;

/// Passthrough router: always routes to remote LLM
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
        Ok(RouteDecision::RemoteLlm)
    }
}
