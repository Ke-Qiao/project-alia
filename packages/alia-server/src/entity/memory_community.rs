//! SeaORM entity for memory_communities table
//!
//! Represents topic clusters for GraphRAG global retrieval.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "memory_communities")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    #[sea_orm(column_type = "String(StringLen::N(200))")]
    pub title: String,
    #[sea_orm(column_type = "Text")]
    pub summary: String,
    /// Member node IDs stored as JSON array
    #[sea_orm(column_type = "JsonBinary")]
    pub node_ids: serde_json::Value,
    pub level: i32,
    /// Stored as raw bytes; actual vector ops done via raw SQL with pgvector
    #[sea_orm(nullable)]
    pub embedding: Option<Vec<u8>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// Parse node_ids JSON array to Vec<Uuid>
    pub fn get_node_ids(&self) -> Vec<Uuid> {
        self.node_ids
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok()))
                    .collect()
            })
            .unwrap_or_default()
    }
}
