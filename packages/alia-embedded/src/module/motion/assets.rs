//! Approved animation bytes are read once; verification and parsing share them.
use std::io::Read;
use std::path::Path;

use alia_core::embodiment::motion::MotionAnimationAsset;
use anyhow::{Context, Result, ensure};

use crate::config::MotionConfig;

pub(super) fn read_approved(
    config: &MotionConfig,
    asset: &MotionAnimationAsset,
) -> Result<Vec<u8>> {
    ensure!(
        config
            .approved_animations
            .iter()
            .any(|approved| approved == asset),
        "animation is not in the local approved catalog"
    );
    let file = open_animation(&config.animation_dir, &asset.file)?;
    read_verified(file, asset)
}

fn read_verified(file: std::fs::File, asset: &MotionAnimationAsset) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(u64::from(asset.bytes) + 1)
        .read_to_end(&mut bytes)?;
    asset.verify_bytes(&bytes).map_err(anyhow::Error::msg)?;
    Ok(bytes)
}

#[cfg(unix)]
fn open_animation(directory: &Path, name: &str) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    // Reject a symlink in the final configured directory component. Once open,
    // this fd pins the directory even if its path is renamed/replaced.
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(directory)
        .context("opening approved animation directory")?;
    open_animation_at(&directory, name)
}

#[cfg(unix)]
fn open_animation_at(directory: &std::fs::File, name: &str) -> Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let name = std::ffi::CString::new(name)?;
    // The validated single component and directory fd remain alive across openat.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("opening approved animation");
    }
    // Transfer the successful descriptor immediately; all later errors close it.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    ensure!(
        file.metadata()?.is_file(),
        "animation must be a regular file"
    );
    Ok(file)
}

#[cfg(not(unix))]
fn open_animation(_: &Path, _: &str) -> Result<std::fs::File> {
    anyhow::bail!("safe animation asset opening is unavailable on this platform")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{OpenOptionsExt, symlink};

    #[test]
    fn opened_directory_remains_anchored_when_its_path_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("original");
        let moved = root.path().join("moved");
        let outside = root.path().join("outside");
        std::fs::create_dir(&directory).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(directory.join("path.anim"), "frames\n1 0 0\n").unwrap();
        std::fs::write(outside.join("path.anim"), "frames\n7 0 0\n").unwrap();
        let directory_fd = std::fs::File::open(&directory).unwrap();
        std::fs::rename(&directory, &moved).unwrap();
        symlink(&outside, &directory).unwrap();
        let mut file = open_animation_at(&directory_fd, "path.anim").unwrap();
        let mut actual = String::new();
        file.read_to_string(&mut actual).unwrap();
        assert_eq!(actual, "frames\n1 0 0\n");
        assert!(open_animation(&directory, "path.anim").is_err());
    }

    #[test]
    fn symlink_replacement_is_rejected_and_regular_fd_is_nonblocking_and_cloexec() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = directory.path().join("path.anim");
        std::fs::write(&path, "frames\n1 0 0\n").unwrap();
        let directory_fd = std::fs::File::open(directory.path()).unwrap();
        let opened = open_animation_at(&directory_fd, "path.anim").unwrap();
        // Pure temporary regular files: verify the flags that avoid FIFO waits.
        let status = unsafe { libc::fcntl(opened.as_raw_fd(), libc::F_GETFL) };
        let descriptor = unsafe { libc::fcntl(opened.as_raw_fd(), libc::F_GETFD) };
        assert!(status >= 0 && descriptor >= 0);
        assert_ne!(status & libc::O_NONBLOCK, 0);
        assert_ne!(descriptor & libc::FD_CLOEXEC, 0);
        std::fs::remove_file(&path).unwrap();
        let target = outside.path().join("outside.anim");
        std::fs::write(&target, "frames\n7 0 0\n").unwrap();
        symlink(&target, &path).unwrap();
        assert!(open_animation_at(&directory_fd, "path.anim").is_err());
    }

    #[test]
    fn guarded_temporary_fifo_is_rejected_as_non_regular() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pipe.anim");
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        // This nonblocking guard supplies both ends, so even a regression that
        // drops O_NONBLOCK from the tested read cannot hang the test runner.
        let _guard = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
            .unwrap();
        assert!(
            open_animation(directory.path(), "pipe.anim")
                .unwrap_err()
                .to_string()
                .contains("regular file")
        );
    }

    #[test]
    fn opened_file_bytes_are_verified_without_reopening_the_replaced_name() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("path.anim");
        let original = b"frames\n1 0 0\n";
        let asset = MotionAnimationAsset {
            file: "path.anim".into(),
            version: 1,
            sha256: alia_core::embodiment::motion::motion_animation_sha256(original),
            bytes: original.len() as u32,
        };
        std::fs::write(&path, original).unwrap();
        let original_file = open_animation(directory.path(), "path.anim").unwrap();
        std::fs::rename(&path, directory.path().join("original.anim")).unwrap();
        std::fs::write(&path, b"frames\n7 0 0\n").unwrap();
        assert_eq!(
            read_verified(original_file, &asset).unwrap().as_slice(),
            original.as_slice()
        );
        let replaced = open_animation(directory.path(), "path.anim").unwrap();
        assert!(read_verified(replaced, &asset).is_err());
        std::fs::write(&path, original).unwrap();
        let same_inode = open_animation(directory.path(), "path.anim").unwrap();
        std::fs::write(&path, b"frames\n7 0 0\n").unwrap();
        assert!(read_verified(same_inode, &asset).is_err());
    }
}
