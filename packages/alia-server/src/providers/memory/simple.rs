//! Simple In-Memory Backend
//!
//! A lightweight memory backend that stores memories in an in-memory HashMap.
//! Supports basic keyword-based retrieval without vector search or graph features.
//! Suitable for development, testing, or deployments without PostgreSQL/pgvector.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    GraphFragment, MemoryBackend, MemoryNode, MemoryPatch, RetrievalOpts,
};

/// In-memory storage for user memories
type UserMemories = HashMap<Uuid, Vec<MemoryNode>>;

/// Simple in-memory backend
pub struct SimpleMemory {
    memories: Arc<RwLock<UserMemories>>,
}

impl SimpleMemory {
    pub fn new() -> Self {
        Self {
            memories: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Simple keyword-based relevance scoring
    fn score_relevance(query: &str, content: &str) -> f32 {
        let query_words: Vec<&str> = query.split_whitespace().collect();
        let content_lower = content.to_lowercase();
        
        let mut matches = 0;
        for word in &query_words {
            if content_lower.contains(&word.to_lowercase()) {
                matches += 1;
            }
        }
        
        if query_words.is_empty() {
            return 0.0;
        }
        
        matches as f32 / query_words.len() as f32
    }
}

impl Default for SimpleMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for SimpleMemory {
    fn name(&self) -> &'static str {
        "simple"
    }

    async fn init(&mut self, _ctx: &ModuleContext) -> anyhow::Result<()> {
        tracing::info!("SimpleMemory initialized (in-memory storage)");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}

#[async_trait]
impl MemoryBackend for SimpleMemory {
    async fn store(&self, user_id: Uuid, mut node: MemoryNode) -> anyhow::Result<Uuid> {
        let id = node.id.unwrap_or_else(Uuid::new_v4);
        node.id = Some(id);
        
        let mut memories = self.memories.write().await;
        memories.entry(user_id).or_default().push(node);
        
        tracing::debug!(user_id = %user_id, node_id = %id, "stored memory node");
        Ok(id)
    }

    async fn retrieve(
        &self,
        user_id: Uuid,
        query: &str,
        opts: RetrievalOpts,
    ) -> anyhow::Result<Vec<MemoryNode>> {
        let memories = self.memories.read().await;
        
        let user_memories = match memories.get(&user_id) {
            Some(mems) => mems,
            None => return Ok(Vec::new()),
        };

        // Filter by memory layer if specified
        let filtered: Vec<_> = user_memories
            .iter()
            .filter(|m| {
                if let Some(ref layers) = opts.layers {
                    layers.contains(&m.memory_layer)
                } else {
                    true
                }
            })
            .collect();

        // Score and rank by relevance
        let mut scored: Vec<_> = filtered
            .into_iter()
            .map(|m| {
                let score = Self::score_relevance(query, &m.content) * m.importance;
                (m.clone(), score)
            })
            .filter(|(_, score)| {
                if let Some(min) = opts.min_similarity {
                    *score >= min
                } else {
                    *score > 0.0
                }
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Limit results
        let limit = opts.max_results.unwrap_or(10) as usize;
        let results: Vec<MemoryNode> = scored.into_iter().take(limit).map(|(m, _)| m).collect();

        tracing::debug!(
            user_id = %user_id,
            query = %query,
            result_count = results.len(),
            "retrieved memories"
        );

        Ok(results)
    }

    async fn update(&self, node_id: Uuid, patch: MemoryPatch) -> anyhow::Result<()> {
        let mut memories = self.memories.write().await;
        
        for user_memories in memories.values_mut() {
            for memory in user_memories.iter_mut() {
                if memory.id == Some(node_id) {
                    if let Some(content) = patch.content {
                        memory.content = content;
                    }
                    if let Some(metadata) = patch.metadata {
                        memory.metadata = metadata;
                    }
                    if let Some(importance) = patch.importance {
                        memory.importance = importance;
                    }
                    tracing::debug!(node_id = %node_id, "updated memory node");
                    return Ok(());
                }
            }
        }

        anyhow::bail!("Memory node {} not found", node_id)
    }

    async fn delete(&self, node_id: Uuid) -> anyhow::Result<()> {
        let mut memories = self.memories.write().await;
        
        for user_memories in memories.values_mut() {
            let original_len = user_memories.len();
            user_memories.retain(|m| m.id != Some(node_id));
            if user_memories.len() < original_len {
                tracing::debug!(node_id = %node_id, "deleted memory node");
                return Ok(());
            }
        }

        // Not finding the node is not an error (idempotent delete)
        Ok(())
    }

    async fn graph_neighbors(&self, _node_id: Uuid, _depth: u32) -> anyhow::Result<GraphFragment> {
        // SimpleMemory doesn't support graph operations
        Ok(GraphFragment {
            nodes: Vec::new(),
            edges: Vec::new(),
        })
    }

    async fn refresh_communities(&self, _user_id: Uuid) -> anyhow::Result<()> {
        // SimpleMemory doesn't support community detection
        Ok(())
    }
}
