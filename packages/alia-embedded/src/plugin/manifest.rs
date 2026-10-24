use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// Plugin manifest parsed from `manifest.toml` inside each plugin directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub plugin: PluginSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSection {
    pub name: String,
    pub version: String,
    #[serde(rename = "type")]
    pub plugin_type: PluginType,
    pub entry: String,
    pub runtime: PluginRuntime,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub wasm_limits: alia_plugin_api::Limits,
    #[serde(default)]
    pub capabilities: PluginCapabilities,
    #[serde(default)]
    pub events: PluginEvents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginType {
    Sensor,
    Actuator,
    Processor,
    Bridge,
}

impl fmt::Display for PluginType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sensor => write!(f, "sensor"),
            Self::Actuator => write!(f, "actuator"),
            Self::Processor => write!(f, "processor"),
            Self::Bridge => write!(f, "bridge"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginRuntime {
    Wasm,
    /// Explicitly authorized device driver, outside the WASM security boundary.
    TrustedNative,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginCapabilities {
    #[serde(default)]
    pub gpio: bool,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub usb: bool,
    /// Expression actions this plugin can execute, such as `small_nod`.
    ///
    /// These names are advertised to the server as
    /// `expression.action.<name>` capabilities. The server treats motion
    /// support as unavailable unless the action is explicitly declared here.
    #[serde(default)]
    pub expression_actions: Vec<String>,
    /// Runtime expression state claims advertised to the server.
    ///
    /// Supported values are intentionally small and conservative:
    /// `ready`, `busy`, `thermal_limit`, and `battery_saver`.
    #[serde(default)]
    pub expression_state: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginEvents {
    #[serde(default)]
    pub publishes: Vec<String>,
    #[serde(default)]
    pub subscribes: Vec<String>,
}

impl PluginManifest {
    /// Parse a manifest from a TOML string.
    pub fn parse(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    /// Load a manifest from a file path.
    pub fn load(path: &PathBuf) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::parse(&content)?)
    }
}
