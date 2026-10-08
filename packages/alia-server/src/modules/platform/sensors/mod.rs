//! Shared sensor plane: definitions, fresh reads, persistence, and owned cleanup.

pub mod registry;
mod types;

pub use registry::SensorRegistry;
pub use types::*;
