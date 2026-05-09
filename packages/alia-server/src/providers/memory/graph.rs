//! Knowledge Graph Operations
//!
//! Node and edge CRUD operations for the GraphRAG knowledge graph.
//! Includes community detection for global search.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::memory_edge::RelationType;
use crate::entity::memory_node::{MemoryLayer, NodeType};
use crate::entity::{memory_community, memory_edge, memory_node};

/// Graph edge with resolved node info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub id: Uuid,
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    pub weight: f32,
    pub metadata: serde_json::Value,
}

/// Graph node with basic info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: Uuid,
    pub user_id: Uuid,
    pub memory_layer: String,
    pub node_type: String,
    pub content: String,
    pub importance: f32,
    pub metadata: serde_json::Value,
}

/// Graph fragment (subgraph)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphFragment {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Community (topic cluster)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Community {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    pub summary: String,
    pub node_ids: Vec<Uuid>,
    pub level: i32,
}

/// Knowledge Graph Manager
pub struct KnowledgeGraph {
    db: DatabaseConnection,
}

impl KnowledgeGraph {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    // ==================== Node Operations ====================

    /// Create a new graph node
    pub async fn create_node(
        &self,
        user_id: Uuid,
        layer: MemoryLayer,
        node_type: NodeType,
        content: String,
        metadata: serde_json::Value,
        importance: f32,
    ) -> anyhow::Result<Uuid> {
        let id = Uuid::new_v4();

        let model = memory_node::ActiveModel {
            id: Set(id),
            user_id: Set(user_id),
            memory_layer: Set(layer),
            node_type: Set(node_type),
            content: Set(content),
            metadata: Set(metadata),
            embedding: Set(None),
            importance: Set(importance),
            access_count: Set(0),
            last_accessed_at: Set(None),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            expires_at: Set(None),
        };

        model.insert(&self.db).await?;
        Ok(id)
    }

    /// Get node by ID
    pub async fn get_node(&self, id: Uuid) -> anyhow::Result<Option<GraphNode>> {
        let node = memory_node::Entity::find_by_id(id).one(&self.db).await?;
        Ok(node.map(Self::model_to_graph_node))
    }

    /// Delete a node (cascades to edges)
    pub async fn delete_node(&self, id: Uuid) -> anyhow::Result<bool> {
        let result = memory_node::Entity::delete_by_id(id).exec(&self.db).await?;
        Ok(result.rows_affected > 0)
    }

    /// Update node embedding (raw vector bytes for pgvector)
    pub async fn update_embedding(&self, id: Uuid, embedding: Vec<f32>) -> anyhow::Result<()> {
        // Convert f32 vec to bytes for storage
        // pgvector expects specific format - we'll use raw SQL for actual vector ops
        let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();

        let node = memory_node::Entity::find_by_id(id).one(&self.db).await?;
        if let Some(node) = node {
            let mut active: memory_node::ActiveModel = node.into();
            active.embedding = Set(Some(bytes));
            active.updated_at = Set(Utc::now());
            active.update(&self.db).await?;
        }

        Ok(())
    }

    // ==================== Edge Operations ====================

    /// Create an edge between two nodes
    pub async fn create_edge(
        &self,
        source_id: Uuid,
        target_id: Uuid,
        relation_type: RelationType,
        weight: f32,
        metadata: serde_json::Value,
    ) -> anyhow::Result<Uuid> {
        let id = Uuid::new_v4();

        let model = memory_edge::ActiveModel {
            id: Set(id),
            source_id: Set(source_id),
            target_id: Set(target_id),
            relation_type: Set(relation_type),
            weight: Set(weight),
            metadata: Set(metadata),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
        };

        model.insert(&self.db).await?;
        Ok(id)
    }

    /// Get edges from a node
    pub async fn get_outgoing_edges(&self, node_id: Uuid) -> anyhow::Result<Vec<GraphEdge>> {
        let edges = memory_edge::Entity::find()
            .filter(memory_edge::Column::SourceId.eq(node_id))
            .all(&self.db)
            .await?;

        Ok(edges.into_iter().map(Self::model_to_graph_edge).collect())
    }

    /// Get edges to a node
    pub async fn get_incoming_edges(&self, node_id: Uuid) -> anyhow::Result<Vec<GraphEdge>> {
        let edges = memory_edge::Entity::find()
            .filter(memory_edge::Column::TargetId.eq(node_id))
            .all(&self.db)
            .await?;

        Ok(edges.into_iter().map(Self::model_to_graph_edge).collect())
    }

    /// Delete an edge
    pub async fn delete_edge(&self, id: Uuid) -> anyhow::Result<bool> {
        let result = memory_edge::Entity::delete_by_id(id).exec(&self.db).await?;
        Ok(result.rows_affected > 0)
    }

    /// Update edge weight
    pub async fn update_edge_weight(&self, id: Uuid, weight: f32) -> anyhow::Result<()> {
        let edge = memory_edge::Entity::find_by_id(id).one(&self.db).await?;
        if let Some(edge) = edge {
            let mut active: memory_edge::ActiveModel = edge.into();
            active.weight = Set(weight);
            active.updated_at = Set(Utc::now());
            active.update(&self.db).await?;
        }
        Ok(())
    }

    // ==================== Graph Traversal ====================

    /// Get N-hop neighborhood of a node
    pub async fn get_neighbors(&self, node_id: Uuid, depth: u32) -> anyhow::Result<GraphFragment> {
        let mut visited_nodes: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        let mut all_edges: Vec<GraphEdge> = Vec::new();
        let mut frontier = vec![node_id];

        for _ in 0..depth {
            let mut next_frontier = Vec::new();

            for current_id in &frontier {
                if visited_nodes.contains(current_id) {
                    continue;
                }
                visited_nodes.insert(*current_id);

                // Get outgoing edges
                let out_edges = self.get_outgoing_edges(*current_id).await?;
                for edge in &out_edges {
                    if !visited_nodes.contains(&edge.target_id) {
                        next_frontier.push(edge.target_id);
                    }
                }
                all_edges.extend(out_edges);

                // Get incoming edges
                let in_edges = self.get_incoming_edges(*current_id).await?;
                for edge in &in_edges {
                    if !visited_nodes.contains(&edge.source_id) {
                        next_frontier.push(edge.source_id);
                    }
                }
                all_edges.extend(in_edges);
            }

            frontier = next_frontier;
        }

        // Fetch all visited nodes
        let nodes: Vec<GraphNode> = memory_node::Entity::find()
            .filter(memory_node::Column::Id.is_in(visited_nodes.iter().cloned()))
            .all(&self.db)
            .await?
            .into_iter()
            .map(Self::model_to_graph_node)
            .collect();

        // Deduplicate edges
        let mut edge_set: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        let edges: Vec<GraphEdge> = all_edges
            .into_iter()
            .filter(|e| edge_set.insert(e.id))
            .collect();

        Ok(GraphFragment { nodes, edges })
    }

    /// Find shortest path between two nodes (BFS)
    pub async fn find_path(
        &self,
        from: Uuid,
        to: Uuid,
        max_depth: u32,
    ) -> anyhow::Result<Option<Vec<Uuid>>> {
        use std::collections::{HashMap, VecDeque};

        let mut visited: HashMap<Uuid, Uuid> = HashMap::new(); // node -> parent
        let mut queue: VecDeque<(Uuid, u32)> = VecDeque::new();

        queue.push_back((from, 0));
        visited.insert(from, from); // self as parent for start

        while let Some((current, depth)) = queue.pop_front() {
            if current == to {
                // Reconstruct path
                let mut path = vec![to];
                let mut node = to;
                while node != from {
                    node = *visited.get(&node).unwrap();
                    path.push(node);
                }
                path.reverse();
                return Ok(Some(path));
            }

            if depth >= max_depth {
                continue;
            }

            // Explore neighbors
            let edges = self.get_outgoing_edges(current).await?;
            for edge in edges {
                if !visited.contains_key(&edge.target_id) {
                    visited.insert(edge.target_id, current);
                    queue.push_back((edge.target_id, depth + 1));
                }
            }
        }

        Ok(None)
    }

    // ==================== Community Operations ====================

    /// Create or update a community
    pub async fn upsert_community(
        &self,
        user_id: Uuid,
        title: String,
        summary: String,
        node_ids: Vec<Uuid>,
        level: i32,
    ) -> anyhow::Result<Uuid> {
        let id = Uuid::new_v4();
        let node_ids_json =
            serde_json::json!(node_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>());

        let model = memory_community::ActiveModel {
            id: Set(id),
            user_id: Set(user_id),
            title: Set(title),
            summary: Set(summary),
            node_ids: Set(node_ids_json),
            level: Set(level),
            embedding: Set(None),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
        };

        model.insert(&self.db).await?;
        Ok(id)
    }

    /// Get communities for a user
    pub async fn get_communities(
        &self,
        user_id: Uuid,
        level: Option<i32>,
    ) -> anyhow::Result<Vec<Community>> {
        let mut query =
            memory_community::Entity::find().filter(memory_community::Column::UserId.eq(user_id));

        if let Some(l) = level {
            query = query.filter(memory_community::Column::Level.eq(l));
        }

        let communities = query
            .order_by_desc(memory_community::Column::UpdatedAt)
            .all(&self.db)
            .await?;

        Ok(communities
            .into_iter()
            .map(|c| {
                let node_ids = c.get_node_ids();
                Community {
                    id: c.id,
                    user_id: c.user_id,
                    title: c.title,
                    summary: c.summary,
                    node_ids,
                    level: c.level,
                }
            })
            .collect())
    }

    /// Delete all communities for a user (before regeneration)
    pub async fn clear_communities(&self, user_id: Uuid) -> anyhow::Result<u64> {
        let result = memory_community::Entity::delete_many()
            .filter(memory_community::Column::UserId.eq(user_id))
            .exec(&self.db)
            .await?;

        Ok(result.rows_affected)
    }

    /// Get full graph for a user (for visualization)
    pub async fn get_user_graph(&self, user_id: Uuid) -> anyhow::Result<GraphFragment> {
        let nodes: Vec<GraphNode> = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .all(&self.db)
            .await?
            .into_iter()
            .map(Self::model_to_graph_node)
            .collect();

        let node_ids: Vec<Uuid> = nodes.iter().map(|n| n.id).collect();

        let edges: Vec<GraphEdge> = memory_edge::Entity::find()
            .filter(memory_edge::Column::SourceId.is_in(node_ids.clone()))
            .all(&self.db)
            .await?
            .into_iter()
            .map(Self::model_to_graph_edge)
            .collect();

        Ok(GraphFragment { nodes, edges })
    }

    // ==================== Helpers ====================

    fn model_to_graph_node(node: memory_node::Model) -> GraphNode {
        GraphNode {
            id: node.id,
            user_id: node.user_id,
            memory_layer: node.memory_layer.to_string(),
            node_type: node.node_type.to_string(),
            content: node.content,
            importance: node.importance,
            metadata: node.metadata,
        }
    }

    fn model_to_graph_edge(edge: memory_edge::Model) -> GraphEdge {
        GraphEdge {
            id: edge.id,
            source_id: edge.source_id,
            target_id: edge.target_id,
            relation_type: edge.relation_type.to_string(),
            weight: edge.weight,
            metadata: edge.metadata,
        }
    }
}
