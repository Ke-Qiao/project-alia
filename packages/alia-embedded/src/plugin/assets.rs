//! Bounded, privately staged packages. Publication never overwrites an installation.
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::manifest::PluginManifest;

pub(super) const MAX_ARCHIVE: usize = 16 * 1024 * 1024;
const MAX_FILE: u64 = 16 * 1024 * 1024;
const MAX_EXPANDED: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 1024;
const RECORD: &str = ".alia-asset.json";

#[derive(Serialize, Deserialize)]
pub(super) struct Asset {
    pub sha256: String,
    pub archive_sha256: String,
    pub files: BTreeMap<String, String>,
}

pub(super) fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "plugin name must contain 1-128 ASCII letters, digits, '-' or '_'"
    );
    Ok(())
}

fn relative_file(name: &str) -> Result<&Path> {
    let path = Path::new(name);
    ensure!(
        !name.is_empty()
            && name.len() <= 512
            && !name.contains('\\')
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "invalid package path"
    );
    ensure!(
        path.components().count() <= 16 && name != RECORD,
        "reserved or excessively deep package path"
    );
    Ok(path)
}

pub(super) fn prepare(
    root: &Path,
    name: &str,
    version: &str,
    zip_data: &[u8],
) -> Result<(tempfile::TempDir, PluginManifest, Asset)> {
    validate_name(name)?;
    ensure!(
        !version.trim().is_empty() && version.len() <= 128,
        "plugin version is required"
    );
    ensure!(
        zip_data.len() <= MAX_ARCHIVE,
        "plugin archive exceeds 16 MiB"
    );
    let stage = tempfile::Builder::new()
        .prefix(".alia-stage-")
        .tempdir_in(root)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_data))?;
    ensure!(
        zip.len() <= MAX_FILES,
        "plugin archive has too many entries"
    );
    let mut seen = HashSet::new();
    let mut total = 0_u64;
    let mut files = BTreeMap::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().trim_end_matches('/').to_owned();
        let relative = relative_file(&name)?;
        ensure!(seen.insert(name.clone()), "duplicate archive member");
        let kind = entry.unix_mode().unwrap_or(0) & 0o170000;
        ensure!(
            kind == 0
                || (entry.is_dir() && kind == 0o040000)
                || (!entry.is_dir() && kind == 0o100000),
            "only regular files and directories are supported"
        );
        ensure!(entry.size() <= MAX_FILE, "plugin member exceeds 16 MiB");
        total = total
            .checked_add(entry.size())
            .context("archive size overflow")?;
        ensure!(total <= MAX_EXPANDED, "expanded plugin exceeds 64 MiB");
        let target = stage.path().join(relative);
        if entry.is_dir() {
            fs::create_dir_all(target)?;
            continue;
        }
        fs::create_dir_all(target.parent().context("member has no parent")?)?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&target)?;
        let mut content = Vec::new();
        (&mut entry).take(MAX_FILE + 1).read_to_end(&mut content)?;
        ensure!(
            content.len() as u64 == entry.size() && content.len() as u64 <= MAX_FILE,
            "plugin member size differs from header or exceeds budget"
        );
        file.write_all(&content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(
                if entry.unix_mode().unwrap_or(0) & 0o111 != 0 {
                    0o500
                } else {
                    0o400
                },
            ))?;
        }
        file.sync_all()?;
        files.insert(name, format!("{:x}", Sha256::digest(&content)));
    }
    let manifest = PluginManifest::load(&stage.path().join("manifest.toml"))?;
    ensure!(
        manifest.plugin.name == name && manifest.plugin.version == version,
        "manifest name/version must match installation request"
    );
    relative_file(&manifest.plugin.entry)?;
    ensure!(
        files.contains_key(&manifest.plugin.entry),
        "entry must be a regular package file"
    );
    let asset = Asset {
        sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(&files)?)),
        archive_sha256: format!("{:x}", Sha256::digest(zip_data)),
        files,
    };
    let record_bytes = serde_json::to_vec(&asset)?;
    ensure!(
        record_bytes.len() <= 256 * 1024,
        "asset record exceeds budget"
    );
    let mut record = File::create(stage.path().join(RECORD))?;
    record.write_all(&record_bytes)?;
    record.sync_all()?;
    sync_tree(stage.path())?;
    verify(stage.path(), Some(&asset.sha256))?;
    Ok((stage, manifest, asset))
}

fn sync_tree(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path())?;
        }
    }
    File::open(path)?.sync_all()?;
    Ok(())
}

pub(super) fn verify(dir: &Path, expected: Option<&str>) -> Result<(PluginManifest, Asset)> {
    ensure!(
        !fs::symlink_metadata(dir)?.file_type().is_symlink(),
        "plugin directory must not be a symlink"
    );
    ensure!(
        fs::symlink_metadata(dir.join(RECORD))?
            .file_type()
            .is_file(),
        "asset record must be a regular file"
    );
    let mut record = Vec::new();
    File::open(dir.join(RECORD))?
        .take(256 * 1024 + 1)
        .read_to_end(&mut record)?;
    ensure!(record.len() <= 256 * 1024, "asset record exceeds budget");
    let asset: Asset = serde_json::from_slice(&record)?;
    ensure!(
        asset.sha256.len() == 64 && asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid asset digest"
    );
    ensure!(
        expected.is_none_or(|digest| digest == asset.sha256),
        "installed content identity changed"
    );
    ensure!(asset.files.len() <= MAX_FILES, "too many asset files");
    let mut total = 0_u64;
    let root = dir.canonicalize()?;
    let mut actual = BTreeMap::new();
    let mut pending = vec![root.clone()];
    let mut entries = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            entries += 1;
            ensure!(
                entries <= MAX_FILES + 1,
                "installed package has too many entries"
            );
            let entry = entry?;
            let path = entry.path();
            let name = path
                .strip_prefix(&root)?
                .to_str()
                .context("non UTF-8 package path")?
                .to_owned();
            if name == RECORD {
                ensure!(entry.file_type()?.is_file(), "invalid asset record");
                continue;
            }
            relative_file(&name)?;
            let kind = entry.file_type()?;
            ensure!(
                kind.is_dir() || kind.is_file(),
                "package contains a link or special file"
            );
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            let metadata = entry.metadata()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(metadata.nlink() == 1, "package contains hard links");
            }
            total = total
                .checked_add(metadata.len())
                .context("package size overflow")?;
            ensure!(
                metadata.len() <= MAX_FILE && total <= MAX_EXPANDED && actual.len() < MAX_FILES,
                "installed package exceeds budget"
            );
            let mut content = Vec::new();
            File::open(path)?
                .take(MAX_FILE + 1)
                .read_to_end(&mut content)?;
            ensure!(
                content.len() as u64 <= MAX_FILE,
                "installed member exceeds budget"
            );
            actual.insert(name, format!("{:x}", Sha256::digest(&content)));
        }
    }
    ensure!(actual == asset.files, "installed package contents changed");
    ensure!(
        format!("{:x}", Sha256::digest(serde_json::to_vec(&actual)?)) == asset.sha256,
        "installed content digest does not match its identity"
    );
    let manifest = PluginManifest::load(&dir.join("manifest.toml"))?;
    validate_name(&manifest.plugin.name)?;
    relative_file(&manifest.plugin.entry)?;
    ensure!(
        asset.files.contains_key(&manifest.plugin.entry),
        "entry must be a regular package file"
    );
    Ok((manifest, asset))
}
