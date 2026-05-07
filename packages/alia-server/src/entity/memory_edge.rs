//! SeaORM entity for memory_edges table
//!
//! Represents relationships between nodes in the GraphRAG knowledge graph.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Relation type enum matching database values
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(50))")]
pub enum RelationType {
    #[sea_orm(string_value = "prefers")]
    Prefers,
    #[sea_orm(string_value = "lives_in")]
    LivesIn,
    #[sea_orm(string_value = "owns")]
    Owns,
    #[sea_orm(string_value = "habit")]
    Habit,
    #[sea_orm(string_value = "related_to")]
    RelatedTo,
    #[sea_orm(string_value = "knows")]
    Knows,
    #[sea_orm(string_value = "part_of")]
    PartOf,
    #[sea_orm(string_value = "triggers")]
    Triggers,
    #[sea_orm(string_value = "causes")]
    Causes,
}

impl std::fmt::Display for RelationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelationType::Prefers => write!(f, "prefers"),
            RelationType::LivesIn => write!(f, "lives_in"),
            RelationType::Owns => write!(f, "owns"),
            RelationType::Habit => write!(f, "habit"),
            RelationType::RelatedTo => write!(f, "related_to"),
            RelationType::Knows => write!(f, "knows"),
            RelationType::PartOf => write!(f, "part_of"),
            RelationType::Triggers => write!(f, "triggers"),
            RelationType::Causes => write!(f, "causes"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "memory_edges")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: RelationType,
    pub weight: f32,
    #[sea_orm(column_type = "JsonBinary")]
    pub metadata: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::memory_node::Entity",
        from = "Column::SourceId",
        to = "super::memory_node::Column::Id"
    )]
    SourceNode,
    #[sea_orm(
        belongs_to = "super::memory_node::Entity",
        from = "Column::TargetId",
        to = "super::memory_node::Column::Id"
    )]
    TargetNode,
}

impl Related<super::memory_node::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SourceNode.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
