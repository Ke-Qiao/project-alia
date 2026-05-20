//! Built-in memory system for storing and retrieving user memories.
//!
//! Memory is part of Alia's core domain. External providers should not decide
//! what the assistant remembers or how relationship state evolves.

pub mod simple;

pub use simple::AliaMemory;

use async_trait::async_trait;
use uuid::Uuid;

use crate::module::ModuleLifecycle;
use crate::providers::adapter::{
    MemoryBackend, MemoryCapabilities, MemoryListOpts, MemoryNode, MemoryPatch, RetrievalOpts,
};

pub struct NoopMemory;

#[async_trait]
impl ModuleLifecycle for NoopMemory {
    fn name(&self) -> &'static str {
        "noop_memory"
    }
}

#[async_trait]
impl MemoryBackend for NoopMemory {
    async fn store(&self, _user_id: Uuid, _node: MemoryNode) -> anyhow::Result<String> {
        Ok(Uuid::new_v4().to_string())
    }

    async fn list(&self, _user_id: Uuid, _opts: MemoryListOpts) -> anyhow::Result<Vec<MemoryNode>> {
        Ok(Vec::new())
    }

    async fn get(&self, _user_id: Uuid, _node_id: &str) -> anyhow::Result<Option<MemoryNode>> {
        Ok(None)
    }

    async fn retrieve(
        &self,
        _user_id: Uuid,
        _query: &str,
        _opts: RetrievalOpts,
    ) -> anyhow::Result<Vec<MemoryNode>> {
        Ok(Vec::new())
    }

    async fn update(&self, _node_id: &str, _patch: MemoryPatch) -> anyhow::Result<()> {
        Ok(())
    }

    async fn delete(&self, _node_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn clear(&self, _user_id: Uuid) -> anyhow::Result<u64> {
        Ok(0)
    }

    async fn memory_capabilities(&self) -> MemoryCapabilities {
        MemoryCapabilities::unavailable(self.name())
    }
}
