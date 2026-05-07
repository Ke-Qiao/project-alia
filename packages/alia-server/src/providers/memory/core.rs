//! L2 Core Memory - User Profile and Key Facts
//!
//! Persistent storage for user preferences, facts, and relationships.
//! No TTL - persists until user deletes or system consolidates.
//!
//! Node types:
//! - `preference`: User preferences (e.g., "prefers 26°C AC")
//! - `fact`: Key facts about user (e.g., "lives in Shanghai")
//! - `person`: People in user's life (e.g., "daughter Xiaoming")
//! - `entity`: Named entities (devices, places, etc.)

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::memory_node::{self, MemoryLayer, NodeType};

/// Core memory entry types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreType {
    Preference,
    Fact,
    Person,
    Entity,
}

impl From<CoreType> for NodeType {
    fn from(ct: CoreType) -> Self {
        match ct {
            CoreType::Preference => NodeType::Preference,
            CoreType::Fact => NodeType::Fact,
            CoreType::Person => NodeType::Person,
            CoreType::Entity => NodeType::Entity,
        }
    }
}

/// Core memory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreEntry {
    pub id: Uuid,
    pub user_id: Uuid,
    pub core_type: CoreType,
    /// Natural language description
    pub content: String,
    /// Structured data (type-specific)
    pub data: serde_json::Value,
    /// Importance score (0.0 - 1.0)
    pub importance: f32,
    /// Source of this memory (e.g., "conversation", "explicit", "inferred")
    pub source: String,
    /// Creation timestamp
    pub created_at: chrono::DateTime<Utc>,
    /// Last update timestamp
    pub updated_at: chrono::DateTime<Utc>,
}

/// L2 Core Memory Manager
pub struct CoreMemory {
    db: DatabaseConnection,
    /// Maximum core memories per user
    max_per_user: u64,
}

impl CoreMemory {
    pub fn new(db: DatabaseConnection) -> Self {
        Self {
            db,
            max_per_user: 500,
        }
    }

    pub fn with_max_per_user(mut self, max: u64) -> Self {
        self.max_per_user = max;
        self
    }

    /// Store a new core memory
    pub async fn store(&self, entry: CoreEntry) -> anyhow::Result<Uuid> {
        // Check limit
        let count = self.count(entry.user_id).await?;
        if count >= self.max_per_user {
            anyhow::bail!(
                "Core memory limit reached ({}/{})",
                count,
                self.max_per_user
            );
        }

        let metadata = serde_json::json!({
            "source": entry.source,
            "data": entry.data,
        });

        let model = memory_node::ActiveModel {
            id: Set(entry.id),
            user_id: Set(entry.user_id),
            memory_layer: Set(MemoryLayer::Core),
            node_type: Set(entry.core_type.into()),
            content: Set(entry.content),
            metadata: Set(metadata),
            embedding: Set(None),
            importance: Set(entry.importance),
            access_count: Set(0),
            last_accessed_at: Set(None),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            expires_at: Set(None), // Core memories don't expire
        };

        model.insert(&self.db).await?;
        Ok(entry.id)
    }

    /// Get all core memories for a user
    pub async fn get_all(&self, user_id: Uuid) -> anyhow::Result<Vec<CoreEntry>> {
        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Core))
            .order_by_desc(memory_node::Column::Importance)
            .all(&self.db)
            .await?;

        Ok(nodes.into_iter().map(Self::node_to_entry).collect())
    }

    /// Get core memories by type
    pub async fn get_by_type(
        &self,
        user_id: Uuid,
        core_type: CoreType,
    ) -> anyhow::Result<Vec<CoreEntry>> {
        let node_type: NodeType = core_type.into();

        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Core))
            .filter(memory_node::Column::NodeType.eq(node_type))
            .order_by_desc(memory_node::Column::Importance)
            .all(&self.db)
            .await?;

        Ok(nodes.into_iter().map(Self::node_to_entry).collect())
    }

    /// Get core memory by ID
    pub async fn get_by_id(&self, id: Uuid) -> anyhow::Result<Option<CoreEntry>> {
        let node = memory_node::Entity::find_by_id(id).one(&self.db).await?;
        Ok(node.map(Self::node_to_entry))
    }

    /// Update core memory content
    pub async fn update(
        &self,
        id: Uuid,
        content: Option<String>,
        data: Option<serde_json::Value>,
        importance: Option<f32>,
    ) -> anyhow::Result<()> {
        let node = memory_node::Entity::find_by_id(id).one(&self.db).await?;

        if let Some(node) = node {
            let mut active: memory_node::ActiveModel = node.clone().into();

            if let Some(content) = content {
                active.content = Set(content);
            }

            if let Some(data) = data {
                let mut metadata = node.metadata.clone();
                metadata["data"] = data;
                active.metadata = Set(metadata);
            }

            if let Some(importance) = importance {
                active.importance = Set(importance);
            }

            active.updated_at = Set(Utc::now());
            active.update(&self.db).await?;
        }

        Ok(())
    }

    /// Delete core memory
    pub async fn delete(&self, id: Uuid) -> anyhow::Result<bool> {
        let result = memory_node::Entity::delete_by_id(id).exec(&self.db).await?;
        Ok(result.rows_affected > 0)
    }

    /// Search core memories by content (keyword matching)
    pub async fn search(
        &self,
        user_id: Uuid,
        query: &str,
        limit: u64,
    ) -> anyhow::Result<Vec<CoreEntry>> {
        // Simple ILIKE search - vector search done separately
        let pattern = format!("%{}%", query);

        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Core))
            .filter(memory_node::Column::Content.like(&pattern))
            .order_by_desc(memory_node::Column::Importance)
            .limit(limit)
            .all(&self.db)
            .await?;

        Ok(nodes.into_iter().map(Self::node_to_entry).collect())
    }

    /// Count core memories for a user
    pub async fn count(&self, user_id: Uuid) -> anyhow::Result<u64> {
        let count = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Core))
            .count(&self.db)
            .await?;

        Ok(count)
    }

    /// Get top-N most important core memories
    pub async fn get_top(&self, user_id: Uuid, limit: u64) -> anyhow::Result<Vec<CoreEntry>> {
        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Core))
            .order_by_desc(memory_node::Column::Importance)
            .limit(limit)
            .all(&self.db)
            .await?;

        Ok(nodes.into_iter().map(Self::node_to_entry).collect())
    }

    /// Merge duplicate/similar core memories (consolidation)
    pub async fn merge(&self, primary_id: Uuid, secondary_ids: Vec<Uuid>) -> anyhow::Result<()> {
        // Delete secondary nodes, keeping primary
        for id in secondary_ids {
            memory_node::Entity::delete_by_id(id).exec(&self.db).await?;
        }

        // Bump importance of primary
        if let Some(node) = memory_node::Entity::find_by_id(primary_id)
            .one(&self.db)
            .await?
        {
            let mut active: memory_node::ActiveModel = node.into();
            let current_importance = active.importance.clone().unwrap();
            active.importance = Set((current_importance + 0.1).min(1.0));
            active.updated_at = Set(Utc::now());
            active.update(&self.db).await?;
        }

        Ok(())
    }

    fn node_to_entry(node: memory_node::Model) -> CoreEntry {
        let metadata = &node.metadata;
        let core_type = match node.node_type {
            NodeType::Preference => CoreType::Preference,
            NodeType::Fact => CoreType::Fact,
            NodeType::Person => CoreType::Person,
            NodeType::Entity => CoreType::Entity,
            _ => CoreType::Fact,
        };

        CoreEntry {
            id: node.id,
            user_id: node.user_id,
            core_type,
            content: node.content,
            data: metadata.get("data").cloned().unwrap_or(serde_json::Value::Null),
            importance: node.importance,
            source: metadata
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            created_at: node.created_at,
            updated_at: node.updated_at,
        }
    }
}
