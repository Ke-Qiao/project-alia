//! SeaORM entity for skills table
//!
//! Represents structured skill definitions that map user intents
//! to deterministic actions without full LLM reasoning.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "skills")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub name: String,
    pub display_name: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub description: Option<String>,
    #[sea_orm(column_type = "String(StringLen::N(50))")]
    pub category: String,
    /// Intent matching patterns/keywords
    #[sea_orm(column_type = "JsonBinary")]
    pub intent_patterns: serde_json::Value,
    /// Parameter definitions (JSON Schema)
    #[sea_orm(column_type = "JsonBinary")]
    pub parameters: serde_json::Value,
    /// Internal execution routing config (plugin, action)
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub execution_config: Option<serde_json::Value>,
    /// Response template for simple confirmations
    #[sea_orm(column_type = "Text", nullable)]
    pub prompt_template: Option<String>,
    pub cacheable: bool,
    pub enabled: bool,
    /// Higher priority wins on conflict
    pub priority: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
