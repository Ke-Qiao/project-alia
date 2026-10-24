use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, ensure};

#[derive(Debug)]
pub struct SettingsSaveError {
    published: bool,
    source: anyhow::Error,
}

impl SettingsSaveError {
    pub fn was_published(&self) -> bool {
        self.published
    }
}

impl std::fmt::Display for SettingsSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status = if self.published {
            "embedded settings were published, but durability is unknown; reload and retry"
        } else {
            "embedded settings were not published"
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
    save_with_sync(path, content, |parent| {
        // A previous save may have created a directory without confirming durability.
        for ancestor in parent.ancestors() {
            File::open(ancestor)?
                .sync_all()
                .with_context(|| format!("sync settings directory {}", ancestor.display()))?;
        }
        Ok(())
    })
}

fn save_with_sync(
    path: &Path,
    content: &str,
    sync_directory: impl FnOnce(&Path) -> Result<()>,
) -> Result<(), SettingsSaveError> {
    let mut published = false;
    let result = (|| -> Result<()> {
        let path = std::path::absolute(path)?;
        // Preserve configured symlinks; a dangling link must be repaired explicitly.
        let target = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => path.canonicalize()?,
            Ok(_) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => path,
            Err(error) => return Err(error.into()),
        };
        let parent = target.parent().context("settings target has no parent")?;
        fs::create_dir_all(parent)?;
        let original = match OpenOptions::new().write(true).open(&target) {
            Ok(file) => {
                let metadata = file.metadata()?;
                ensure!(metadata.is_file(), "settings target must be a regular file");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    ensure!(
                        metadata.nlink() == 1,
                        "cannot atomically replace settings with multiple hard links"
                    );
                }
                Some(metadata)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        if let Some(original) = &original {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let candidate = temporary.as_file().metadata()?;
                ensure!(
                    original.uid() == candidate.uid() && original.gid() == candidate.gid(),
                    "cannot atomically replace settings without changing its owner or group"
                );
            }
        }
        temporary
            .write_all(content.as_bytes())
            .context("write settings snapshot")?;
        if let Some(original) = original {
            temporary
                .as_file()
                .set_permissions(original.permissions())?;
        }
        temporary
            .as_file()
            .sync_all()
            .context("sync settings snapshot")?;
        temporary
            .persist(&target)
            .context("replace settings snapshot")?;
        published = true;
        sync_directory(parent)
    })();
    result.map_err(|source| SettingsSaveError { published, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_is_atomic_and_directory_sync_failure_is_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("settings.toml");
        save(&target, "version = 1\n").unwrap();
        let mut previous = File::open(&target).unwrap();
        let error = save_with_sync(&target, "version = 2\n", |_| {
            anyhow::bail!("injected sync failure")
        })
        .unwrap_err();
        assert!(error.was_published());
        assert!(error.to_string().contains("durability is unknown"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "version = 2\n");
        let mut old = String::new();
        std::io::Read::read_to_string(&mut previous, &mut old).unwrap();
        assert_eq!(old, "version = 1\n");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_publication_preserves_the_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("settings.toml");
        fs::create_dir(&target).unwrap();
        let error = save(&target, "version = 2\n").unwrap_err();
        assert!(!error.was_published());
        assert!(target.is_dir());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
