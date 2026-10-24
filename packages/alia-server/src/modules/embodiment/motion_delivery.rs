//! Durable motion delivery facts share the canonical Response, without using
//! audio counters to describe servo commands or treating recovery as replay.

use alia_core::embodiment::motion::{MotionCommand, MotionProgress};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod repository;
pub use repository::MotionDeliveryRepository;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotionDeliveryOrigin {
    pub response_id: Uuid,
    pub output_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MotionDeliveryRecord {
    pub delivery_id: Uuid,
    pub response_id: Uuid,
    pub output_key: String,
    pub dispatch_id: Uuid,
    #[schema(value_type = serde_json::Value)]
    pub command: MotionCommand,
    #[schema(value_type = Option<serde_json::Value>)]
    pub progress: Option<MotionProgress>,
    /// A later device fact does not rewrite the frozen Unknown outcome.
    #[schema(value_type = Option<serde_json::Value>)]
    pub latest_checkpoint: Option<MotionProgress>,
    pub status: String,
    pub unknown_reason: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum MotionLookupError {
    #[error("motion delivery or response not found")]
    NotFound,
    #[error("motion owner is unavailable")]
    Unauthorized,
}

#[derive(Debug, Clone)]
pub enum MotionDispatch {
    Fresh {
        delivery_id: Uuid,
        dispatch_id: Uuid,
    },
    Existing(MotionDeliveryRecord),
}
