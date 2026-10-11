//! Atomic snapshots for runtime settings; failed saves remain explicit retries.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

#[derive(Debug)]
pub struct SettingsSaveError {
    published: bool,
    source: anyhow::Error,
}

impl SettingsSaveError {
    /// True only after replacement of the target file succeeded.
    pub fn was_published(&self) -> bool {
        self.published
    }

    pub(crate) fn not_published(error: impl Into<anyhow::Error>) -> Self {
        Self {
            published: false,
            source: error.into(),
        }
    }
}

impl std::fmt::Display for SettingsSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status = if self.published {
            "settings snapshot was published, but durability is unknown; reload and retry"
        } else {
            "settings snapshot was not published"
        };
        write!(f, "{status}: {:#}", self.source)
    }
}
impl std::error::Error for SettingsSaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub(super) fn save(path: &Path, content: &str) -> Result<(), SettingsSaveError> {
    let target = destination(path).map_err(SettingsSaveError::not_published)?;
    match fs::metadata(&target) {
        Ok(metadata) => ensure_regular(&metadata).map_err(SettingsSaveError::not_published)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(SettingsSaveError::not_published(error)),
    }
    let original = match OpenOptions::new().write(true).open(&target) {
        Ok(file) => {
            let metadata = file.metadata().map_err(SettingsSaveError::not_published)?;
            ensure_regular(&metadata).map_err(SettingsSaveError::not_published)?;
            Some(metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(SettingsSaveError::not_published(error)),
    };
    let temporary = target.with_file_name(format!(".alia-settings-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(SettingsSaveError::not_published)?;
    let mut published = false;
    let result = (|| -> Result<()> {
        if let Some(original) = &original {
            check_owner(original, &file.metadata()?)?;
        }
        file.write_all(content.as_bytes())?;
        // Finish writing privately before restoring an operator's existing mode.
        if let Some(original) = original {
            file.set_permissions(original.permissions())?;
        }
        checkpoint(&target, Stage::FileSync, &temporary)?;
        file.sync_all().context("sync settings snapshot")?;
        drop(file);
        checkpoint(&target, Stage::Prepared, &temporary)?;
        fs::rename(&temporary, &target).context("replace settings snapshot")?;
        published = true;
        checkpoint(&target, Stage::Published, &target)?;
        // Earlier failed saves may have left visible but unsynced parent entries.
        for ancestor in target
            .parent()
            .context("settings target has no parent")?
            .ancestors()
        {
            checkpoint(&target, Stage::DirectorySync, ancestor)?;
            File::open(ancestor)?
                .sync_all()
                .with_context(|| format!("sync settings directory {}", ancestor.display()))?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(()),
        Err(mut source) => {
            if !published && let Err(cleanup) = fs::remove_file(&temporary) {
                if cleanup.kind() != std::io::ErrorKind::NotFound {
                    source = source
                        .context(format!("settings temporary cleanup also failed: {cleanup}"));
                }
            }
            Err(SettingsSaveError { published, source })
        }
    }
}

fn ensure_regular(metadata: &fs::Metadata) -> Result<()> {
    ensure!(metadata.is_file(), "settings target must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.nlink() == 1,
            "cannot atomically replace settings with multiple hard links"
        );
    }
    Ok(())
}

fn check_owner(original: &fs::Metadata, candidate: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            original.uid() == candidate.uid() && original.gid() == candidate.gid(),
            "cannot atomically replace settings without changing its owner or group"
        );
    }
    #[cfg(not(unix))]
    let _ = (original, candidate);
    Ok(())
}

fn destination(path: &Path) -> Result<PathBuf> {
    let mut path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut visited = std::collections::HashSet::new();
    let mut following_link = false;
    loop {
        let name = path
            .file_name()
            .context("invalid settings filename")?
            .to_owned();
        let parent = path.parent().context("settings target has no parent")?;
        if following_link {
            ensure!(
                fs::metadata(parent)?.is_dir(),
                "settings symlink target parent must already be a directory"
            );
        } else {
            fs::create_dir_all(parent)?;
        }
        path = parent.canonicalize()?.join(name);
        ensure!(visited.insert(path.clone()), "settings symlink cycle");
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link = fs::read_link(&path)?;
                path = if link.is_absolute() {
                    link
                } else {
                    path.parent().unwrap().join(link)
                };
                following_link = true;
            }
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(path),
            Err(error) => return Err(error.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    FileSync,
    Prepared,
    Published,
    DirectorySync,
}
fn checkpoint(target: &Path, stage: Stage, path: &Path) -> Result<()> {
    #[cfg(test)]
    tests::checkpoint(target, stage, path)?;
    #[cfg(not(test))]
    let _ = (target, stage, path);
    Ok(())
}

#[cfg(test)]
mod tests;
