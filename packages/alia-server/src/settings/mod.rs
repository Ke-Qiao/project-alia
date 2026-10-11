//! Settings Registry: runtime-mutable settings with namespace isolation.
//!
//! Provides:
//! - `SettingsRegistry`: in-memory typed key-value store
//! - Namespace support: `core.*` for server settings, `plugin.<name>.*` for plugins
//! - TOML serialization for persistence
//! - Schema declarations for frontend rendering

pub mod context_policy;
pub mod registry;
mod storage;
pub mod values;
pub use registry::SettingsRegistry;
pub use storage::SettingsSaveError;
