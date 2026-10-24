//! On-disk installation and grant lifecycle. Installed assets never execute.
use crate::{Deployment, Limits, MAX_MODULE_BYTES};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub abi_version: u32,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub tools: Vec<alia_plugin_api::ToolDescriptor>,
    /// Expression capabilities use their separately versioned, typed domain ABI.
    #[serde(default)]
    pub expression: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageSnapshot {
    pub manifest: PackageManifest,
    pub implementation_pin: String,
    pub enabled: bool,
}
struct Installed {
    manifest: PackageManifest,
    deployment: Arc<Deployment>,
}

pub struct PackageStore {
    root: PathBuf,
    entries: Mutex<BTreeMap<String, Installed>>,
}

impl PackageStore {
    pub fn open(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let mut entries = BTreeMap::new();
        for dir in std::fs::read_dir(&root)? {
            let dir = dir?;
            let name = dir.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            validate_name(&name)?;
            ensure!(
                dir.file_type()?.is_dir(),
                "plugin package must be a directory"
            );
            let (manifest, module) = read_asset(&dir.path())?;
            ensure!(manifest.name == name, "package name differs from directory");
            let deployment = prepare(&manifest, module)?;
            let grant_path = dir.path().join("grant");
            if grant_path.exists() {
                let grant = read_bounded(&grant_path, 256)?;
                if grant == deployment.implementation_pin().as_bytes() {
                    deployment.enable(deployment.implementation_pin())?;
                }
            }
            entries.insert(
                name,
                Installed {
                    manifest,
                    deployment,
                },
            );
        }
        Ok(Self {
            root,
            entries: Mutex::new(entries),
        })
    }

    pub async fn list(&self) -> Vec<PackageSnapshot> {
        self.entries.lock().await.values().map(snapshot).collect()
    }
    pub async fn get(&self, name: &str) -> Result<(PackageManifest, Arc<Deployment>)> {
        let entries = self.entries.lock().await;
        let entry = entries.get(name).context("plugin package not installed")?;
        Ok((entry.manifest.clone(), entry.deployment.clone()))
    }
    pub async fn install(
        &self,
        manifest: PackageManifest,
        module: Vec<u8>,
    ) -> Result<PackageSnapshot> {
        let deployment = prepare(&manifest, module.clone())?;
        let mut entries = self.entries.lock().await;
        ensure!(
            !entries.contains_key(&manifest.name),
            "package already installed; use upgrade with its expected pin"
        );
        let destination = self.root.join(&manifest.name);
        ensure!(!destination.exists(), "package destination already exists");
        let durable = publish(&self.root, &destination, &manifest, &module)?;
        let entry = Installed {
            manifest,
            deployment,
        };
        let result = snapshot(&entry);
        entries.insert(entry.manifest.name.clone(), entry);
        durable.context("package installed, but directory durability is unconfirmed; inspect its current pin before retrying")?;
        Ok(result)
    }
    /// The caller must supply the reviewed identity. Every grant receives a new
    /// Deployment so a revoked Arc captured by an old tool never becomes live.
    pub async fn enable(&self, name: &str, expected_pin: &str) -> Result<Arc<Deployment>> {
        let mut entries = self.entries.lock().await;
        let entry = entries
            .get_mut(name)
            .context("plugin package not installed")?;
        ensure!(
            entry.deployment.implementation_pin() == expected_pin,
            "package pin changed"
        );
        if entry.deployment.is_live() {
            return Ok(entry.deployment.clone());
        }
        entry.deployment.shutdown().await?;
        let (manifest, module) = read_asset(&self.root.join(name))?;
        let deployment = prepare(&manifest, module)?;
        ensure!(
            deployment.implementation_pin() == expected_pin,
            "installed WASM changed; upgrade and review it again"
        );
        let durable = write_atomic(&self.root.join(name), "grant", expected_pin.as_bytes())?;
        if let Err(error) = durable {
            // A rename already published the grant. Withdraw it before returning
            // a failed activation; never silently restore this attempt on boot.
            remove_grant(&self.root.join(name)).context(
                "grant publication and rollback durability unconfirmed; runtime remains disabled",
            )?;
            return Err(error).context(
                "grant publication was not durable; grant withdrawn and runtime remains disabled",
            );
        }
        deployment.enable(expected_pin)?;
        entry.deployment = deployment.clone();
        Ok(deployment)
    }
    pub async fn revoke(&self, name: &str, expected_pin: &str) -> Result<()> {
        let entries = self.entries.lock().await;
        let entry = entries.get(name).context("plugin package not installed")?;
        ensure!(
            entry.deployment.implementation_pin() == expected_pin,
            "package pin changed"
        );
        // Remove the persisted grant before waiting, so cancellation/restart cannot restore it.
        entry.deployment.close();
        let persisted = remove_grant(&self.root.join(name));
        entry.deployment.shutdown().await?;
        persisted.context(
            "runtime revoked, but grant removal durability is unconfirmed; retry before restarting",
        )
    }
    pub async fn upgrade(
        &self,
        manifest: PackageManifest,
        module: Vec<u8>,
        expected_pin: &str,
    ) -> Result<PackageSnapshot> {
        let deployment = prepare(&manifest, module.clone())?;
        let mut entries = self.entries.lock().await;
        let entry = entries
            .get(&manifest.name)
            .context("plugin package not installed")?;
        ensure!(
            entry.deployment.implementation_pin() == expected_pin,
            "package pin changed"
        );
        entry.deployment.close();
        let persisted = remove_grant(&self.root.join(&manifest.name));
        entry.deployment.shutdown().await?;
        persisted.context(
            "runtime revoked, but grant removal durability is unconfirmed; retry before restarting",
        )?;
        // One atomic asset file contains metadata and WASM. A crash exposes either
        // complete generation, always without a grant after the revocation above.
        let destination = self.root.join(&manifest.name);
        let durable = write_atomic(&destination, "asset", &encode_asset(&manifest, &module)?)?;
        let entry = Installed {
            manifest,
            deployment,
        };
        let result = snapshot(&entry);
        entries.insert(entry.manifest.name.clone(), entry);
        durable.context("new package generation published inactive, but durability is unconfirmed; inspect its current pin before retrying")?;
        Ok(result)
    }
    pub async fn uninstall(&self, name: &str, expected_pin: &str) -> Result<()> {
        let mut entries = self.entries.lock().await;
        let entry = entries.get(name).context("plugin package not installed")?;
        ensure!(
            entry.deployment.implementation_pin() == expected_pin,
            "package pin changed"
        );
        entry.deployment.close();
        let persisted = remove_grant(&self.root.join(name));
        entry.deployment.shutdown().await?;
        persisted.context(
            "runtime revoked, but grant removal durability is unconfirmed; retry before restarting",
        )?;
        std::fs::remove_dir_all(self.root.join(name))?;
        entries.remove(name);
        std::fs::File::open(&self.root)?
            .sync_all()
            .context("package removed, but directory durability is unconfirmed")?;
        Ok(())
    }
    pub async fn shutdown(&self) -> Result<()> {
        let entries = self.entries.lock().await;
        for entry in entries.values() {
            entry.deployment.close();
        }
        for entry in entries.values() {
            entry.deployment.shutdown().await?;
        }
        Ok(())
    }
}
fn snapshot(entry: &Installed) -> PackageSnapshot {
    PackageSnapshot {
        manifest: entry.manifest.clone(),
        implementation_pin: entry.deployment.implementation_pin().to_owned(),
        enabled: entry.deployment.is_live(),
    }
}
fn prepare(manifest: &PackageManifest, module: Vec<u8>) -> Result<Arc<Deployment>> {
    validate_name(&manifest.name)?;
    ensure!(
        !manifest.version.is_empty() && manifest.version.len() <= 128,
        "invalid package version"
    );
    ensure!(
        manifest.abi_version == alia_plugin_api::ABI_VERSION,
        "unsupported WASM package ABI"
    );
    ensure!(
        manifest.tools.len() <= 64 && serde_json::to_vec(manifest)?.len() <= 128 * 1024,
        "package metadata exceeds budget"
    );
    Deployment::new(
        module,
        manifest.limits.clone(),
        &serde_json::to_value(manifest)?,
    )
}
fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 48
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "invalid plugin package name"
    );
    Ok(())
}
fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.file_type().is_file() && metadata.len() <= limit as u64,
        "invalid or oversized package asset"
    );
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "package asset exceeds byte budget");
    Ok(bytes)
}
// Outer errors precede publication. The inner result reports directory fsync
// after rename; callers must update observable state even when that sync fails.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<std::io::Result<()>> {
    use std::io::Write;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(dir.join(name)).map_err(|e| e.error)?;
    Ok(std::fs::File::open(dir).and_then(|file| file.sync_all()))
}
fn publish(
    root: &Path,
    destination: &Path,
    manifest: &PackageManifest,
    module: &[u8],
) -> Result<std::io::Result<()>> {
    let stage = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(root)?;
    write_atomic(stage.path(), "asset", &encode_asset(manifest, module)?)??;
    std::fs::rename(stage.path(), destination)?;
    Ok(std::fs::File::open(root).and_then(|file| file.sync_all()))
}
fn remove_grant(dir: &Path) -> Result<()> {
    match std::fs::remove_file(dir.join("grant")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // A prior unlink may have succeeded without a durable directory sync.
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

fn encode_asset(manifest: &PackageManifest, module: &[u8]) -> Result<Vec<u8>> {
    let metadata = serde_json::to_vec(manifest)?;
    let mut bytes = Vec::with_capacity(8 + metadata.len() + module.len());
    bytes.extend_from_slice(b"ALW1");
    bytes.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&metadata);
    bytes.extend_from_slice(module);
    Ok(bytes)
}
fn read_asset(dir: &Path) -> Result<(PackageManifest, Vec<u8>)> {
    let bytes = read_bounded(&dir.join("asset"), MAX_MODULE_BYTES + 128 * 1024 + 8)?;
    ensure!(
        bytes.len() >= 8 && &bytes[..4] == b"ALW1",
        "invalid package asset envelope"
    );
    let length = u32::from_le_bytes(bytes[4..8].try_into().expect("four bytes")) as usize;
    ensure!(
        length <= 128 * 1024 && length <= bytes.len() - 8,
        "invalid package metadata size"
    );
    let manifest = serde_json::from_slice(&bytes[8..8 + length])?;
    Ok((manifest, bytes[8 + length..].to_vec()))
}
