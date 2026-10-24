use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use tokio_util::sync::CancellationToken;

pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(100);
const WRITE_TIMEOUT: Duration = Duration::from_millis(250);

pub(super) struct Serial {
    file: File,
    exclusive: bool,
}

impl Serial {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
            .with_context(|| format!("opening R1 UART {}", path.display()))?;
        ensure!(
            file.metadata()?.file_type().is_char_device(),
            "R1 UART must be a character device"
        );
        let fd = file.as_raw_fd();
        syscall(
            unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) },
            "locking R1 UART",
        )?;
        let mut serial = Self {
            file,
            exclusive: false,
        };
        syscall(
            unsafe { libc::ioctl(fd, libc::TIOCEXCL) },
            "claiming exclusive R1 UART",
        )?;
        serial.exclusive = true;
        configure(fd)?;
        let lines = libc::TIOCM_DTR | libc::TIOCM_RTS;
        // Drivers may assert modem lines during open; never deliberately pulse them.
        syscall(
            unsafe { libc::ioctl(fd, libc::TIOCMBIC, &lines) },
            "clearing R1 DTR/RTS",
        )?;
        serial.clear_input()?;
        Ok(serial)
    }

    pub(super) fn clear_input(&self) -> Result<()> {
        syscall(
            unsafe { libc::tcflush(self.file.as_raw_fd(), libc::TCIFLUSH) },
            "discarding R1 UART input",
        )
    }

    pub(super) fn read(&mut self, bytes: &mut [u8]) -> Result<usize> {
        if !self.ready(libc::POLLIN, POLL_INTERVAL)? {
            return Ok(0);
        }
        match self.file.read(bytes) {
            Ok(0) => bail!("R1 UART disconnected"),
            Ok(count) => Ok(count),
            Err(error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) =>
            {
                Ok(0)
            }
            Err(error) => Err(error).context("reading R1 UART"),
        }
    }

    pub(super) fn write(&mut self, mut bytes: &[u8], cancel: &CancellationToken) -> Result<()> {
        let deadline = Instant::now() + WRITE_TIMEOUT;
        while !bytes.is_empty() {
            ensure!(!cancel.is_cancelled(), "R1 UART write cancelled");
            let remaining = deadline.saturating_duration_since(Instant::now());
            ensure!(!remaining.is_zero(), "R1 UART write timed out");
            if !self.ready(libc::POLLOUT, remaining.min(POLL_INTERVAL))? {
                continue;
            }
            match self.file.write(bytes) {
                Ok(0) => bail!("R1 UART accepted no output bytes"),
                Ok(count) => bytes = &bytes[count..],
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(error) => return Err(error).context("writing R1 UART"),
            }
        }
        Ok(())
    }

    fn ready(&self, events: libc::c_short, timeout: Duration) -> Result<bool> {
        let mut fd = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut fd, 1, timeout.as_millis().clamp(1, 100) as i32) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(error).context("polling R1 UART");
        }
        ensure!(
            fd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) == 0,
            "R1 UART disconnected or failed"
        );
        Ok(fd.revents & events != 0)
    }
}

impl Drop for Serial {
    fn drop(&mut self) {
        if self.exclusive {
            // Closing our fd alone retains TIOCEXCL while another tty fd remains open.
            if unsafe { libc::ioctl(self.file.as_raw_fd(), libc::TIOCNXCL) } < 0 {
                tracing::warn!(error = %std::io::Error::last_os_error(), "releasing R1 UART exclusivity failed");
            }
        }
    }
}

fn syscall(result: libc::c_int, operation: &'static str) -> Result<()> {
    if result < 0 {
        return Err(std::io::Error::last_os_error()).context(operation);
    }
    Ok(())
}

fn configure(fd: std::os::fd::RawFd) -> Result<()> {
    let mut attributes = unsafe { std::mem::zeroed::<libc::termios>() };
    syscall(
        unsafe { libc::tcgetattr(fd, &mut attributes) },
        "reading R1 UART settings",
    )?;
    unsafe { libc::cfmakeraw(&mut attributes) };
    attributes.c_cflag &=
        !(libc::CSIZE | libc::PARENB | libc::CSTOPB | libc::CRTSCTS | libc::HUPCL);
    attributes.c_cflag |= libc::CS8 | libc::CLOCAL | libc::CREAD;
    attributes.c_iflag &= !(libc::IXON | libc::IXOFF | libc::IXANY);
    attributes.c_cc[libc::VMIN] = 0;
    attributes.c_cc[libc::VTIME] = 0;
    syscall(
        unsafe { libc::cfsetispeed(&mut attributes, libc::B115200) },
        "setting R1 input baud",
    )?;
    syscall(
        unsafe { libc::cfsetospeed(&mut attributes, libc::B115200) },
        "setting R1 output baud",
    )?;
    syscall(
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &attributes) },
        "applying R1 UART settings",
    )
}

#[cfg(any(test, feature = "test-support"))]
pub(super) fn pty() -> (File, Serial, std::path::PathBuf) {
    use std::os::fd::FromRawFd;
    let (mut master, mut slave) = (-1, -1);
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    let master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    let mut path = [0 as libc::c_char; 128];
    assert_eq!(
        unsafe { libc::ttyname_r(slave.as_raw_fd(), path.as_mut_ptr(), path.len()) },
        0
    );
    let path = unsafe { std::ffi::CStr::from_ptr(path.as_ptr()) }
        .to_str()
        .unwrap()
        .into();
    configure(slave.as_raw_fd()).unwrap();
    for file in [&master, &slave] {
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
    }
    (
        master,
        Serial {
            file: slave,
            exclusive: false,
        },
        path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_owner_release_allows_reopen_while_pty_master_remains() {
        let (_master, mut serial, path) = pty();
        assert_eq!(
            unsafe { libc::ioctl(serial.file.as_raw_fd(), libc::TIOCEXCL) },
            0
        );
        serial.exclusive = true;
        let error = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EBUSY));
        drop(serial);
        assert!(OpenOptions::new().read(true).write(true).open(path).is_ok());
    }
}
