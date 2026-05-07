//! SeaORM entity for memory_nodes table
//!
//! Represents nodes in the GraphRAG knowledge graph.
//! Includes entities, facts, preferences, habits, events, and persons.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Memory layer enum matching database values
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(20))")]
pub enum MemoryLayer {
    #[sea_orm(string_value = "episodic")]
    Episodic,
    #[sea_orm(string_value = "core")]
    Core,
    #[sea_orm(string_value = "procedural")]
    Procedural,
}

impl std::fmt::Display for MemoryLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryLayer::Episodic => write!(f, "episodic"),
            MemoryLayer::Core => write!(f, "core"),
            MemoryLayer::Procedural => write!(f, "procedural"),
        }
    }
}

/// Node type enum matching database values
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(50))")]
pub enum NodeType {
    #[sea_orm(string_value = "entity")]
    Entity,
    #[sea_orm(string_value = "fact")]
    Fact,
    #[sea_orm(string_value = "preference")]
    Preference,
    #[sea_orm(string_value = "habit")]
    Habit,
    #[sea_orm(string_value = "event")]
    Event,
    #[sea_orm(string_value = "person")]
    Person,
    #[sea_orm(string_value = "summary")]
    Summary,
}

impl std::fmt::Display for NodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeType::Entity => write!(f, "entity"),
            NodeType::Fact => write!(f, "fact"),
            NodeType::Preference => write!(f, "preference"),
            NodeType::Habit => write!(f, "habit"),
            NodeType::Event => write!(f, "event"),
            NodeType::Person => write!(f, "person"),
            NodeType::Summary => write!(f, "summary"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "memory_nodes")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub memory_layer: MemoryLayer,
    pub node_type: NodeType,
    #[sea_orm(column_type = "Text")]
    pub content: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub metadata: serde_json::Value,
    /// Stored as raw bytes; actual vector ops done via raw SQL with pgvector
    #[sea_orm(nullable)]
    pub embedding: Option<Vec<u8>>,
    pub importance: f32,
    pub access_count: i32,
    pub last_accessed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::memory_edge::Entity", on_delete = "Cascade")]
    OutgoingEdges,
}

impl Related<super::memory_edge::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OutgoingEdges.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
