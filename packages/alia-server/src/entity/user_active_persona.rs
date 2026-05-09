//! SeaORM entity for user_active_persona table
//!
//! Tracks which persona each user has selected as their active persona.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_active_persona")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    pub persona_id: Uuid,
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
