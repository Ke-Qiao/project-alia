//! SeaORM entity for skill/preferred-tool bindings.
//!
//! Skills provide cognitive context; tools provide executable functions. This
//! junction table stores soft tool preferences, not tool visibility rules.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "skill_preferred_tools")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub skill_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub tool_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::skill::Entity",
        from = "Column::SkillId",
        to = "super::skill::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Skill,
    #[sea_orm(
        belongs_to = "super::tool::Entity",
        from = "Column::ToolId",
        to = "super::tool::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Tool,
}

impl Related<super::skill::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Skill.def()
    }
}

impl Related<super::tool::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Tool.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
