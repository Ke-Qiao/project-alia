//! SeaORM entity for tools table
//!
//! Tools extend what the LLM **can do**. Each tool is an executable function
//! that the LLM can call via function calling. Tools can be builtin or
//! registered by plugins at runtime.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "tools")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub name: String,
    #[sea_orm(column_type = "Text")]
    pub description: String,
    /// Parameter definitions (JSON Schema)
    #[sea_orm(column_type = "JsonBinary")]
    pub parameters: serde_json::Value,
    /// Source: 'builtin' or plugin name
    #[sea_orm(column_type = "String(StringLen::N(100))")]
    pub source: String,
    pub require_confirmation: bool,
    #[sea_orm(column_type = "String(StringLen::N(50))", nullable)]
    pub required_role: Option<String>,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
