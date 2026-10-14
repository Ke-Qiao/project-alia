//! Cue uploads create new files beneath the startup directory without following links.
use std::path::{Component, Path, PathBuf};

use tonic::Status;
use uuid::Uuid;

pub(super) fn write_uploads(
    root: &Path,
    slug: &str,
    uploads: &[(&str, &[u8])],
) -> Result<Vec<PathBuf>, Status> {
    let names = uploads
        .iter()
        .map(|(requested, _)| {
            let requested = requested.trim();
            let name = if requested.is_empty() {
                format!("direct-response-{}.wav", Uuid::new_v4())
            } else {
                requested.to_owned()
            };
            validate_leaf(&name)?;
            Ok(name)
        })
        .collect::<Result<Vec<_>, Status>>()?;
    if names.is_empty() {
        return Ok(Vec::new());
    }
    if !slug.is_empty() {
        validate_leaf(slug)?;
    }
    write_new_files(root, slug, &names, uploads)?;
    Ok(names
        .into_iter()
        .map(|name| root.join(slug).join(name))
        .collect())
}

fn validate_leaf(name: &str) -> Result<(), Status> {
    let mut components = Path::new(name).components();
    if name.contains('\0')
        || !matches!(components.next(), Some(Component::Normal(component)) if component == name)
        || components.next().is_some()
    {
        return Err(Status::invalid_argument(
            "cue upload name must be a single filename",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn write_new_files(
    root: &Path,
    slug: &str,
    names: &[String],
    uploads: &[(&str, &[u8])],
) -> Result<(), Status> {
    use std::ffi::CString;
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    fn error(error: impl std::fmt::Display) -> Status {
        Status::internal(format!("cue upload: {error}"))
    }

    fn directory_at(parent: &File, name: &std::ffi::OsStr) -> Result<File, Status> {
        let name = CString::new(name.as_bytes()).map_err(error)?;
        let flags = libc::O_RDONLY
            | libc::O_DIRECTORY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK;
        let mut fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
            if created < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(error(std::io::Error::last_os_error()));
            }
            fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        }
        if fd < 0 {
            return Err(error(std::io::Error::last_os_error()));
        }
        // Each successful descriptor is immediately owned, including on later errors.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    struct CreatedFiles {
        directory: File,
        names: Vec<CString>,
        complete: bool,
    }
    impl Drop for CreatedFiles {
        fn drop(&mut self) {
            if !self.complete {
                for name in &self.names {
                    if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) } < 0 {
                        tracing::error!(error = %std::io::Error::last_os_error(), "failed to remove incomplete cue upload");
                    }
                }
            }
        }
    }

    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(if root.is_absolute() { "/" } else { "." })
        .map_err(error)?;
    for component in root.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            // Parent components come only from trusted startup configuration, never uploads.
            Component::Normal(_) | Component::ParentDir => {
                directory = directory_at(&directory, component.as_os_str())?;
            }
            Component::Prefix(_) => {
                return Err(Status::invalid_argument("unsupported cue directory prefix"));
            }
        }
    }
    if !slug.is_empty() {
        directory = directory_at(&directory, slug.as_ref())?;
    }
    let mut created = CreatedFiles {
        directory,
        names: Vec::new(),
        complete: false,
    };
    for (name, (_, bytes)) in names.iter().zip(uploads) {
        let name = CString::new(name.as_bytes()).map_err(error)?;
        let fd = unsafe {
            libc::openat(
                created.directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY
                    | libc::O_CREAT
                    | libc::O_EXCL
                    | libc::O_NOFOLLOW
                    | libc::O_CLOEXEC
                    | libc::O_NONBLOCK,
                0o600,
            )
        };
        if fd < 0 {
            return Err(error(std::io::Error::last_os_error()));
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        created.names.push(name);
        file.write_all(bytes).map_err(error)?;
    }
    // From here a database outcome may be unknown; deleting files could break a committed rule.
    created.complete = true;
    Ok(())
}

#[cfg(not(unix))]
fn write_new_files(_: &Path, _: &str, _: &[String], _: &[(&str, &[u8])]) -> Result<(), Status> {
    Err(Status::unimplemented(
        "safe cue uploads are unavailable on this platform",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn invalid_later_filename_is_rejected_before_any_writes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap().join("audio");
        for path in [
            "../config.toml",
            "/config.toml",
            "nested/file.wav",
            ".",
            "..",
            "a/",
            "a\0b",
        ] {
            assert!(
                write_uploads(&root, "cue", &[("valid.wav", b"first"), (path, b"bad")]).is_err()
            );
            assert!(!root.exists());
        }
    }

    #[test]
    fn new_uploads_keep_existing_files_and_rollback_a_partial_batch() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap().join("audio");
        let paths =
            write_uploads(&root, "cue", &[("", b"first"), ("named.wav", b"second")]).unwrap();
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"first");
        assert_eq!(std::fs::read(&paths[1]).unwrap(), b"second");
        assert!(
            write_uploads(
                &root,
                "cue",
                &[("new.wav", b"new"), ("named.wav", b"overwrite")]
            )
            .is_err()
        );
        assert!(!root.join("cue/new.wav").exists());
        assert_eq!(std::fs::read(&paths[1]).unwrap(), b"second");
        assert_eq!(std::fs::read_dir(root.join("cue")).unwrap().count(), 2);
    }

    #[test]
    fn directory_links_leaf_links_and_hardlinks_cannot_overwrite_config() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().canonicalize().unwrap();
        let root = parent.join("audio");
        std::fs::create_dir(&root).unwrap();
        let config = parent.join("embedded.toml");
        std::fs::write(&config, b"local trusted config").unwrap();
        symlink(&parent, root.join("linked")).unwrap();
        assert!(write_uploads(&root, "linked", &[("embedded.toml", b"bad")]).is_err());
        symlink(&config, root.join("leaf.wav")).unwrap();
        assert!(write_uploads(&root, "", &[("leaf.wav", b"bad")]).is_err());
        std::fs::hard_link(&config, root.join("hard.wav")).unwrap();
        assert!(write_uploads(&root, "", &[("hard.wav", b"bad")]).is_err());
        symlink(&root, parent.join("root-alias")).unwrap();
        assert!(write_uploads(&parent.join("root-alias"), "", &[("new.wav", b"bad")]).is_err());
        assert!(write_uploads(&parent.join("root-alias/sub"), "", &[("new.wav", b"bad")]).is_err());
        assert!(!root.join("sub").exists());
        assert_eq!(std::fs::read(config).unwrap(), b"local trusted config");
    }

    #[test]
    fn trusted_startup_parent_components_work_without_allowing_upload_traversal() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().canonicalize().unwrap();
        std::fs::create_dir(parent.join("config")).unwrap();
        let root = parent.join("config/../audio");
        let paths = write_uploads(&root, "cue", &[("sample.wav", b"data")]).unwrap();
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"data");
        assert_eq!(
            std::fs::read(parent.join("audio/cue/sample.wav")).unwrap(),
            b"data"
        );
    }
}
