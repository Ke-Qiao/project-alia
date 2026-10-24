use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::assets;
use super::manifest::{PluginCapabilities, PluginManifest, PluginRuntime, PluginType};
use alia_plugin_runtime::Deployment;

mod process;
mod wasm;
use crate::module::traits::DiscoveryMetadata;
use process::ProcessOwner;
use wasm::WasmOwner;

const MAX_COMMANDS: usize = 1024;
const MAX_MESSAGE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    Installed,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

struct Run {
    instance_id: String,
    commands: mpsc::Sender<Vec<u8>>,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
    cleanup_error: Option<String>,
}

struct PluginEntry {
    manifest: PluginManifest,
    state: PluginState,
    dir: PathBuf,
    asset_sha256: String,
    run: Option<Run>,
    runtime_capabilities: Vec<String>,
    commands: HashMap<String, CommandRecord>,
}

#[derive(Clone, Debug)]
pub struct CommandReceipt {
    pub instance_id: String,
    pub command_id: String,
    pub state: String,
    pub result_json: Vec<u8>,
}

struct CommandRecord {
    action: String,
    payload: serde_json::Value,
    receipt: CommandReceipt,
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
    pub instance_id: String,
    pub command_id: String,
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

    pub async fn scan(&self) -> Result<()> {
        if !self.plugins_dir.exists() {
            return Ok(());
        }
        let mut plugins = self.plugins.write().await;
        for entry in std::fs::read_dir(&self.plugins_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || plugins.contains_key(&name) {
                continue;
            }
            let checked = (|| -> Result<_> {
                assets::validate_name(&name)?;
                let (manifest, asset) = assets::verify(&entry.path(), None)?;
                ensure!(
                    manifest.plugin.name == name,
                    "plugin directory and manifest name differ"
                );
                Ok((manifest, asset))
            })();
            match checked {
                Ok((manifest, asset)) => {
                    plugins.insert(
                        name,
                        PluginEntry {
                            manifest,
                            state: PluginState::Installed,
                            dir: entry.path(),
                            asset_sha256: asset.sha256,
                            run: None,
                            runtime_capabilities: Vec::new(),
                            commands: HashMap::new(),
                        },
                    );
                }
                Err(error) => {
                    warn!(%name, %error, "plugin asset is not verified; leaving it inactive")
                }
            }
        }
        Ok(())
    }

    pub async fn install(&self, name: &str, version: &str, zip_data: &[u8]) -> Result<String> {
        assets::validate_name(name)?;
        ensure!(
            zip_data.len() <= assets::MAX_ARCHIVE,
            "plugin archive exceeds 16 MiB"
        );
        let mut plugins = self.plugins.write().await;
        std::fs::create_dir_all(&self.plugins_dir)?;
        let root = self.plugins_dir.canonicalize()?;
        let destination = root.join(name);
        ensure!(
            !plugins.contains_key(name)
                && std::fs::symlink_metadata(&destination)
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
            "plugin already installed; stop and uninstall before replacing its asset"
        );
        let (owned_name, owned_version, bytes) =
            (name.to_owned(), version.to_owned(), zip_data.to_vec());
        let prepare_root = root.clone();
        // Cancellation leaves only this operation's TempDir; the worker never publishes.
        let (stage, manifest, asset) = tokio::task::spawn_blocking(move || {
            assets::prepare(&prepare_root, &owned_name, &owned_version, &bytes)
        })
        .await??;
        std::fs::rename(stage.path(), &destination).context("publishing plugin asset")?;
        let digest = asset.sha256;
        plugins.insert(
            name.to_owned(),
            PluginEntry {
                manifest,
                state: PluginState::Installed,
                dir: destination,
                asset_sha256: digest.clone(),
                run: None,
                runtime_capabilities: Vec::new(),
                commands: HashMap::new(),
            },
        );
        sync_directories(&root).context("plugin asset was published, but durability is unknown; inspect installed identity before retrying")?;
        info!(name, asset_sha256 = %digest, "plugin installed without activation");
        Ok(digest)
    }

    pub async fn uninstall(&self, name: &str) -> Result<()> {
        let mut plugins = self.plugins.write().await;
        let entry = plugins
            .get(name)
            .with_context(|| format!("plugin not found: {name}"))?;
        ensure!(
            entry.run.is_none(),
            "cannot uninstall plugin before its process and pipes finish cleanup"
        );
        std::fs::remove_dir_all(&entry.dir)?;
        plugins.remove(name);
        std::fs::File::open(&self.plugins_dir)?
            .sync_all()
            .context("plugin was removed, but directory durability is unknown")?;
        Ok(())
    }

    pub async fn start(
        &self,
        name: &str,
        expected_asset_sha256: &str,
        allow_trusted_native: bool,
    ) -> Result<()> {
        let mut plugins = self.plugins.write().await;
        let entry = plugins
            .get_mut(name)
            .with_context(|| format!("plugin not found: {name}"))?;
        ensure!(
            entry.run.is_none(),
            "plugin already has a running or cleaning-up instance"
        );
        ensure!(
            entry.asset_sha256 == expected_asset_sha256,
            "plugin asset changed; review the installed digest before activation"
        );
        let (manifest, _) = assets::verify(&entry.dir, Some(&entry.asset_sha256))?;
        let instance_id = uuid::Uuid::new_v4().to_string();
        if manifest.plugin.runtime == PluginRuntime::Wasm {
            ensure!(
                manifest.plugin.plugin_type == PluginType::Processor,
                "WASM packages currently support processors only"
            );
            ensure!(
                !manifest.plugin.capabilities.gpio
                    && !manifest.plugin.capabilities.network
                    && !manifest.plugin.capabilities.usb
                    && manifest.plugin.capabilities.expression_actions.is_empty()
                    && manifest.plugin.capabilities.expression_state.is_empty()
                    && manifest.plugin.events.publishes.is_empty()
                    && manifest.plugin.events.subscribes.is_empty(),
                "WASM processors cannot request native device or network authority"
            );
            let deployment = Deployment::new(
                std::fs::read(entry.dir.join(&manifest.plugin.entry))?,
                manifest.plugin.wasm_limits.clone(),
                &serde_json::to_value(&manifest)?,
            )?;
            deployment.require_export("alia_processor")?;
            deployment.enable(deployment.implementation_pin())?;
            let (commands, receiver) = mpsc::channel(16);
            let stop = CancellationToken::new();
            entry.state = PluginState::Running;
            entry.runtime_capabilities.clear();
            entry.run = Some(Run {
                instance_id: instance_id.clone(),
                commands,
                stop: stop.clone(),
                task: None,
                cleanup_error: None,
            });
            let owner = WasmOwner {
                plugins: self.plugins.clone(),
                events: self.event_tx.clone(),
                name: name.to_owned(),
                instance: instance_id.clone(),
                stop,
                deployment,
            };
            entry.run.as_mut().expect("run installed").task =
                Some(tokio::spawn(owner.run(receiver)));
            emit(
                &self.event_tx,
                name,
                &instance_id,
                "",
                "lifecycle",
                serde_json::json!({"state":"running","runtime":"wasm"}),
            );
            return Ok(());
        }
        ensure!(
            allow_trusted_native,
            "trusted native device execution requires explicit operator authorization"
        );
        ensure!(
            manifest.plugin.plugin_type != PluginType::Processor,
            "processor plugins must use WASM"
        );
        ensure!(
            cfg!(unix),
            "trusted native process ownership is supported on Unix only"
        );
        let (command, args) = plugin_command(&entry.dir, &manifest);
        let mut command_builder = Command::new(&command);
        command_builder
            .args(args)
            .current_dir(&entry.dir)
            .env("ALIA_PLUGIN_INSTANCE_ID", &instance_id)
            .env("ALIA_PLUGIN_ASSET_SHA256", &entry.asset_sha256)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command_builder.process_group(0);
        entry.state = PluginState::Starting;
        let mut child = match command_builder.spawn() {
            Ok(child) => child,
            Err(error) => {
                entry.state = PluginState::Failed;
                return Err(error).context("spawning plugin");
            }
        };
        let stdin = child.stdin.take().context("plugin stdin missing")?;
        let stdout = child.stdout.take().context("plugin stdout missing")?;
        let stderr = child.stderr.take().context("plugin stderr missing")?;
        let (commands, receiver) = mpsc::channel(16);
        let stop = CancellationToken::new();
        entry.runtime_capabilities.clear();
        entry.run = Some(Run {
            instance_id: instance_id.clone(),
            commands,
            stop: stop.clone(),
            task: None,
            cleanup_error: None,
        });
        let owner = ProcessOwner {
            plugins: self.plugins.clone(),
            events: self.event_tx.clone(),
            name: name.to_owned(),
            instance: instance_id.clone(),
            stop,
            ready: CancellationToken::new(),
        };
        let task = tokio::spawn(owner.run(child, stdin, stdout, stderr, receiver));
        entry.run.as_mut().expect("run was installed").task = Some(task);
        emit(
            &self.event_tx,
            name,
            &instance_id,
            "",
            "lifecycle",
            serde_json::json!({"state":"starting"}),
        );
        Ok(())
    }

    pub async fn stop(&self, name: &str) -> Result<()> {
        let mut plugins = self.plugins.write().await;
        let entry = plugins
            .get_mut(name)
            .with_context(|| format!("plugin not found: {name}"))?;
        let run = entry
            .run
            .as_ref()
            .context("plugin has no active instance")?;
        entry.state = PluginState::Stopping;
        ensure!(
            !run.task.as_ref().is_some_and(|task| task.is_finished()),
            "plugin owner exited without confirmed cleanup"
        );
        ensure!(
            run.cleanup_error.is_none(),
            "plugin cleanup pending: {}",
            run.cleanup_error.as_deref().unwrap_or_default()
        );
        run.stop.cancel();
        emit(
            &self.event_tx,
            name,
            &run.instance_id,
            "",
            "lifecycle",
            serde_json::json!({"state":"stopping"}),
        );
        Ok(())
    }

    pub async fn send_command(
        &self,
        name: &str,
        instance_id: &str,
        command_id: &str,
        action: &str,
        payload_json: &[u8],
    ) -> Result<CommandReceipt> {
        ensure!(
            !instance_id.is_empty()
                && instance_id.len() <= 128
                && !command_id.is_empty()
                && command_id.len() <= 256,
            "instance_id and stable command_id are required"
        );
        ensure!(
            !action.is_empty() && action.len() <= 256 && payload_json.len() <= MAX_MESSAGE,
            "invalid or oversized plugin command"
        );
        let payload = if payload_json.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_slice(payload_json).context("invalid command payload")?
        };
        let mut plugins = self.plugins.write().await;
        let entry = plugins
            .get_mut(name)
            .with_context(|| format!("plugin not found: {name}"))?;
        if let Some(previous) = entry.commands.get(command_id) {
            ensure!(
                previous.receipt.instance_id == instance_id
                    && previous.action == action
                    && previous.payload == payload,
                "command ID already belongs to another instance or payload"
            );
            return Ok(previous.receipt.clone());
        }
        ensure!(entry.state == PluginState::Running, "plugin is not running");
        let run = entry.run.as_ref().context("plugin owner missing")?;
        ensure!(
            run.instance_id == instance_id,
            "plugin instance changed; old commands must not be replayed"
        );
        ensure!(
            entry.commands.len() < MAX_COMMANDS,
            "plugin command ledger is full; no new commands admitted"
        );
        let mut envelope = serde_json::to_vec(
            &serde_json::json!({"instance_id":instance_id,"command_id":command_id,"action":action,"payload":payload}),
        )?;
        ensure!(
            envelope.len() < MAX_MESSAGE,
            "command envelope exceeds budget"
        );
        envelope.push(b'\n');
        let receipt = CommandReceipt {
            instance_id: instance_id.to_owned(),
            command_id: command_id.to_owned(),
            state: "unknown".into(),
            result_json: Vec::new(),
        };
        run.commands
            .try_send(envelope)
            .context("plugin command queue unavailable")?;
        entry.commands.insert(
            command_id.to_owned(),
            CommandRecord {
                action: action.to_owned(),
                payload,
                receipt: receipt.clone(),
            },
        );
        Ok(receipt)
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<PluginEventRecord> {
        self.event_tx.subscribe()
    }

    pub async fn list(&self) -> Vec<PluginSnapshot> {
        self.plugins
            .read()
            .await
            .values()
            .map(|entry| {
                let mut capabilities =
                    plugin_capability_strings(&entry.manifest.plugin.capabilities);
                capabilities.extend(entry.runtime_capabilities.clone());
                capabilities.sort();
                capabilities.dedup();
                PluginSnapshot {
                    name: entry.manifest.plugin.name.clone(),
                    version: entry.manifest.plugin.version.clone(),
                    plugin_type: entry.manifest.plugin.plugin_type.to_string(),
                    state: if entry
                        .run
                        .as_ref()
                        .is_some_and(|run| run.task.as_ref().is_some_and(|task| task.is_finished()))
                    {
                        PluginState::Stopping
                    } else {
                        entry.state
                    },
                    description: entry
                        .manifest
                        .plugin
                        .description
                        .clone()
                        .unwrap_or_default(),
                    capabilities,
                    asset_sha256: entry.asset_sha256.clone(),
                    instance_id: entry
                        .run
                        .as_ref()
                        .map(|run| run.instance_id.clone())
                        .unwrap_or_default(),
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
}

#[derive(Debug, Clone)]
pub struct PluginSnapshot {
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub state: PluginState,
    pub description: String,
    pub capabilities: Vec<String>,
    pub asset_sha256: String,
    pub instance_id: String,
}

fn emit(
    tx: &broadcast::Sender<PluginEventRecord>,
    name: &str,
    instance: &str,
    command: &str,
    event: &str,
    payload: serde_json::Value,
) {
    let _ = tx.send(PluginEventRecord {
        plugin_name: name.to_owned(),
        instance_id: instance.to_owned(),
        command_id: command.to_owned(),
        event_type: event.to_owned(),
        payload_json: serde_json::to_vec(&payload).expect("JSON value serializes"),
        timestamp_us: timestamp_us(),
    });
}

fn sync_directories(path: &std::path::Path) -> std::io::Result<()> {
    for directory in path.ancestors() {
        std::fs::File::open(directory)?.sync_all()?;
    }
    Ok(())
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
    // Native drivers must be executable assets. No extension-based interpreter dispatch.
    (dir.join(&manifest.plugin.entry), Vec::new())
}

fn timestamp_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

#[cfg(test)]
mod tests;
