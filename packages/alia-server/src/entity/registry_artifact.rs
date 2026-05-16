//! Registry index for tool/skill ownership.
//!
//! Concrete artifacts still live in their domain tables (`skills`, `tools`).
//! This table gives lifecycle code a single place to reason about ownership
//! and dependency-safe deletion.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "registry_artifacts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(column_type = "String(StringLen::N(20))")]
    pub artifact_kind: String,
    #[sea_orm(column_type = "String(StringLen::N(100))")]
    pub artifact_name: String,
    #[sea_orm(column_type = "String(StringLen::N(50))")]
    pub owner_kind: String,
    #[sea_orm(column_type = "String(StringLen::N(100))")]
    pub owner_name: String,
    #[sea_orm(column_type = "String(StringLen::N(100))")]
    pub source: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
