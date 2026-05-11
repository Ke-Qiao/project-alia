use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "chat_messages")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub session_id: Uuid,
    pub user_id: Option<String>,
    pub role: String,
    pub content: String,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::chat_session::Entity",
        from = "Column::SessionId",
        to = "super::chat_session::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    ChatSession,
}

impl Related<super::chat_session::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ChatSession.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
