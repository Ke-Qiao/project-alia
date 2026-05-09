//! SeaORM entity for personas table
//!
//! Represents AI personality definitions with system prompt templates,
//! personality traits, voice configuration, and behavior guidelines.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "personas")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub name: String,
    pub display_name: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub description: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub system_prompt_template: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub personality_traits: serde_json::Value,
    #[sea_orm(column_type = "JsonBinary")]
    pub voice_config: serde_json::Value,
    #[sea_orm(column_type = "Text", nullable)]
    pub greeting: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub behavior_guidelines: Option<String>,
    pub is_system: bool,
    pub enabled: bool,
    pub sort_order: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::user_persona_override::Entity")]
    UserPersonaOverrides,
}

impl Related<super::user_persona_override::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::UserPersonaOverrides.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
