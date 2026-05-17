use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "conversation_events")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub session_id: Uuid,
    pub turn_id: Uuid,
    pub sequence: i32,
    pub event_type: String,
    pub actor: String,
    pub summary: Option<String>,
    #[sea_orm(column_type = "JsonBinary")]
    pub payload: serde_json::Value,
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
