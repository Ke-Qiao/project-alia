//! Memory backends for storing and retrieving user memories.
//!
//! Available implementations:
//! - `GraphRagMemory`: Full-featured GraphRAG backend with 4 memory layers
//! - `SimpleMemory`: In-memory storage with keyword-based retrieval
//! - `NoopMemory`: No-op placeholder (returns empty results)
//!
//! Memory Layers:
//! - L0 Working: In-memory conversation context window
//! - L1 Episodic: DB-backed conversation summaries (with TTL)
//! - L2 Core: User profile, preferences, key facts (persistent)
//! - L3 Procedural: Habits and behavior patterns (system-maintained)

#[cfg(feature = "memory-graphrag")]
pub mod core;
#[cfg(feature = "memory-graphrag")]
pub mod episodic;
#[cfg(feature = "memory-graphrag")]
pub mod graph;
#[cfg(feature = "memory-graphrag")]
pub mod graphrag;
#[cfg(feature = "memory-graphrag")]
pub mod procedural;
#[cfg(feature = "memory-graphrag")]
pub mod retrieval;
pub mod simple;
#[cfg(feature = "memory-graphrag")]
pub mod working;

#[cfg(feature = "memory-graphrag")]
pub use graphrag::GraphRagMemory;
pub use simple::SimpleMemory;

use async_trait::async_trait;
use uuid::Uuid;

use crate::module::ModuleLifecycle;
use crate::providers::adapter::{
    GraphFragment, MemoryBackend, MemoryNode, MemoryPatch, RetrievalOpts,
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
