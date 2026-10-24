//! The data-only contract shared by Rust hosts and WASM guests.
use serde::{Deserialize, Serialize};

pub const ABI_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub memory_pages: u32,
    pub fuel: u64,
    pub initialization_fuel: u64,
    pub timeout_ms: u64,
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_pages: 256,
            fuel: 20_000_000,
            initialization_fuel: 2_000_000,
            timeout_ms: 1_000,
            max_input_bytes: 256 * 1024,
            max_output_bytes: 256 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub export: String,
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub require_confirmation: bool,
    pub required_role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessorRequest {
    pub instance_id: String,
    pub command_id: String,
    pub action: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessorResponse {
    pub success: bool,
    pub result: serde_json::Value,
}
