//! Sensor system: registration, time-series storage, query, and auto-cleanup.
//!
//! Sensors can be reported from:
//! - Embedded devices (via gRPC → ChannelEvent::SensorData)
//! - Plugins (via ToolRegistry plugin tool registration)
//! - API (direct HTTP ingestion)
//!
//! The scheduler can use sensor values as conditions for task execution.
//! The LLM can read sensor values via the `sensor_read` built-in tool.

pub mod registry;

pub use registry::{SensorDefinition, SensorQuery, SensorReading, SensorRegistry, SensorRegistryConfig, SensorSummary};
