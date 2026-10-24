//! Tools module: extend what the LLM **can do**.
//!
//! Tools are executable functions the LLM can call via function calling.
//! Skills can bind tools as context, but do not execute them directly. Tools
//! provide the actual actions — reading sensors, controlling
//! devices, managing memory, etc.
//!
//! This module provides:
//! - `BuiltinToolRegistry`: Full implementation with built-in tools and plugin support
//! - Sensor tools: `sensor_read`, `sensor_list`

pub mod builtin;
pub mod entry;
#[cfg(feature = "tools-http")]
pub mod fetch;
pub mod mcp;
pub mod sensor;
pub mod wasm;

pub use builtin::BuiltinToolRegistry;
