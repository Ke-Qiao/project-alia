//! Server-owned JSON document storage for plugin-private state.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "plugin_storage_records")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(column_type = "String(StringLen::N(100))")]
    pub plugin_name: String,
    #[sea_orm(column_type = "String(StringLen::N(100))")]
    pub collection: String,
    #[sea_orm(column_type = "String(StringLen::N(200))")]
    pub key: String,
    #[sea_orm(column_type = "String(StringLen::N(30))")]
    pub scope: String,
    #[sea_orm(column_type = "String(StringLen::N(200))")]
    pub owner_id: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub value: serde_json::Value,
    pub schema_version: i32,
    pub version: i64,
    pub ttl_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
