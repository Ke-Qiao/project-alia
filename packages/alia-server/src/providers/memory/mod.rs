use async_trait::async_trait;
use uuid::Uuid;

use crate::providers::adapter::{
    GraphFragment, MemoryBackend, MemoryNode, MemoryPatch, RetrievalOpts,
};
use crate::module::ModuleLifecycle;

pub struct NoopMemory;

#[async_trait]
impl ModuleLifecycle for NoopMemory {
    fn name(&self) -> &'static str {
        "noop_memory"
    }
}

#[async_trait]
impl MemoryBackend for NoopMemory {
    async fn store(&self, _user_id: Uuid, _node: MemoryNode) -> anyhow::Result<Uuid> {
        Ok(Uuid::new_v4())
    }

    async fn retrieve(
        &self,
        _user_id: Uuid,
        _query: &str,
        _opts: RetrievalOpts,
    ) -> anyhow::Result<Vec<MemoryNode>> {
        Ok(Vec::new())
    }

    async fn update(&self, _node_id: Uuid, _patch: MemoryPatch) -> anyhow::Result<()> {
        Ok(())
    }

    async fn delete(&self, _node_id: Uuid) -> anyhow::Result<()> {
        Ok(())
    }

    async fn graph_neighbors(&self, _node_id: Uuid, _depth: u32) -> anyhow::Result<GraphFragment> {
        Ok(GraphFragment {
            nodes: Vec::new(),
            edges: Vec::new(),
        })
    }
}
