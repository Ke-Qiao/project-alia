//! L3 Procedural Memory - Habits and Behavior Patterns
//!
//! System-maintained memory for learned user behaviors and routines.
//! Unlike core memories (user-editable), procedural memories are
//! automatically extracted and updated by pattern recognition.
//!
//! Examples:
//! - "Usually wakes up at 7:00 AM on weekdays"
//! - "Prefers dim lights after 10 PM"
//! - "Asks about weather before leaving home"

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::memory_node::{self, MemoryLayer, NodeType};

/// Habit frequency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Frequency {
    Daily,
    Weekdays,
    Weekends,
    Weekly,
    Occasional,
}

impl std::fmt::Display for Frequency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Frequency::Daily => write!(f, "daily"),
            Frequency::Weekdays => write!(f, "weekdays"),
            Frequency::Weekends => write!(f, "weekends"),
            Frequency::Weekly => write!(f, "weekly"),
            Frequency::Occasional => write!(f, "occasional"),
        }
    }
}

/// Time of day for habit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeWindow {
    /// Start hour (0-23)
    pub start_hour: u8,
    /// End hour (0-23)
    pub end_hour: u8,
}

/// Procedural memory entry (habit/pattern)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitEntry {
    pub id: Uuid,
    pub user_id: Uuid,
    /// Natural language description of the habit
    pub description: String,
    /// Pattern category (e.g., "sleep", "work", "entertainment")
    pub category: String,
    /// How often this pattern occurs
    pub frequency: Frequency,
    /// Time window when this typically happens
    pub time_window: Option<TimeWindow>,
    /// Trigger conditions (what prompts this behavior)
    pub triggers: Vec<String>,
    /// Associated actions/commands
    pub actions: Vec<String>,
    /// Confidence score (0.0 - 1.0) based on observation count
    pub confidence: f32,
    /// Number of times this pattern was observed
    pub observation_count: i32,
    /// Last time this pattern was observed
    pub last_observed: DateTime<Utc>,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
}

/// L3 Procedural Memory Manager
pub struct ProceduralMemory {
    db: DatabaseConnection,
    /// Minimum confidence to keep a habit (garbage collection)
    min_confidence: f32,
    /// Maximum habits per user
    max_per_user: u64,
}

impl ProceduralMemory {
    pub fn new(db: DatabaseConnection) -> Self {
        Self {
            db,
            min_confidence: 0.3,
            max_per_user: 200,
        }
    }

    /// Store or update a habit pattern
    pub async fn record_habit(&self, entry: HabitEntry) -> anyhow::Result<Uuid> {
        // Check if similar habit exists (by category + time window)
        let existing = self.find_similar(&entry).await?;

        if let Some(existing_id) = existing {
            // Reinforce existing habit
            self.reinforce(existing_id).await?;
            return Ok(existing_id);
        }

        // Check limit
        let count = self.count(entry.user_id).await?;
        if count >= self.max_per_user {
            // Remove lowest confidence habit
            self.prune_lowest(entry.user_id).await?;
        }

        let metadata = serde_json::json!({
            "category": entry.category,
            "frequency": entry.frequency.to_string(),
            "time_window": entry.time_window,
            "triggers": entry.triggers,
            "actions": entry.actions,
            "observation_count": entry.observation_count,
            "last_observed": entry.last_observed.to_rfc3339(),
        });

        let model = memory_node::ActiveModel {
            id: Set(entry.id),
            user_id: Set(entry.user_id),
            memory_layer: Set(MemoryLayer::Procedural),
            node_type: Set(NodeType::Habit),
            content: Set(entry.description),
            metadata: Set(metadata),
            embedding: Set(None),
            importance: Set(entry.confidence),
            access_count: Set(entry.observation_count),
            last_accessed_at: Set(Some(entry.last_observed)),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            expires_at: Set(None),
        };

        model.insert(&self.db).await?;
        Ok(entry.id)
    }

    /// Reinforce an existing habit (increase confidence)
    pub async fn reinforce(&self, id: Uuid) -> anyhow::Result<()> {
        let node = memory_node::Entity::find_by_id(id).one(&self.db).await?;

        if let Some(node) = node {
            let mut active: memory_node::ActiveModel = node.clone().into();

            // Increase confidence (diminishing returns)
            let current = node.importance;
            let new_confidence = current + (1.0 - current) * 0.1;
            active.importance = Set(new_confidence.min(1.0));

            // Update observation count
            let mut metadata = node.metadata.clone();
            let count = metadata
                .get("observation_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            metadata["observation_count"] = serde_json::json!(count + 1);
            metadata["last_observed"] = serde_json::json!(Utc::now().to_rfc3339());
            active.metadata = Set(metadata);

            active.access_count = Set(node.access_count + 1);
            active.last_accessed_at = Set(Some(Utc::now()));
            active.updated_at = Set(Utc::now());

            active.update(&self.db).await?;
        }

        Ok(())
    }

    /// Decay confidence of unused habits
    pub async fn decay_unused(&self, days_threshold: i64, decay_rate: f32) -> anyhow::Result<u64> {
        let cutoff = Utc::now() - chrono::Duration::days(days_threshold);

        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Procedural))
            .filter(memory_node::Column::LastAccessedAt.lte(cutoff))
            .all(&self.db)
            .await?;

        let mut updated = 0;
        for node in nodes {
            let mut active: memory_node::ActiveModel = node.clone().into();
            let new_importance = (node.importance * (1.0 - decay_rate)).max(0.0);

            if new_importance < self.min_confidence {
                // Delete if below threshold
                memory_node::Entity::delete_by_id(node.id)
                    .exec(&self.db)
                    .await?;
            } else {
                active.importance = Set(new_importance);
                active.updated_at = Set(Utc::now());
                active.update(&self.db).await?;
            }
            updated += 1;
        }

        Ok(updated)
    }

    /// Get habits for a specific time window
    pub async fn get_for_time(&self, user_id: Uuid, hour: u8) -> anyhow::Result<Vec<HabitEntry>> {
        // Get all habits and filter by time window
        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Procedural))
            .order_by_desc(memory_node::Column::Importance)
            .all(&self.db)
            .await?;

        let habits: Vec<HabitEntry> = nodes
            .into_iter()
            .map(Self::node_to_entry)
            .filter(|h| {
                h.time_window.as_ref().map_or(true, |tw| {
                    if tw.start_hour <= tw.end_hour {
                        hour >= tw.start_hour && hour <= tw.end_hour
                    } else {
                        // Wraps around midnight
                        hour >= tw.start_hour || hour <= tw.end_hour
                    }
                })
            })
            .collect();

        Ok(habits)
    }

    /// Get habits by category
    pub async fn get_by_category(
        &self,
        user_id: Uuid,
        category: &str,
    ) -> anyhow::Result<Vec<HabitEntry>> {
        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Procedural))
            .order_by_desc(memory_node::Column::Importance)
            .all(&self.db)
            .await?;

        let habits: Vec<HabitEntry> = nodes
            .into_iter()
            .map(Self::node_to_entry)
            .filter(|h| h.category == category)
            .collect();

        Ok(habits)
    }

    /// Get all habits for a user
    pub async fn get_all(&self, user_id: Uuid) -> anyhow::Result<Vec<HabitEntry>> {
        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Procedural))
            .order_by_desc(memory_node::Column::Importance)
            .all(&self.db)
            .await?;

        Ok(nodes.into_iter().map(Self::node_to_entry).collect())
    }

    /// Count habits for a user
    pub async fn count(&self, user_id: Uuid) -> anyhow::Result<u64> {
        let count = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Procedural))
            .count(&self.db)
            .await?;

        Ok(count)
    }

    /// Find similar habit by category and time window
    async fn find_similar(&self, entry: &HabitEntry) -> anyhow::Result<Option<Uuid>> {
        let nodes = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(entry.user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Procedural))
            .all(&self.db)
            .await?;

        for node in nodes {
            let metadata = &node.metadata;
            let category = metadata.get("category").and_then(|v| v.as_str());

            if category == Some(&entry.category) {
                // Check time window overlap
                if let (Some(existing_tw), Some(new_tw)) =
                    (metadata.get("time_window"), &entry.time_window)
                {
                    let existing_start = existing_tw.get("start_hour").and_then(|v| v.as_u64());
                    let existing_end = existing_tw.get("end_hour").and_then(|v| v.as_u64());

                    if let (Some(es), Some(ee)) = (existing_start, existing_end) {
                        // Simple overlap check
                        if (es as u8..=ee as u8).contains(&new_tw.start_hour)
                            || (es as u8..=ee as u8).contains(&new_tw.end_hour)
                        {
                            return Ok(Some(node.id));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    /// Remove lowest confidence habit
    async fn prune_lowest(&self, user_id: Uuid) -> anyhow::Result<()> {
        let node = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::MemoryLayer.eq(MemoryLayer::Procedural))
            .order_by_asc(memory_node::Column::Importance)
            .one(&self.db)
            .await?;

        if let Some(node) = node {
            memory_node::Entity::delete_by_id(node.id)
                .exec(&self.db)
                .await?;
        }

        Ok(())
    }

    fn node_to_entry(node: memory_node::Model) -> HabitEntry {
        let metadata = &node.metadata;

        let frequency = match metadata.get("frequency").and_then(|v| v.as_str()) {
            Some("daily") => Frequency::Daily,
            Some("weekdays") => Frequency::Weekdays,
            Some("weekends") => Frequency::Weekends,
            Some("weekly") => Frequency::Weekly,
            _ => Frequency::Occasional,
        };

        let time_window = metadata.get("time_window").and_then(|tw| {
            let start = tw.get("start_hour")?.as_u64()? as u8;
            let end = tw.get("end_hour")?.as_u64()? as u8;
            Some(TimeWindow {
                start_hour: start,
                end_hour: end,
            })
        });

        let triggers = metadata
            .get("triggers")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let actions = metadata
            .get("actions")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        HabitEntry {
            id: node.id,
            user_id: node.user_id,
            description: node.content,
            category: metadata
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("general")
                .to_string(),
            frequency,
            time_window,
            triggers,
            actions,
            confidence: node.importance,
            observation_count: node.access_count,
            last_observed: node.last_accessed_at.unwrap_or(node.created_at),
            created_at: node.created_at,
        }
    }
}
