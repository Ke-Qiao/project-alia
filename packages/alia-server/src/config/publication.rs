//! One recoverable publication of a server base file and its existing fragment layout.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ServerConfig;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSnapshot {
    digest: String,
    length: u64,
    device: Option<u64>,
    inode: Option<u64>,
    mode: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FragmentFile {
    relative: PathBuf,
    snapshot: FileSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fragments {
    present: bool,
    directories: Vec<PathBuf>,
    files: Vec<FragmentFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    version: u32,
    target: String,
    candidate: String,
    request_digest: String,
    replace: bool,
    original: Option<FileSnapshot>,
    prepared: FileSnapshot,
    fragments: Fragments,
}

/// Publication may already be visible. Only retry this recorded operation.
#[derive(Clone, Debug)]
pub(crate) struct PendingPublication {
    pub target: PathBuf,
    destination: PathBuf,
    descriptor: Descriptor,
    reason: String,
}

impl std::fmt::Display for PendingPublication {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "configuration publication requires recovery for {}: {}; files are retained and durability is unknown (unconfirmed)",
            self.target.display(),
            self.reason
        )
    }
}
impl std::error::Error for PendingPublication {}

impl PendingPublication {
    pub(crate) fn matches_request(
        &self,
        target: &Path,
        content: &str,
        replace: bool,
        fragments: Option<&[(&str, &str)]>,
    ) -> bool {
        destination(target).is_ok_and(|path| path == self.destination)
            && request_digest(content, replace, fragments) == self.descriptor.request_digest
    }

    pub(crate) fn retry(&self) -> Result<PathBuf> {
        complete(&self.destination, &self.descriptor, false)
            .map_err(|error| self.with_error(error))?;
        Ok(self.target.clone())
    }

    fn with_error(&self, error: anyhow::Error) -> anyhow::Error {
        Self {
            reason: format!("{error:#}"),
            ..self.clone()
        }
        .into()
    }
}

pub(crate) fn has_pending(target: &Path) -> bool {
    let Ok(destination) = destination(target) else {
        return true;
    };
    match fs::symlink_metadata(marker_path(&destination)) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Ordinary loads do no synchronization. Only a recorded intent invokes recovery.
pub(crate) fn recover(target: &Path) -> Result<()> {
    let destination = destination(target)?;
    let marker = marker_path(&destination);
    if !entry_exists(&marker)? {
        return Ok(());
    }
    let descriptor = read_descriptor(&destination)?;
    let pending = PendingPublication {
        target: target.to_path_buf(),
        destination,
        descriptor,
        reason: String::new(),
    };
    pending.retry().map(|_| ())
}

pub(crate) fn save(
    target: &Path,
    content: &str,
    replace: bool,
    fragments: Option<&[(&str, &str)]>,
) -> Result<PathBuf> {
    let initial_destination = destination(target)?;
    if entry_exists(&marker_path(&initial_destination))? {
        let descriptor = read_descriptor(&initial_destination)?;
        let pending = PendingPublication {
            target: target.to_path_buf(),
            destination: initial_destination,
            descriptor,
            reason: String::new(),
        };
        if !pending.matches_request(target, content, replace, fragments) {
            return Err(pending.with_error(anyhow::anyhow!(
                "a different configuration publication is pending recovery"
            )));
        }
        return pending.retry();
    }
    ensure!(
        replace || !entry_exists(&initial_destination)?,
        "configuration already exists: {}",
        target.display()
    );
    let fragment_dir = initial_destination.with_extension("d");
    if fragments.is_some() {
        ensure!(
            !replace,
            "example fragments require first-install publication"
        );
        ensure!(
            !entry_exists(&fragment_dir)?,
            "configuration fragments already exist: {}",
            fragment_dir.display()
        );
    }
    validate_input(&initial_destination, content, fragments)?;

    // Validation precedes mkdir; malformed HTTP requests leave no new directories.
    fs::create_dir_all(parent(&initial_destination))?;
    let destination = destination(target)?;
    let fragment_dir = destination.with_extension("d");
    let candidate = parent(&destination).join(format!(".alia-setup-{}.tmp", uuid::Uuid::new_v4()));
    let mut owns_fragments = false;
    let preparation = (|| {
        if let Some(fragments) = fragments {
            fs::create_dir(&fragment_dir).context("create new configuration fragment tree")?;
            owns_fragments = true;
            for (name, value) in fragments {
                let path = fragment_dir.join(name);
                fs::create_dir_all(parent(&path))?;
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                file.write_all(value.as_bytes())?;
                sync_file(&destination, &path, &file)?;
            }
        }
        let mut file = private_file(&candidate)?;
        file.write_all(content.as_bytes())?;
        sync_file(&destination, &candidate, &file)?;
        let prepared = snapshot(&candidate)?.context("configuration candidate disappeared")?;
        let manifest = fragment_snapshot(&destination)?;
        sync_fragments(&destination, &manifest)?;
        sync_ancestors(&destination)?;
        // Validate the actual layout as well as the in-memory CLI template.
        ServerConfig::load_candidate(&destination, content)?.validate_llm_roles()?;
        Ok(Descriptor {
            version: 1,
            target: filename(&destination)?.to_string(),
            candidate: filename(&candidate)?.to_string(),
            request_digest: request_digest(content, replace, fragments),
            replace,
            original: snapshot(&destination)?,
            prepared,
            fragments: manifest,
        })
    })();
    let descriptor = match preparation {
        Ok(descriptor) => descriptor,
        Err(error) => {
            cleanup_preparation(&candidate, owns_fragments.then_some(fragment_dir.as_path()));
            return Err(error);
        }
    };
    let pending = PendingPublication {
        target: target.to_path_buf(),
        destination: destination.clone(),
        descriptor: descriptor.clone(),
        reason: String::new(),
    };
    let marker = marker_path(&destination);
    let marker_temporary = intent_temporary(&destination, &descriptor);
    let mut marker_published = false;
    let install = (|| -> Result<()> {
        let mut file = private_file(&marker_temporary)?;
        file.write_all(&serde_json::to_vec(&descriptor)?)?;
        sync_file(&destination, &marker_temporary, &file)?;
        fs::hard_link(&marker_temporary, &marker)
            .context("exclusively publish configuration recovery record")?;
        marker_published = true;
        fs::remove_file(&marker_temporary)?;
        sync_ancestors(&destination)?;
        checkpoint(&destination, Stage::MarkerDurable, &marker)?;
        Ok(())
    })();
    if let Err(error) = install {
        let _ = fs::remove_file(&marker_temporary);
        if marker_published {
            return Err(pending.with_error(error));
        }
        cleanup_preparation(&candidate, owns_fragments.then_some(fragment_dir.as_path()));
        return Err(error);
    }
    complete(&destination, &descriptor, true).map_err(|error| pending.with_error(error))?;
    Ok(target.to_path_buf())
}

fn validate_input(
    destination: &Path,
    content: &str,
    fragments: Option<&[(&str, &str)]>,
) -> Result<()> {
    if let Some(fragments) = fragments {
        let mut value = toml::from_str(content)?;
        let mut fragments = fragments.to_vec();
        fragments.sort_by_key(|(name, _)| *name);
        for (name, content) in fragments {
            ensure!(
                safe_relative(Path::new(name)),
                "invalid example fragment path"
            );
            super::merge_toml_values(&mut value, toml::from_str(content)?);
        }
        ServerConfig::from_merged_content(destination, &toml::to_string(&value)?)?
            .validate_llm_roles()?;
    } else {
        ServerConfig::load_candidate(destination, content)?.validate_llm_roles()?;
    }
    Ok(())
}

fn complete(destination: &Path, descriptor: &Descriptor, prepared: bool) -> Result<()> {
    validate_descriptor(destination, descriptor)?;
    let marker = marker_path(destination);
    let marker_exists = entry_exists(&marker)?;
    if marker_exists {
        ensure!(
            &read_descriptor(destination)? == descriptor,
            "configuration recovery record changed"
        );
    }
    ensure!(
        fragment_snapshot(destination)? == descriptor.fragments,
        "configuration fragments changed; pending publication retained"
    );
    let current = snapshot(destination)?;
    let already_published = current.as_ref() == Some(&descriptor.prepared);
    let candidate = parent(destination).join(&descriptor.candidate);
    if !already_published {
        ensure!(
            marker_exists,
            "configuration recovery record is missing before publication"
        );
        ensure!(
            current == descriptor.original,
            "configuration target changed; pending publication retained"
        );
        ensure!(
            fs::symlink_metadata(&candidate)?.file_type().is_file(),
            "configuration candidate must remain a regular file"
        );
        ensure!(
            snapshot(&candidate)?.as_ref() == Some(&descriptor.prepared),
            "configuration candidate changed or disappeared; pending publication retained"
        );
    }
    let content_path = if already_published {
        destination
    } else {
        &candidate
    };
    let content = fs::read_to_string(content_path)?;
    ServerConfig::load_candidate(destination, &content)?.validate_llm_roles()?;
    if !prepared {
        sync_file(destination, content_path, &File::open(content_path)?)?;
        sync_fragments(destination, &descriptor.fragments)?;
        if marker_exists {
            sync_file(destination, &marker, &File::open(&marker)?)?;
        }
        sync_ancestors(destination)?;
    }
    if !already_published {
        if descriptor.replace {
            fs::rename(&candidate, destination)?;
        } else {
            fs::hard_link(&candidate, destination)?;
        }
        checkpoint(destination, Stage::Published, destination)?;
    }
    ensure!(
        snapshot(destination)?.as_ref() == Some(&descriptor.prepared),
        "published configuration changed; recovery retained"
    );
    ensure!(
        fragment_snapshot(destination)? == descriptor.fragments,
        "configuration fragments changed during publication"
    );
    sync_file(destination, destination, &File::open(destination)?)?;
    sync_ancestors(destination)?;

    // No deletion until the base and its dependencies have confirmed durability.
    if entry_exists(&candidate)? {
        ensure!(
            snapshot(&candidate)?.as_ref() == Some(&descriptor.prepared),
            "configuration candidate changed; refusing cleanup"
        );
        fs::remove_file(&candidate)?;
    }
    let temporary = intent_temporary(destination, descriptor);
    if entry_exists(&temporary)? {
        ensure!(
            fs::symlink_metadata(&temporary)?.file_type().is_file()
                && fs::read(&temporary)? == serde_json::to_vec(descriptor)?,
            "configuration recovery temporary record changed; refusing cleanup"
        );
        fs::remove_file(&temporary)?;
    }
    if marker_exists {
        ensure!(
            &read_descriptor(destination)? == descriptor,
            "configuration recovery record changed before cleanup"
        );
        fs::remove_file(&marker)?;
        checkpoint(destination, Stage::MarkerRemoved, &marker)?;
    }
    sync_ancestors(destination)
}

fn read_descriptor(destination: &Path) -> Result<Descriptor> {
    let marker = marker_path(destination);
    ensure!(
        fs::symlink_metadata(&marker)?.file_type().is_file(),
        "configuration recovery record must be a regular file"
    );
    let bytes = fs::read(&marker).context("read configuration recovery record")?;
    let descriptor: Descriptor = serde_json::from_slice(&bytes)
        .context("invalid configuration recovery record; manual recovery required")?;
    validate_descriptor(destination, &descriptor)?;
    Ok(descriptor)
}

fn validate_descriptor(destination: &Path, descriptor: &Descriptor) -> Result<()> {
    ensure!(
        descriptor.version == 1 && descriptor.target == filename(destination)?,
        "configuration recovery target or version mismatch"
    );
    let candidate = descriptor
        .candidate
        .strip_prefix(".alia-setup-")
        .and_then(|name| name.strip_suffix(".tmp"));
    ensure!(
        candidate.is_some_and(|value| uuid::Uuid::parse_str(value).is_ok()),
        "invalid configuration recovery candidate name"
    );
    ensure!(
        Path::new(&descriptor.candidate)
            .file_name()
            .and_then(|name| name.to_str())
            == Some(&descriptor.candidate),
        "configuration recovery candidate is not adjacent"
    );
    let fragment_name = destination
        .with_extension("d")
        .file_name()
        .context("configuration fragment name")?
        .to_owned();
    for relative in descriptor
        .fragments
        .directories
        .iter()
        .chain(descriptor.fragments.files.iter().map(|file| &file.relative))
    {
        ensure!(
            safe_relative(relative)
                && relative.components().next() == Some(Component::Normal(&fragment_name)),
            "invalid configuration fragment recovery path"
        );
    }
    Ok(())
}

fn fragment_snapshot(destination: &Path) -> Result<Fragments> {
    let root = destination.with_extension("d");
    let mut directories = Vec::new();
    if root.exists() {
        collect_directories(
            &root,
            parent(destination),
            &mut directories,
            &mut std::collections::HashSet::new(),
        )?;
    }
    directories.sort();
    let files = super::config_fragments(destination)?
        .into_iter()
        .map(|path| {
            Ok(FragmentFile {
                relative: path.strip_prefix(parent(destination))?.to_path_buf(),
                snapshot: snapshot(&path)?.context("configuration fragment disappeared")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Fragments {
        present: root.exists(),
        directories,
        files,
    })
}

fn collect_directories(
    path: &Path,
    root: &Path,
    directories: &mut Vec<PathBuf>,
    visited: &mut std::collections::HashSet<PathBuf>,
) -> Result<()> {
    let canonical = path.canonicalize()?;
    ensure!(
        visited.insert(canonical.clone()),
        "configuration fragment directory cycle"
    );
    directories.push(path.strip_prefix(root)?.to_path_buf());
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_directories(&path, root, directories, visited)?;
        }
    }
    visited.remove(&canonical);
    Ok(())
}

fn sync_fragments(destination: &Path, fragments: &Fragments) -> Result<()> {
    for fragment in &fragments.files {
        let path = parent(destination).join(&fragment.relative);
        sync_file(destination, &path, &File::open(&path)?)?;
    }
    let mut directories = fragments.directories.iter().collect::<Vec<_>>();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        sync_directory(destination, &parent(destination).join(directory))?;
    }
    Ok(())
}

fn sync_ancestors(destination: &Path) -> Result<()> {
    // An existing directory may be an unsynced remnant of an earlier failed save.
    for directory in parent(destination).ancestors() {
        sync_directory(destination, directory)?;
    }
    Ok(())
}

fn snapshot(path: &Path) -> Result<Option<FileSnapshot>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "configuration input must be a regular file: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    #[cfg(unix)]
    let (device, inode, mode) = {
        use std::os::unix::fs::MetadataExt;
        (
            Some(metadata.dev()),
            Some(metadata.ino()),
            Some(metadata.mode()),
        )
    };
    #[cfg(not(unix))]
    let (device, inode, mode) = (None, None, None);
    Ok(Some(FileSnapshot {
        digest: digest(&bytes),
        length: bytes.len() as u64,
        device,
        inode,
        mode,
    }))
}

fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("create private configuration file {}", path.display()))
}

fn destination(path: &Path) -> Result<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Ok(canonical);
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut missing = Vec::new();
    let mut existing = absolute.as_path();
    while !existing.exists() {
        missing.push(
            existing
                .file_name()
                .context("invalid configuration target")?
                .to_owned(),
        );
        existing = existing
            .parent()
            .context("configuration target has no ancestor")?;
    }
    let mut resolved = existing.canonicalize()?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("."))
}
fn filename(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .context("configuration filename must be valid UTF-8")
}
fn marker_path(destination: &Path) -> PathBuf {
    parent(destination).join(format!(
        ".{}.alia-pending.json",
        destination
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    ))
}
fn intent_temporary(destination: &Path, descriptor: &Descriptor) -> PathBuf {
    parent(destination).join(
        descriptor
            .candidate
            .replacen(".alia-setup-", ".alia-intent-", 1),
    )
}
fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}
fn entry_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn request_digest(content: &str, replace: bool, fragments: Option<&[(&str, &str)]>) -> String {
    digest(
        &serde_json::to_vec(&(1, content, replace, fragments))
            .expect("configuration request is serializable"),
    )
}
fn cleanup_preparation(candidate: &Path, fragments: Option<&Path>) {
    let _ = fs::remove_file(candidate);
    if let Some(fragments) = fragments {
        let _ = fs::remove_dir_all(fragments);
    }
}
fn sync_file(target: &Path, path: &Path, file: &File) -> Result<()> {
    checkpoint(target, Stage::FileSync, path)?;
    file.sync_all()
        .with_context(|| format!("sync configuration file {}", path.display()))
}
fn sync_directory(target: &Path, path: &Path) -> Result<()> {
    checkpoint(target, Stage::DirectorySync, path)?;
    File::open(path)?
        .sync_all()
        .with_context(|| format!("sync configuration directory {}", path.display()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    FileSync,
    DirectorySync,
    MarkerDurable,
    Published,
    MarkerRemoved,
}
fn checkpoint(target: &Path, stage: Stage, path: &Path) -> Result<()> {
    #[cfg(test)]
    testing::checkpoint(target, stage, path)?;
    #[cfg(not(test))]
    let _ = (target, stage, path);
    Ok(())
}

#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
