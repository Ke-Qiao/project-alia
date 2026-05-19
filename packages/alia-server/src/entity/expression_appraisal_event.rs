use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "expression_appraisal_events")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub channel_id: String,
    pub user_id: String,
    pub route: Option<String>,
    pub kind: String,
    pub source: String,
    pub target: String,
    pub scope: String,
    pub confidence: f32,
    pub ttl_sec: i64,
    #[sea_orm(column_type = "JsonBinary")]
    pub decay: serde_json::Value,
    #[sea_orm(column_type = "JsonBinary")]
    pub deltas: serde_json::Value,
    #[sea_orm(column_type = "JsonBinary")]
    pub evidence: serde_json::Value,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
