use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "meditation_logs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    /// 'extraction' | 'consolidation' | 'reflection' | 'habit_detection'
    #[sea_orm(column_type = "String(StringLen::N(20))")]
    pub phase: String,
    /// 'post_conversation' | 'idle' | 'sensor' | 'schedule' | 'threshold' | 'manual'
    #[sea_orm(column_type = "String(StringLen::N(20))")]
    pub trigger_type: String,
    #[sea_orm(column_type = "Text")]
    pub summary: String,
    pub nodes_created: i32,
    pub nodes_updated: i32,
    pub nodes_merged: i32,
    pub edges_created: i32,
    #[sea_orm(column_type = "String(StringLen::N(50))", nullable)]
    pub model_used: Option<String>,
    #[sea_orm(nullable)]
    pub duration_ms: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
