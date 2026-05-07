//! L1 Episodic Memory - Conversation Summaries
//!
//! Database-backed storage for conversation summaries and events.
//! Features time-based retrieval and semantic search over past interactions.
//!
//! Lifecycle:
//! - Created when L0 working memory is summarized (session end or window full)
//! - Has configurable TTL (default 90 days)
//! - Can be promoted to L2 core memory during meditation/consolidation

use chrono::{DateTime, Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::memory_node::{self, MemoryLayer, NodeType};

/// Default TTL for episodic memories (90 days)
const DEFAULT_TTL_DAYS: i64 = 90;

/// Episodic memory entry (conversation summary)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodicEntry {
    pub id: Uuid,
    pub user_id: Uuid,
    /// Summary of the conversation
    pub summary: String,
    /// Main topic/theme
    pub topic: Option<String>,
    /// Emotional tone (extracted by LLM)
    pub sentiment: Option<String>,
    /// Key entities mentioned
    pub entities: Vec<String>,
    /// Number of turns in original conversation
    pub turn_count: i32,
    /// Session start time
    pub session_start: DateTime<Utc>,
    /// Session end time
    pub session_end: DateTime<Utc>,
    /// Importance score (0.0 - 1.0)
    pub importance: f32,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Expiration timestamp
    pub expires_at: DateTime<Utc>,
}

/// L1 Episodic Memory Manager
///
/// Manages conversation summaries in the database.
pub struct EpisodicMemory {
    db: DatabaseConnection,
    ttl_days: i64,
}

impl EpisodicMemory {
    pub fn new(db: DatabaseConnection) -> Self {
        Self {
            db,
            ttl_days: DEFAULT_TTL_DAYS,
        }
    }

    pub fn with_ttl(mut self, days: i64) -> Self {
        self.ttl_days = days;
        self
    }

    /// Store a new episodic memory from conversation summary
    pub async fn store(&self, user_id: Uuid, entry: EpisodicEntry) -> anyhow::Result<Uuid> {
        let id = entry.id;
        let expires_at = Utc::now() + Duration::days(self.ttl_days);

        // Build metadata JSON
        let metadata = serde_json::json!({
            "topic": entry.topic,
            "sentiment": entry.sentiment,
            "entities": entry.entities,
            "turn_count": entry.turn_count,
            "session_start": entry.session_start.to_rfc3339(),
            "session_end": entry.session_end.to_rfc3339(),
        });

        let model = memory_node::ActiveModel {
            id: Set(id),
            user_id: Set(user_id),
            memory_layer: Set(MemoryLayer::Episodic),
            node_type: Set(NodeType::Event),
            content: Set(entry.summary),
            metadata: Set(metadata),
            embedding: Set(None), // Set later via update_embedding
            importance: Set(entry.importance),
            access_count: Set(0),
            last_accessed_at: Set(None),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            expires_at: Set(Some(expires_at)),
        };

        model.insert(&self.db).await?;
        Ok(id)
    }

    /// Retrieve episodic memories by time range
    pub async fn get_by_time_range(
        &self,
        user_id: Uuid,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: u64,
    ) -> anyhow::Result<Vec<EpisodicEntry>> {
        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Episodic))
            .filter(memory_node::Column::CreatedAt.gte(start))
            .filter(memory_node::Column::CreatedAt.lte(end))
            .order_by_desc(memory_node::Column::CreatedAt)
            .limit(limit)
            .all(&self.db)
            .await?;

        Ok(nodes.into_iter().map(Self::node_to_entry).collect())
    }

    /// Get recent episodic memories
    pub async fn get_recent(
        &self,
        user_id: Uuid,
        limit: u64,
    ) -> anyhow::Result<Vec<EpisodicEntry>> {
        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Episodic))
            .order_by_desc(memory_node::Column::CreatedAt)
            .limit(limit)
            .all(&self.db)
            .await?;

        Ok(nodes.into_iter().map(Self::node_to_entry).collect())
    }

    /// Get episodic memory by ID
    pub async fn get_by_id(&self, id: Uuid) -> anyhow::Result<Option<EpisodicEntry>> {
        let node = memory_node::Entity::find_by_id(id).one(&self.db).await?;

        Ok(node.map(Self::node_to_entry))
    }

    /// Update importance score (after access or reinforcement)
    pub async fn update_importance(&self, id: Uuid, importance: f32) -> anyhow::Result<()> {
        let node = memory_node::Entity::find_by_id(id).one(&self.db).await?;

        if let Some(node) = node {
            let mut active: memory_node::ActiveModel = node.into();
            active.importance = Set(importance);
            active.access_count = Set(active.access_count.unwrap() + 1);
            active.last_accessed_at = Set(Some(Utc::now()));
            active.updated_at = Set(Utc::now());
            active.update(&self.db).await?;
        }

        Ok(())
    }

    /// Delete expired episodic memories
    pub async fn cleanup_expired(&self) -> anyhow::Result<u64> {
        let result = memory_node::Entity::delete_many()
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Episodic))
            .filter(memory_node::Column::ExpiresAt.lte(Utc::now()))
            .exec(&self.db)
            .await?;

        Ok(result.rows_affected)
    }

    /// Count episodic memories for a user
    pub async fn count(&self, user_id: Uuid) -> anyhow::Result<u64> {
        let count = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Episodic))
            .count(&self.db)
            .await?;

        Ok(count)
    }

    /// Get episodic memories pending consolidation (high importance, old enough)
    pub async fn get_pending_consolidation(
        &self,
        user_id: Uuid,
        min_importance: f32,
        min_age_hours: i64,
        limit: u64,
    ) -> anyhow::Result<Vec<EpisodicEntry>> {
        let cutoff = Utc::now() - Duration::hours(min_age_hours);

        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Episodic))
            .filter(memory_node::Column::Importance.gte(min_importance))
            .filter(memory_node::Column::CreatedAt.lte(cutoff))
            .order_by_desc(memory_node::Column::Importance)
            .limit(limit)
            .all(&self.db)
            .await?;

        Ok(nodes.into_iter().map(Self::node_to_entry).collect())
    }

    /// Convert database model to EpisodicEntry
    fn node_to_entry(node: memory_node::Model) -> EpisodicEntry {
        let metadata = &node.metadata;

        EpisodicEntry {
            id: node.id,
            user_id: node.user_id,
            summary: node.content,
            topic: metadata.get("topic").and_then(|v| v.as_str()).map(String::from),
            sentiment: metadata.get("sentiment").and_then(|v| v.as_str()).map(String::from),
            entities: metadata
                .get("entities")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            turn_count: metadata.get("turn_count").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
            session_start: metadata
                .get("session_start")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or(node.created_at),
            session_end: metadata
                .get("session_end")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or(node.created_at),
            importance: node.importance,
            created_at: node.created_at,
            expires_at: node.expires_at.unwrap_or(node.created_at + Duration::days(DEFAULT_TTL_DAYS)),
        }
    }
}
