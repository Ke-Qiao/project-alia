//! SeaORM entity for user_persona_overrides table
//!
//! Per-user partial overrides for persona fields (greeting, behavior, traits, voice).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_persona_overrides")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub persona_id: Uuid,
    #[sea_orm(column_type = "Text", nullable)]
    pub greeting: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub behavior_guidelines: Option<String>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub personality_traits: Option<serde_json::Value>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub voice_config: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::persona::Entity",
        from = "Column::PersonaId",
        to = "super::persona::Column::Id"
    )]
    Persona,
}

impl Related<super::persona::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Persona.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
