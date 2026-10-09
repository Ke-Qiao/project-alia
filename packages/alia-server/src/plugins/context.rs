use std::sync::Arc;

use async_trait::async_trait;

use crate::providers::adapter::ToolRegistry;

use super::storage::PluginStorage;

/// Scoped capabilities handed to compiled-in LLM tool plugins.
///
/// Tool plugins receive only their storage namespace and the executable tool
/// registry; built-in components keep their own database repositories.
#[derive(Clone)]
pub struct PluginToolContext {
    pub plugin_name: String,
    pub storage: Arc<dyn PluginStorage>,
    pub tools: Arc<dyn ToolRegistry>,
}

#[async_trait]
pub trait CompiledToolPlugin: Send + Sync {
    fn name(&self) -> &'static str;

    fn storage_manifest(&self) -> crate::plugins::PluginStorageManifest {
        crate::plugins::PluginStorageManifest::default()
    }

    async fn register(&self, ctx: PluginToolContext) -> anyhow::Result<()>;
}
