//! Registry dependency edges between tools and skills.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "registry_dependencies")]
pub struct Model {
    #[sea_orm(
        primary_key,
        auto_increment = false,
        column_type = "String(StringLen::N(20))"
    )]
    pub dependent_kind: String,
    #[sea_orm(
        primary_key,
        auto_increment = false,
        column_type = "String(StringLen::N(100))"
    )]
    pub dependent_name: String,
    #[sea_orm(
        primary_key,
        auto_increment = false,
        column_type = "String(StringLen::N(20))"
    )]
    pub dependency_kind: String,
    #[sea_orm(
        primary_key,
        auto_increment = false,
        column_type = "String(StringLen::N(100))"
    )]
    pub dependency_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
