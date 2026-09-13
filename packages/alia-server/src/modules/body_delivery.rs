//! Durable body delivery facts share the canonical Response, without using
//! audio counters to describe servo commands or treating recovery as replay.

use alia_core::body::{BodyCommand, BodyProgress};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod repository;
pub use repository::BodyDeliveryRepository;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyDeliveryOrigin {
    pub response_id: Uuid,
    pub output_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BodyDeliveryRecord {
    pub delivery_id: Uuid,
    pub response_id: Uuid,
    pub output_key: String,
    pub dispatch_id: Uuid,
    #[schema(value_type = serde_json::Value)]
    pub command: BodyCommand,
    #[schema(value_type = Option<serde_json::Value>)]
    pub progress: Option<BodyProgress>,
    /// A later device fact does not rewrite the frozen Unknown outcome.
    #[schema(value_type = Option<serde_json::Value>)]
    pub latest_checkpoint: Option<BodyProgress>,
    pub status: String,
    pub unknown_reason: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BodyLookupError {
    #[error("body delivery or response not found")]
    NotFound,
    #[error("body owner is unavailable")]
    Unauthorized,
}

#[derive(Debug, Clone)]
pub enum BodyDispatch {
    Fresh {
        delivery_id: Uuid,
        dispatch_id: Uuid,
    },
    Existing(BodyDeliveryRecord),
}
