//! SeaORM entity for skills table
//!
//! Skills extend what the LLM **knows how to do well**. Each skill carries
//! intent patterns for matching, a prompt template for domain context, and
//! a list of tool names available during LLM execution.
//!
//! Skills can be registered by:
//! - Built-in seeds (source = "builtin")
//! - Markdown files (source = "file")
//! - Plugins at runtime (source = plugin name)
//! - API (source = "api")

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
    /// Legacy: internal execution routing config. Use `tools` column instead.
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub execution_config: Option<serde_json::Value>,
    /// Tool names to make available to the LLM during this skill's execution.
    /// Stored as a JSON array of strings, e.g. ["device_control", "sensor_read"]
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub tools: Option<serde_json::Value>,
    /// Response template / system prompt for LLM context
    #[sea_orm(column_type = "Text", nullable)]
    pub prompt_template: Option<String>,
    pub cacheable: bool,
    pub enabled: bool,
    /// Higher priority wins on conflict
    pub priority: i32,
    /// Where this skill came from: 'builtin', 'file', 'api', or plugin name
    #[sea_orm(column_type = "String(StringLen::N(100))")]
    pub source: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
