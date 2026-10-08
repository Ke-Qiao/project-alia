//! Insights scheduling owns admission, event routing, and bounded workers.

mod service;
mod types;

pub use service::InsightService;
pub use types::{InsightPhase, InsightStatus, InsightTask};
