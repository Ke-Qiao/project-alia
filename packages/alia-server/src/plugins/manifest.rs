use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::storage::{validate_collection_name, validate_plugin_name};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginStorageManifest {
    #[serde(default)]
    pub collections: Vec<PluginStorageCollection>,
}

impl PluginStorageManifest {
    pub fn validate(&self, plugin_name: &str) -> anyhow::Result<()> {
        validate_plugin_name(plugin_name)?;
        let mut names = HashSet::new();
        for collection in &self.collections {
            collection.validate()?;
            if !names.insert(collection.name.clone()) {
                anyhow::bail!("duplicate plugin storage collection '{}'", collection.name);
            }
        }
        Ok(())
    }

    pub fn collection(&self, name: &str) -> Option<&PluginStorageCollection> {
        self.collections
            .iter()
            .find(|collection| collection.name == name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginStorageCollection {
    pub name: String,
    #[serde(default)]
    pub scope: Vec<PluginStorageScopeKind>,
    #[serde(default = "default_schema_version")]
    pub schema_version: i32,
    #[serde(default)]
    pub retention: PluginStorageRetention,
    #[serde(default)]
    pub ttl_sec: Option<u64>,
    #[serde(default)]
    pub max_record_bytes: Option<usize>,
}

impl PluginStorageCollection {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_collection_name(&self.name)?;
        if matches!(self.name.as_str(), "secret" | "secrets") {
            anyhow::bail!(
                "plugin storage collection '{}' is reserved; use the server secret store",
                self.name
            );
        }
        if self.scope.is_empty() {
            anyhow::bail!(
                "plugin storage collection '{}' must declare at least one scope",
                self.name
            );
        }
        let mut scopes = HashSet::new();
        for scope in &self.scope {
            if !scopes.insert(scope) {
                anyhow::bail!(
                    "plugin storage collection '{}' declares duplicate scope '{}'",
                    self.name,
                    scope
                );
            }
        }
        if self.schema_version < 1 {
            anyhow::bail!(
                "plugin storage collection '{}' must use schema_version >= 1",
                self.name
            );
        }
        if self.ttl_sec == Some(0) {
            anyhow::bail!(
                "plugin storage collection '{}' ttl_sec must be greater than zero",
                self.name
            );
        }
        if self.retention == PluginStorageRetention::ExportRequired && self.ttl_sec.is_some() {
            anyhow::bail!(
                "plugin storage collection '{}' cannot combine export_required retention with ttl_sec",
                self.name
            );
        }
        Ok(())
    }

    pub fn ttl(&self) -> Option<Duration> {
        self.ttl_sec.map(Duration::from_secs)
    }

    pub fn allows_scope(&self, scope: PluginStorageScopeKind) -> bool {
        self.scope.contains(&scope)
    }
}

fn default_schema_version() -> i32 {
    1
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStorageScopeKind {
    Global,
    User,
    Channel,
    Conversation,
}

impl PluginStorageScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::User => "user",
            Self::Channel => "channel",
            Self::Conversation => "conversation",
        }
    }
}

impl fmt::Display for PluginStorageScopeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStorageRetention {
    RetainOnUninstall,
    DeleteOnUninstall,
    ExportRequired,
}

impl Default for PluginStorageRetention {
    fn default() -> Self {
        Self::RetainOnUninstall
    }
}
