use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::manifest::PluginManifest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    Installed,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

pub struct PluginEntry {
    pub manifest: PluginManifest,
    pub state: PluginState,
    pub dir: PathBuf,
    pub pid: Option<u32>,
}

pub struct PluginRegistry {
    plugins: RwLock<HashMap<String, PluginEntry>>,
    plugins_dir: PathBuf,
}

impl PluginRegistry {
    pub fn new(plugins_dir: PathBuf) -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
            plugins_dir,
        }
    }

    /// Scan the plugins directory and load all manifests.
    pub async fn scan(&self) -> Result<()> {
        if !self.plugins_dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&self.plugins_dir).await?;
        let mut plugins = self.plugins.write().await;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let manifest_path = path.join("manifest.toml");
            if !manifest_path.exists() {
                continue;
            }

            match PluginManifest::load(&manifest_path) {
                Ok(manifest) => {
                    let name = manifest.plugin.name.clone();
                    info!(name = %name, "discovered plugin");
                    plugins.insert(
                        name,
                        PluginEntry {
                            manifest,
                            state: PluginState::Installed,
                            dir: path,
                            pid: None,
                        },
                    );
                }
                Err(e) => {
                    warn!(
                        path = %manifest_path.display(),
                        error = %e,
                        "failed to load plugin manifest"
                    );
                }
            }
        }

        Ok(())
    }

    /// Install a plugin from a zip archive.
    ///
    /// Validates zip structure (must contain `manifest.toml` and entry script)
    /// and guards against path traversal attacks.
    pub async fn install(&self, name: &str, zip_data: &[u8]) -> Result<()> {
        let plugin_dir = self.plugins_dir.join(name);

        // Create plugin directory
        tokio::fs::create_dir_all(&plugin_dir).await?;

        // Extract zip in a blocking task (zip I/O is sync)
        let plugin_dir_clone = plugin_dir.clone();
        let zip_data = zip_data.to_vec();
        let manifest = tokio::task::spawn_blocking(move || -> Result<PluginManifest> {
            let cursor = std::io::Cursor::new(&zip_data);
            let mut archive = zip::ZipArchive::new(cursor)?;

            let canonical_base = plugin_dir_clone
                .canonicalize()
                .unwrap_or(plugin_dir_clone.clone());

            for i in 0..archive.len() {
                let mut file = archive.by_index(i)?;
                let raw_name = file.enclosed_name().context("invalid zip entry name")?;

                let out_path = plugin_dir_clone.join(&raw_name);

                // Path traversal check: ensure extracted path stays within plugin_dir
                // We must check the path components, not just canonicalize (which requires the file to exist)
                let normalized = out_path.components().fold(PathBuf::new(), |mut acc, comp| {
                    match comp {
                        std::path::Component::ParentDir => {
                            acc.pop();
                        }
                        std::path::Component::Normal(c) => acc.push(c),
                        std::path::Component::RootDir => acc.push("/"),
                        _ => {}
                    }
                    acc
                });
                if !normalized.starts_with(&canonical_base) {
                    bail!(
                        "path traversal detected in zip entry: {}",
                        raw_name.display()
                    );
                }

                if file.is_dir() {
                    std::fs::create_dir_all(&out_path)?;
                } else {
                    if let Some(parent) = out_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let mut outfile = std::fs::File::create(&out_path)?;
                    std::io::copy(&mut file, &mut outfile)?;
                }
            }

            // Parse manifest
            let manifest_path = plugin_dir_clone.join("manifest.toml");
            let manifest = PluginManifest::load(&manifest_path)
                .context("plugin zip must contain a valid manifest.toml")?;

            // Verify entry script exists
            let entry_path = plugin_dir_clone.join(&manifest.plugin.entry);
            if !entry_path.exists() {
                bail!(
                    "entry script '{}' not found in plugin archive",
                    manifest.plugin.entry
                );
            }

            Ok(manifest)
        })
        .await??;

        // Register the plugin
        let mut plugins = self.plugins.write().await;
        plugins.insert(
            name.to_string(),
            PluginEntry {
                manifest,
                state: PluginState::Installed,
                dir: plugin_dir,
                pid: None,
            },
        );

        info!(name, "plugin installed");
        Ok(())
    }

    /// Uninstall a plugin, removing its directory.
    pub async fn uninstall(&self, name: &str) -> Result<()> {
        let mut plugins = self.plugins.write().await;
        let entry = plugins
            .get(name)
            .with_context(|| format!("plugin not found: {name}"))?;

        if entry.state == PluginState::Running {
            bail!("cannot uninstall running plugin: {name}");
        }

        let dir = entry.dir.clone();
        plugins.remove(name);
        drop(plugins);

        tokio::fs::remove_dir_all(&dir).await?;
        info!(name, "plugin uninstalled");
        Ok(())
    }

    /// List all installed plugins.
    pub async fn list(&self) -> Vec<PluginSnapshot> {
        let plugins = self.plugins.read().await;
        plugins
            .values()
            .map(|e| PluginSnapshot {
                name: e.manifest.plugin.name.clone(),
                version: e.manifest.plugin.version.clone(),
                plugin_type: e.manifest.plugin.plugin_type.to_string(),
                state: e.state,
                description: e.manifest.plugin.description.clone().unwrap_or_default(),
            })
            .collect()
    }

    /// Get mutable access to a plugin entry for state updates.
    pub async fn get_mut(
        &self,
        name: &str,
    ) -> Option<tokio::sync::RwLockWriteGuard<'_, HashMap<String, PluginEntry>>> {
        let plugins = self.plugins.write().await;
        if plugins.contains_key(name) {
            Some(plugins)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct PluginSnapshot {
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub state: PluginState,
    pub description: String,
}
