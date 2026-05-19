use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Mutex, RwLock, broadcast};
use tracing::{info, warn};

use crate::module::traits::DiscoveryMetadata;

use super::manifest::{PluginCapabilities, PluginManifest};

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
    pub stdin: Option<Arc<Mutex<ChildStdin>>>,
    pub runtime_capabilities: Vec<String>,
}

pub struct PluginRegistry {
    plugins: Arc<RwLock<HashMap<String, PluginEntry>>>,
    plugins_dir: PathBuf,
    event_tx: broadcast::Sender<PluginEventRecord>,
}

#[derive(Debug, Clone, Default)]
pub struct PluginDiscoverySummary {
    pub capabilities: Vec<String>,
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct PluginEventRecord {
    pub plugin_name: String,
    pub event_type: String,
    pub payload_json: Vec<u8>,
    pub timestamp_us: u64,
}

impl PluginRegistry {
    pub fn new(plugins_dir: PathBuf) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            plugins_dir,
            event_tx,
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
                            stdin: None,
                            runtime_capabilities: Vec::new(),
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
                stdin: None,
                runtime_capabilities: Vec::new(),
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

    pub async fn start(&self, name: &str) -> Result<()> {
        let (manifest, dir) = {
            let mut plugins = self.plugins.write().await;
            let entry = plugins
                .get_mut(name)
                .with_context(|| format!("plugin not found: {name}"))?;

            if matches!(entry.state, PluginState::Running | PluginState::Starting) {
                bail!("plugin already running: {name}");
            }

            entry.state = PluginState::Starting;
            entry.pid = None;
            entry.stdin = None;
            (entry.manifest.clone(), entry.dir.clone())
        };

        let (command, args) = plugin_command(&dir, &manifest);
        let mut child = Command::new(&command);
        child
            .args(&args)
            .current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = child
            .spawn()
            .with_context(|| format!("spawning plugin '{}' with {}", name, command.display()))?;

        let pid = child.id().unwrap_or_default();
        let stdin = child
            .stdin
            .take()
            .map(|handle| Arc::new(Mutex::new(handle)));
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        {
            let mut plugins = self.plugins.write().await;
            let entry = plugins
                .get_mut(name)
                .with_context(|| format!("plugin not found after spawn: {name}"))?;
            entry.state = PluginState::Running;
            entry.pid = Some(pid);
            entry.stdin = stdin.clone();
        }

        self.emit_event(
            name,
            "lifecycle",
            serde_json::json!({ "state": "running", "pid": pid }),
        );

        self.spawn_output_task(name.to_string(), "stdout", stdout);
        self.spawn_output_task(name.to_string(), "stderr", stderr);
        self.spawn_wait_task(name.to_string(), child);

        info!(name, pid, "plugin started");
        Ok(())
    }

    pub async fn stop(&self, name: &str) -> Result<()> {
        let pid = {
            let mut plugins = self.plugins.write().await;
            let entry = plugins
                .get_mut(name)
                .with_context(|| format!("plugin not found: {name}"))?;

            if !matches!(entry.state, PluginState::Running | PluginState::Starting) {
                bail!("plugin is not running: {name}");
            }

            entry.state = PluginState::Stopping;
            entry.stdin = None;
            entry.pid
        };

        if let Some(pid) = pid {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }

        self.emit_event(
            name,
            "lifecycle",
            serde_json::json!({ "state": "stopping" }),
        );
        info!(name, pid, "plugin stop requested");
        Ok(())
    }

    pub async fn send_command(
        &self,
        name: &str,
        command_id: &str,
        action: &str,
        payload_json: &[u8],
    ) -> Result<()> {
        let stdin = {
            let plugins = self.plugins.read().await;
            let entry = plugins
                .get(name)
                .with_context(|| format!("plugin not found: {name}"))?;
            if entry.state != PluginState::Running {
                bail!("plugin is not running: {name}");
            }
            entry
                .stdin
                .clone()
                .ok_or_else(|| anyhow::anyhow!("plugin stdin unavailable: {name}"))?
        };

        let payload = if payload_json.is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_slice(payload_json).context("parsing plugin command payload")?
        };

        let envelope = serde_json::json!({
            "command_id": command_id,
            "action": action,
            "payload": payload,
        });

        let mut stdin = stdin.lock().await;
        stdin
            .write_all(serde_json::to_string(&envelope)?.as_bytes())
            .await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        Ok(())
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<PluginEventRecord> {
        self.event_tx.subscribe()
    }

    /// List all installed plugins.
    pub async fn list(&self) -> Vec<PluginSnapshot> {
        let plugins = self.plugins.read().await;
        plugins
            .values()
            .map(|e| {
                let mut capabilities = plugin_capability_strings(&e.manifest.plugin.capabilities);
                capabilities.extend(e.runtime_capabilities.clone());
                capabilities.sort();
                capabilities.dedup();
                PluginSnapshot {
                    name: e.manifest.plugin.name.clone(),
                    version: e.manifest.plugin.version.clone(),
                    plugin_type: e.manifest.plugin.plugin_type.to_string(),
                    state: e.state,
                    description: e.manifest.plugin.description.clone().unwrap_or_default(),
                    capabilities,
                }
            })
            .collect()
    }

    /// Build a discovery-friendly summary of installed plugins.
    ///
    /// This keeps discovery generic: future plugin runtime features can enrich
    /// advertisements without changing the discovery backends themselves.
    pub async fn discovery_summary(&self) -> PluginDiscoverySummary {
        let plugins = self.plugins.read().await;
        let mut names = Vec::new();
        let mut types = Vec::new();
        let mut capabilities = vec!["plugin.runtime".to_string()];

        let mut gpio = false;
        let mut network = false;
        let mut usb = false;

        for entry in plugins.values() {
            names.push(entry.manifest.plugin.name.clone());
            types.push(entry.manifest.plugin.plugin_type.to_string());
            capabilities.push(format!("plugin.type.{}", entry.manifest.plugin.plugin_type));

            if entry.manifest.plugin.capabilities.gpio {
                gpio = true;
            }
            if entry.manifest.plugin.capabilities.network {
                network = true;
            }
            if entry.manifest.plugin.capabilities.usb {
                usb = true;
            }
        }

        names.sort();
        types.sort();
        types.dedup();

        if gpio {
            capabilities.push("plugin.capability.gpio".to_string());
        }
        if network {
            capabilities.push("plugin.capability.network".to_string());
        }
        if usb {
            capabilities.push("plugin.capability.usb".to_string());
        }

        capabilities.sort();
        capabilities.dedup();

        let mut attributes = HashMap::new();
        attributes.insert("plugin.count".to_string(), names.len().to_string());
        if !names.is_empty() {
            attributes.insert("plugin.names".to_string(), names.join(","));
            attributes.insert("plugin.types".to_string(), types.join(","));
        }
        attributes.insert("plugin.cap.gpio".to_string(), gpio.to_string());
        attributes.insert("plugin.cap.network".to_string(), network.to_string());
        attributes.insert("plugin.cap.usb".to_string(), usb.to_string());

        PluginDiscoverySummary {
            capabilities,
            attributes,
        }
    }

    pub async fn enrich_discovery_metadata(&self, metadata: &mut DiscoveryMetadata) {
        let summary = self.discovery_summary().await;
        metadata.capabilities.extend(summary.capabilities);
        metadata.attributes.extend(summary.attributes);
        metadata.capabilities.sort();
        metadata.capabilities.dedup();
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

    fn emit_event(&self, plugin_name: &str, event_type: &str, payload: serde_json::Value) {
        let payload_json = serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec());
        let _ = self.event_tx.send(PluginEventRecord {
            plugin_name: plugin_name.to_string(),
            event_type: event_type.to_string(),
            payload_json,
            timestamp_us: timestamp_us(),
        });
    }

    fn spawn_output_task(
        &self,
        plugin_name: String,
        stream_name: &'static str,
        stream: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
    ) {
        let Some(stream) = stream else {
            return;
        };
        let event_tx = self.event_tx.clone();
        let plugins = Arc::clone(&self.plugins);

        tokio::spawn(async move {
            let reader = BufReader::new(stream);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let (event_type, payload_json) =
                    match serde_json::from_str::<serde_json::Value>(&line) {
                        Ok(value) => {
                            let event_type = value
                                .get("event_type")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or(stream_name)
                                .to_string();
                            let payload = value.get("payload").cloned().unwrap_or(value);
                            (
                                event_type,
                                serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec()),
                            )
                        }
                        Err(_) => (
                            stream_name.to_string(),
                            serde_json::to_vec(&serde_json::json!({ "line": line }))
                                .unwrap_or_else(|_| b"{}".to_vec()),
                        ),
                    };

                if event_type == "expression_state" {
                    let runtime_capabilities = runtime_expression_capabilities(&payload_json);
                    if !runtime_capabilities.is_empty() {
                        let mut plugins = plugins.write().await;
                        if let Some(entry) = plugins.get_mut(&plugin_name) {
                            entry.runtime_capabilities = runtime_capabilities;
                        }
                    }
                }

                let _ = event_tx.send(PluginEventRecord {
                    plugin_name: plugin_name.clone(),
                    event_type,
                    payload_json,
                    timestamp_us: timestamp_us(),
                });
            }
        });
    }

    fn spawn_wait_task(&self, plugin_name: String, mut child: tokio::process::Child) {
        let plugins = Arc::clone(&self.plugins);
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            let (next_state, exit_code) = match status {
                Ok(status) if status.success() => (PluginState::Stopped, status.code()),
                Ok(status) => (PluginState::Failed, status.code()),
                Err(_) => (PluginState::Failed, None),
            };

            let mut map = plugins.write().await;
            if let Some(entry) = map.get_mut(&plugin_name) {
                entry.state = next_state;
                entry.pid = None;
                entry.stdin = None;
            }
            drop(map);

            let state_name = match next_state {
                PluginState::Stopped => "stopped",
                PluginState::Failed => "failed",
                PluginState::Installed => "installed",
                PluginState::Starting => "starting",
                PluginState::Running => "running",
                PluginState::Stopping => "stopping",
            };

            let payload_json = serde_json::to_vec(&serde_json::json!({
                "state": state_name,
                "exit_code": exit_code,
            }))
            .unwrap_or_else(|_| b"{}".to_vec());

            let _ = event_tx.send(PluginEventRecord {
                plugin_name,
                event_type: "lifecycle".to_string(),
                payload_json,
                timestamp_us: timestamp_us(),
            });
        });
    }
}

#[derive(Debug, Clone)]
pub struct PluginSnapshot {
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub state: PluginState,
    pub description: String,
    pub capabilities: Vec<String>,
}

fn plugin_capability_strings(capabilities: &PluginCapabilities) -> Vec<String> {
    let mut values = Vec::new();
    if capabilities.gpio {
        values.push("plugin.capability.gpio".to_string());
    }
    if capabilities.network {
        values.push("plugin.capability.network".to_string());
    }
    if capabilities.usb {
        values.push("plugin.capability.usb".to_string());
    }
    values.extend(
        capabilities
            .expression_actions
            .iter()
            .map(|action| format!("expression.action.{}", action.trim()))
            .filter(|capability| capability != "expression.action."),
    );
    values.extend(
        capabilities
            .expression_state
            .iter()
            .map(|state| format!("expression.state.{}", state.trim()))
            .filter(|capability| capability != "expression.state."),
    );
    values.sort();
    values.dedup();
    values
}

fn runtime_expression_capabilities(payload_json: &[u8]) -> Vec<String> {
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(payload_json) else {
        return Vec::new();
    };
    let mut values = Vec::new();
    if let Some(actions) = payload.get("actions").and_then(serde_json::Value::as_array) {
        values.extend(
            actions
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(|action| format!("expression.action.{}", action.trim()))
                .filter(|capability| capability != "expression.action."),
        );
    }
    if let Some(states) = payload.get("states").and_then(serde_json::Value::as_array) {
        values.extend(
            states
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(|state| format!("expression.state.{}", state.trim()))
                .filter(|capability| capability != "expression.state."),
        );
    }
    for (key, prefix) in [
        ("ready", "expression.state.ready"),
        ("busy", "expression.state.busy"),
        ("thermal_limit", "expression.state.thermal_limit"),
        ("battery_saver", "expression.state.battery_saver"),
    ] {
        if payload
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            values.push(prefix.to_string());
        }
    }
    if let Some(level) = payload
        .get("battery_level")
        .and_then(serde_json::Value::as_f64)
    {
        values.push(format!(
            "expression.state.battery_level.{:.2}",
            level.clamp(0.0, 1.0)
        ));
    }
    values.sort();
    values.dedup();
    values
}

fn plugin_command(dir: &std::path::Path, manifest: &PluginManifest) -> (PathBuf, Vec<String>) {
    let entry = dir.join(&manifest.plugin.entry);
    match entry.extension().and_then(|ext| ext.to_str()) {
        Some("py") => (PathBuf::from("python3"), vec![entry.display().to_string()]),
        Some("sh") => (PathBuf::from("sh"), vec![entry.display().to_string()]),
        _ => (entry, Vec::new()),
    }
}

fn timestamp_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
