//! GraphRAG Memory Backend
//!
//! Full-featured memory backend implementing the MemoryBackend trait.
//! Integrates all four memory layers and provides hybrid retrieval.
//!
//! Architecture:
//! - L0 Working Memory: In-memory conversation context
//! - L1 Episodic Memory: DB-backed conversation summaries
//! - L2 Core Memory: User profile, preferences, facts
//! - L3 Procedural Memory: Habits and behavior patterns
//! - Knowledge Graph: Entity relationships and community detection
//! - Hybrid Retrieval: Local + Global + Vector search

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::entity::memory_node::{MemoryLayer, NodeType};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    GraphFragment as AdapterGraphFragment, MemoryBackend, MemoryEdge as AdapterMemoryEdge,
    MemoryNode as AdapterMemoryNode, MemoryPatch, RetrievalOpts,
};

use super::core::CoreMemory;
use super::episodic::EpisodicMemory;
use super::graph::KnowledgeGraph;
use super::procedural::ProceduralMemory;
use super::retrieval::{HybridRetrieval, RetrievalOptions};
use super::working::WorkingMemory;

/// GraphRAG Memory Backend
///
/// Implements the full memory system with all four layers and hybrid retrieval.
pub struct GraphRagMemory {
    db: Option<DatabaseConnection>,

    // Memory layers
    working: Arc<WorkingMemory>,
    episodic: Option<EpisodicMemory>,
    core: Option<CoreMemory>,
    procedural: Option<ProceduralMemory>,

    // Graph operations
    graph: Option<KnowledgeGraph>,

    // Retrieval engine
    retrieval: Option<HybridRetrieval>,

    // Configuration
    config: GraphRagConfig,
}

/// Configuration for GraphRAG memory
#[derive(Debug, Clone)]
pub struct GraphRagConfig {
    /// L0 working memory window size
    pub working_window_size: usize,
    /// L1 episodic memory TTL in days
    pub episodic_ttl_days: i64,
    /// L2 max core memories per user
    pub max_core_memories: u64,
    /// L3 max procedural memories per user
    pub max_procedural_memories: u64,
    /// Minimum confidence for procedural memories
    pub procedural_min_confidence: f32,
}

impl Default for GraphRagConfig {
    fn default() -> Self {
        Self {
            working_window_size: 20,
            episodic_ttl_days: 90,
            max_core_memories: 500,
            max_procedural_memories: 200,
            procedural_min_confidence: 0.3,
        }
    }
}

impl GraphRagMemory {
    pub fn new() -> Self {
        Self {
            db: None,
            working: Arc::new(WorkingMemory::default()),
            episodic: None,
            core: None,
            procedural: None,
            graph: None,
            retrieval: None,
            config: GraphRagConfig::default(),
        }
    }

    pub fn with_config(config: GraphRagConfig) -> Self {
        Self {
            db: None,
            working: Arc::new(WorkingMemory::new(config.working_window_size)),
            episodic: None,
            core: None,
            procedural: None,
            graph: None,
            retrieval: None,
            config,
        }
    }

    /// Get the working memory layer (L0)
    pub fn working(&self) -> Arc<WorkingMemory> {
        self.working.clone()
    }

    /// Get the episodic memory layer (L1)
    pub fn episodic(&self) -> Option<&EpisodicMemory> {
        self.episodic.as_ref()
    }

    /// Get the core memory layer (L2)
    pub fn core(&self) -> Option<&CoreMemory> {
        self.core.as_ref()
    }

    /// Get the procedural memory layer (L3)
    pub fn procedural(&self) -> Option<&ProceduralMemory> {
        self.procedural.as_ref()
    }

    /// Get the knowledge graph
    pub fn graph(&self) -> Option<&KnowledgeGraph> {
        self.graph.as_ref()
    }

    /// Get the retrieval engine
    pub fn retrieval(&self) -> Option<&HybridRetrieval> {
        self.retrieval.as_ref()
    }

    /// Convert adapter types to internal types
    fn adapter_to_layer(layer: &str) -> MemoryLayer {
        match layer {
            "episodic" => MemoryLayer::Episodic,
            "core" => MemoryLayer::Core,
            "procedural" => MemoryLayer::Procedural,
            _ => MemoryLayer::Core,
        }
    }

    fn adapter_to_node_type(node_type: &str) -> NodeType {
        match node_type {
            "entity" => NodeType::Entity,
            "fact" => NodeType::Fact,
            "preference" => NodeType::Preference,
            "habit" => NodeType::Habit,
            "event" => NodeType::Event,
            "person" => NodeType::Person,
            "summary" => NodeType::Summary,
            _ => NodeType::Fact,
        }
    }
}

impl Default for GraphRagMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for GraphRagMemory {
    fn name(&self) -> &'static str {
        "graphrag_memory"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let db = ctx.db.clone();

        // Initialize memory layers with database connection
        self.db = Some(db.clone());
        self.episodic = Some(EpisodicMemory::new(db.clone()).with_ttl(self.config.episodic_ttl_days));
        self.core = Some(CoreMemory::new(db.clone()).with_max_per_user(self.config.max_core_memories));
        self.procedural = Some(ProceduralMemory::new(db.clone()));
        self.graph = Some(KnowledgeGraph::new(db.clone()));
        self.retrieval = Some(HybridRetrieval::new(db.clone()));

        tracing::info!(
            working_window = self.config.working_window_size,
            episodic_ttl = self.config.episodic_ttl_days,
            "GraphRAG memory initialized"
        );

        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if self.db.is_some() {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Unhealthy("Database not initialized".into())
        }
    }
}

#[async_trait]
impl MemoryBackend for GraphRagMemory {
    async fn store(&self, user_id: Uuid, node: AdapterMemoryNode) -> anyhow::Result<Uuid> {
        let graph = self.graph.as_ref().ok_or_else(|| {
            anyhow::anyhow!("GraphRAG not initialized - call init() first")
        })?;

        let layer = Self::adapter_to_layer(&node.memory_layer);
        let node_type = Self::adapter_to_node_type(&node.node_type);

        let id = graph
            .create_node(
                user_id,
                layer,
                node_type,
                node.content,
                node.metadata,
                node.importance,
            )
            .await?;

        Ok(id)
    }

    async fn retrieve(
        &self,
        user_id: Uuid,
        query: &str,
        opts: RetrievalOpts,
    ) -> anyhow::Result<Vec<AdapterMemoryNode>> {
        let retrieval = self.retrieval.as_ref().ok_or_else(|| {
            anyhow::anyhow!("GraphRAG not initialized - call init() first")
        })?;

        let retrieval_opts = RetrievalOptions {
            max_results: opts.max_results.unwrap_or(10) as usize,
            min_relevance: opts.min_similarity.unwrap_or(0.3),
            layers: opts.layers,
            include_graph_context: true,
            graph_depth: 1,
        };

        // For now, do keyword search (embedding would require external adapter)
        let results = retrieval
            .retrieve(user_id, query, None, &[], &retrieval_opts)
            .await?;

        // Convert to adapter types
        let nodes: Vec<AdapterMemoryNode> = results
            .into_iter()
            .map(|r| AdapterMemoryNode {
                id: Some(r.node_id),
                memory_layer: r.memory_layer,
                node_type: r.node_type,
                content: r.content,
                metadata: r.metadata,
                importance: r.importance,
            })
            .collect();

        Ok(nodes)
    }

    async fn update(&self, node_id: Uuid, patch: MemoryPatch) -> anyhow::Result<()> {
        let db = self.db.as_ref().ok_or_else(|| {
            anyhow::anyhow!("GraphRAG not initialized - call init() first")
        })?;

        use crate::entity::memory_node;
        use sea_orm::{ActiveModelTrait, EntityTrait, Set};

        let node = memory_node::Entity::find_by_id(node_id)
            .one(db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", node_id))?;

        let mut active: memory_node::ActiveModel = node.clone().into();

        if let Some(content) = patch.content {
            active.content = Set(content);
        }

        if let Some(metadata) = patch.metadata {
            active.metadata = Set(metadata);
        }

        if let Some(importance) = patch.importance {
            active.importance = Set(importance);
        }

        active.updated_at = Set(chrono::Utc::now());
        active.update(db).await?;

        Ok(())
    }

    async fn delete(&self, node_id: Uuid) -> anyhow::Result<()> {
        let graph = self.graph.as_ref().ok_or_else(|| {
            anyhow::anyhow!("GraphRAG not initialized - call init() first")
        })?;

        graph.delete_node(node_id).await?;
        Ok(())
    }

    async fn graph_neighbors(&self, node_id: Uuid, depth: u32) -> anyhow::Result<AdapterGraphFragment> {
        let graph = self.graph.as_ref().ok_or_else(|| {
            anyhow::anyhow!("GraphRAG not initialized - call init() first")
        })?;

        let fragment = graph.get_neighbors(node_id, depth).await?;

        // Convert to adapter types
        let nodes: Vec<AdapterMemoryNode> = fragment
            .nodes
            .into_iter()
            .map(|n| AdapterMemoryNode {
                id: Some(n.id),
                memory_layer: n.memory_layer,
                node_type: n.node_type,
                content: n.content,
                metadata: n.metadata,
                importance: n.importance,
            })
            .collect();

        let edges: Vec<AdapterMemoryEdge> = fragment
            .edges
            .into_iter()
            .map(|e| AdapterMemoryEdge {
                source_id: e.source_id,
                target_id: e.target_id,
                relation_type: e.relation_type,
                weight: e.weight,
            })
            .collect();

        Ok(AdapterGraphFragment { nodes, edges })
    }

    async fn refresh_communities(&self, user_id: Uuid) -> anyhow::Result<()> {
        let graph = self.graph.as_ref().ok_or_else(|| {
            anyhow::anyhow!("GraphRAG not initialized - call init() first")
        })?;

        // Clear existing communities
        graph.clear_communities(user_id).await?;

        // TODO: Implement actual community detection algorithm
        // This would typically use Louvain or similar clustering
        // For now, just log that it was called
        tracing::info!(user_id = %user_id, "Community refresh requested (not yet implemented)");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GraphRagConfig::default();
        assert_eq!(config.working_window_size, 20);
        assert_eq!(config.episodic_ttl_days, 90);
        assert_eq!(config.max_core_memories, 500);
    }

    #[test]
    fn test_layer_conversion() {
        assert!(matches!(
            GraphRagMemory::adapter_to_layer("episodic"),
            MemoryLayer::Episodic
        ));
        assert!(matches!(
            GraphRagMemory::adapter_to_layer("core"),
            MemoryLayer::Core
        ));
        assert!(matches!(
            GraphRagMemory::adapter_to_layer("procedural"),
            MemoryLayer::Procedural
        ));
    }
}
