use std::sync::Arc;

use crate::providers::adapter::ToolRegistry;

use super::storage::PluginStorage;

/// Scoped capabilities handed to compiled-in LLM tool plugins.
///
/// This is intentionally narrower than `ModuleContext`: built-in components
/// keep their normal database repositories, while tool plugins receive only
/// the storage namespace and tool registry they need.
#[derive(Clone)]
pub struct PluginToolContext {
    pub plugin_name: String,
    pub storage: Arc<dyn PluginStorage>,
    pub tools: Arc<dyn ToolRegistry>,
}

pub trait CompiledToolPlugin: Send + Sync {
    fn name(&self) -> &'static str;

    fn storage_manifest(&self) -> crate::plugins::PluginStorageManifest {
        crate::plugins::PluginStorageManifest::default()
    }

    fn register(&self, ctx: PluginToolContext) -> anyhow::Result<()>;
}
