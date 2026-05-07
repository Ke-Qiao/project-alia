//! SeaORM entity for llm_cache table
//!
//! Stores cached LLM responses with support for exact hash matching
//! and semantic similarity matching via pgvector embeddings.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "llm_cache")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    #[sea_orm(column_type = "String(StringLen::N(100))")]
    pub scope: String,
    #[sea_orm(column_type = "String(StringLen::N(64))")]
    pub query_hash: String,
    #[sea_orm(column_type = "Text")]
    pub query_text: String,
    #[sea_orm(column_type = "Text")]
    pub response_text: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub metadata: serde_json::Value,
    /// Stored as raw bytes; actual vector ops done via raw SQL with pgvector
    #[sea_orm(nullable)]
    pub embedding: Option<Vec<u8>>,
    pub hit_count: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
