use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use tracing::info;

use crate::config::MotionConfig;

/// These are host submission boundaries. None establishes motor position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServoBoundary {
    SerialHostWrite,
    I2cRegisterWrite,
    Simulated,
}

#[derive(Clone, Debug)]
pub struct ServoWrite {
    pub boundary: ServoBoundary,
    pub submitted_channels: Vec<u32>,
    pub uncertain_channels: Vec<u32>,
    pub accepted_bytes: usize,
    pub error: Option<String>,
    pub cancelled: bool,
}
impl ServoWrite {
    fn empty(boundary: ServoBoundary) -> Self {
        Self {
            boundary,
            submitted_channels: Vec::new(),
            uncertain_channels: Vec::new(),
            accepted_bytes: 0,
            error: None,
            cancelled: false,
        }
    }
    pub fn fully_submitted(&self, channels: usize) -> bool {
        self.error.is_none()
            && !self.cancelled
            && self.uncertain_channels.is_empty()
            && self.submitted_channels.len() == channels
    }
}

pub trait ServoBackend: Send + Sync {
    /// Called only by the owned native motion writer. Cancellation is checked at
    /// each new OS write boundary, never by abandoning an entered syscall.
    fn submit_frame(&self, angles: &[f32], cancelled: &dyn Fn() -> bool) -> ServoWrite;
    /// Release lazily opened native devices on the same owned writer.
    fn close(&self) {}
}

fn validate_angles(angles: &[f32], servo_count: usize) -> Result<()> {
    ensure!(
        !angles.is_empty() && angles.len() <= servo_count,
        "servo frame dimensions are invalid"
    );
    ensure!(
        angles
            .iter()
            .all(|angle| angle.is_finite() && (0.0..=180.0).contains(angle)),
        "servo frame contains invalid angles"
    );
    Ok(())
}

/// Serial framing may span short host writes. Preserve the accepted byte prefix
/// on failure; a partial command has no provable per-channel effect.
fn submit_serial(
    writer: &mut impl Write,
    command: &[u8],
    channels: usize,
    cancelled: &dyn Fn() -> bool,
) -> ServoWrite {
    let mut result = ServoWrite::empty(ServoBoundary::SerialHostWrite);
    while result.accepted_bytes < command.len() {
        if cancelled() {
            result.cancelled = true;
            break;
        }
        match writer.write(&command[result.accepted_bytes..]) {
            Ok(0) => {
                result.error = Some("serial write accepted zero bytes".into());
                break;
            }
            Ok(count) => result.accepted_bytes += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                result.error = Some(format!("serial write: {error}"));
                break;
            }
        }
    }
    if result.accepted_bytes == command.len() {
        result.submitted_channels.extend(0..channels as u32);
        // A File flush is a host operation, not an MCU acknowledgement. Written
        // bytes remain submitted evidence even if a later flush reports failure.
        if let Err(error) = writer.flush() {
            result.error = Some(format!("serial flush: {error}"));
        }
    } else if result.accepted_bytes != 0 {
        result.uncertain_channels.extend(0..channels as u32);
    }
    result
}

/// One I2C register transaction must never use write_all: retrying the suffix of
/// a short write would reinterpret data bytes as a new register address.
fn write_i2c_transaction(writer: &mut impl Write, bytes: &[u8]) -> Result<usize> {
    let count = writer.write(bytes).context("I2C register transaction")?;
    ensure!(
        count == bytes.len(),
        "I2C transaction accepted only {count}/{} bytes; channel effect is unknown",
        bytes.len()
    );
    Ok(count)
}

pub fn create_servo_backend(config: &MotionConfig) -> Result<Box<dyn ServoBackend>> {
    if !config.hardware_enabled {
        return Ok(Box::new(MockServoBackend::new(config.servo_count)));
    }

    let boundary = match config.servo_backend.as_str() {
        "serial" | "arduino" | "arduino_serial" => ServoBoundary::SerialHostWrite,
        "pca9685" | "i2c" => ServoBoundary::I2cRegisterWrite,
        "mock" => return Ok(Box::new(MockServoBackend::new(config.servo_count))),
        other => bail!("unknown motion.servo_backend '{other}'; expected serial, pca9685, or mock"),
    };
    Ok(Box::new(DeferredServoBackend {
        config: config.clone(),
        boundary,
        initialized: Mutex::new(None),
    }))
}

/// Construction validates configuration without opening a device. Native open,
/// initialization, writes and destruction all run within the owned writer.
struct DeferredServoBackend {
    config: MotionConfig,
    boundary: ServoBoundary,
    initialized: Mutex<Option<Box<dyn ServoBackend>>>,
}

impl ServoBackend for DeferredServoBackend {
    fn submit_frame(&self, angles: &[f32], cancelled: &dyn Fn() -> bool) -> ServoWrite {
        let mut result = ServoWrite::empty(self.boundary);
        if let Err(error) = validate_angles(angles, self.config.servo_count) {
            result.error = Some(error.to_string());
            return result;
        }
        if cancelled() {
            result.cancelled = true;
            return result;
        }
        let mut initialized = self
            .initialized
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if initialized.is_none() {
            let backend: Result<Box<dyn ServoBackend>> = match self.boundary {
                ServoBoundary::SerialHostWrite => SerialServoBackend::new(&self.config)
                    .map(|backend| Box::new(backend) as Box<dyn ServoBackend>),
                ServoBoundary::I2cRegisterWrite => {
                    Pca9685ServoBackend::new(&self.config, cancelled)
                        .map(|backend| Box::new(backend) as Box<dyn ServoBackend>)
                }
                ServoBoundary::Simulated => unreachable!("mock backend is immediate"),
            };
            match backend {
                Ok(backend) => *initialized = Some(backend),
                Err(error) => {
                    result.error = Some(format!("native motion backend initialization: {error:#}"));
                    return result;
                }
            }
        }
        initialized
            .as_ref()
            .expect("initialized backend")
            .submit_frame(angles, cancelled)
    }

    fn close(&self) {
        self.initialized
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }
}

pub struct MockServoBackend {
    servo_count: usize,
    last_angles: Mutex<Vec<f32>>,
}

impl MockServoBackend {
    pub fn new(servo_count: usize) -> Self {
        Self {
            servo_count,
            last_angles: Mutex::new(vec![0.0; servo_count]),
        }
    }
}

impl ServoBackend for MockServoBackend {
    fn submit_frame(&self, angles: &[f32], cancelled: &dyn Fn() -> bool) -> ServoWrite {
        let mut result = ServoWrite::empty(ServoBoundary::Simulated);
        if let Err(error) = validate_angles(angles, self.servo_count) {
            result.error = Some(error.to_string());
            return result;
        }
        if cancelled() {
            result.cancelled = true;
            return result;
        }
        let mut last = self
            .last_angles
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        last[..angles.len()].copy_from_slice(angles);
        result.submitted_channels.extend(0..angles.len() as u32);
        result
    }
}

pub struct SerialServoBackend {
    servo_count: usize,
    file: Mutex<std::fs::File>,
}

impl SerialServoBackend {
    pub fn new(config: &MotionConfig) -> Result<Self> {
        let file =
            open_serial_port(&config.serial_path, config.serial_baud_rate).with_context(|| {
                format!(
                    "opening Arduino servo UART: {}",
                    config.serial_path.display()
                )
            })?;
        if config.serial_startup_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(config.serial_startup_delay_ms));
        }
        info!(
            path = %config.serial_path.display(),
            baud_rate = config.serial_baud_rate,
            servo_count = config.servo_count,
            "Arduino servo UART connected"
        );
        Ok(Self {
            servo_count: config.servo_count,
            file: Mutex::new(file),
        })
    }
}

impl ServoBackend for SerialServoBackend {
    fn submit_frame(&self, angles: &[f32], cancelled: &dyn Fn() -> bool) -> ServoWrite {
        if let Err(error) = validate_angles(angles, self.servo_count) {
            let mut result = ServoWrite::empty(ServoBoundary::SerialHostWrite);
            result.error = Some(error.to_string());
            return result;
        }
        let mut file = self.file.lock().unwrap_or_else(|error| error.into_inner());
        submit_serial(
            &mut *file,
            serial_frame_command(angles).as_bytes(),
            angles.len(),
            cancelled,
        )
    }
}

pub struct Pca9685ServoBackend {
    servo_count: usize,
    boards: Vec<Mutex<Pca9685Device>>,
}

impl Pca9685ServoBackend {
    pub fn new(config: &MotionConfig, cancelled: &dyn Fn() -> bool) -> Result<Self> {
        if config.pca9685_addresses.is_empty() {
            bail!("motion.pca9685_addresses must not be empty when hardware is enabled");
        }
        let boards = initialize_pca_boards(&config.pca9685_addresses, cancelled, |address| {
            Pca9685Device::open(
                &config.i2c_device,
                address,
                config.pca_frequency_hz,
                config.min_pulse_us,
                config.max_pulse_us,
            )
            .with_context(|| {
                format!(
                    "opening PCA9685 servo controller at 0x{address:02x} on {}",
                    config.i2c_device.display()
                )
            })
        })?;
        Ok(Self {
            servo_count: config.servo_count,
            boards,
        })
    }
}

fn initialize_pca_boards<Io: Read + Write>(
    addresses: &[u16],
    cancelled: &dyn Fn() -> bool,
    mut open: impl FnMut(u16) -> Result<Pca9685Device<Io>>,
) -> Result<Vec<Mutex<Pca9685Device<Io>>>> {
    let mut boards = Vec::new();
    let initialization = (|| {
        for &address in addresses {
            ensure!(!cancelled(), "motion initialization cancelled");
            let mut device = open(address)?;
            let original = device
                .initialize(cancelled)
                .with_context(|| format!("initializing PCA9685 address 0x{address:02x}"))?;
            boards.push((device, original));
        }
        ensure!(!cancelled(), "motion initialization cancelled");
        Ok::<_, anyhow::Error>(())
    })();
    if let Err(error) = initialization {
        let mut failures = Vec::new();
        for (device, original) in boards.iter_mut().rev() {
            if let Err(recovery) = device.restore(*original) {
                failures.push(format!("address 0x{:02x}: {recovery:#}", device.address));
            }
        }
        if failures.is_empty() {
            return Err(
                error.context("PCA9685 construction aborted; prior-board restoration submitted")
            );
        }
        bail!(
            "PCA9685 construction failed; output state unknown; cause: {error:#}; prior-board recovery: {}",
            failures.join("; ")
        );
    }
    Ok(boards
        .into_iter()
        .map(|(device, _)| Mutex::new(device))
        .collect())
}

impl ServoBackend for Pca9685ServoBackend {
    fn submit_frame(&self, angles: &[f32], cancelled: &dyn Fn() -> bool) -> ServoWrite {
        if let Err(error) = validate_angles(angles, self.servo_count) {
            let mut result = ServoWrite::empty(ServoBoundary::I2cRegisterWrite);
            result.error = Some(error.to_string());
            return result;
        }
        submit_i2c_channels(angles, cancelled, |channel, angle| {
            let board = self
                .boards
                .get(channel / 16)
                .with_context(|| format!("missing PCA9685 board for channel {channel}"))?;
            let mut board = board.lock().unwrap_or_else(|error| error.into_inner());
            board.set_servo_angle(channel % 16, angle)
        })
    }
}

fn submit_i2c_channels(
    angles: &[f32],
    cancelled: &dyn Fn() -> bool,
    mut write_channel: impl FnMut(usize, f32) -> Result<()>,
) -> ServoWrite {
    let mut result = ServoWrite::empty(ServoBoundary::I2cRegisterWrite);
    for (channel, &angle) in angles.iter().enumerate() {
        if cancelled() {
            result.cancelled = true;
            break;
        }
        match write_channel(channel, angle) {
            Ok(()) => {
                result.submitted_channels.push(channel as u32);
                result.accepted_bytes += 5;
            }
            Err(error) => {
                result.uncertain_channels.push(channel as u32);
                result.error = Some(error.to_string());
                break;
            }
        }
    }
    result
}

struct Pca9685Device<Io = std::fs::File> {
    file: Io,
    address: u16,
    frequency_hz: u16,
    min_pulse_us: u16,
    max_pulse_us: u16,
}

const PCA_MODE1: u8 = 0x00;
const PCA_PRESCALE: u8 = 0xfe;
const PCA_RESTART: u8 = 0x80;
const PCA_EXTCLK: u8 = 0x40;
const PCA_AI: u8 = 0x20;
const PCA_SLEEP: u8 = 0x10;

#[derive(Clone, Copy, Debug)]
struct Pca9685Snapshot {
    mode: u8,
    prescale: u8,
}

impl Pca9685Device {
    fn open(
        path: &Path,
        address: u16,
        frequency_hz: u16,
        min_pulse_us: u16,
        max_pulse_us: u16,
    ) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("opening I2C device {}", path.display()))?;
        set_i2c_slave(&file, address).with_context(|| {
            format!(
                "selecting PCA9685 I2C address 0x{address:02x} on {}",
                PathBuf::from(path).display()
            )
        })?;
        Ok(Self {
            file,
            address,
            frequency_hz,
            min_pulse_us,
            max_pulse_us,
        })
    }
}

impl<Io: Read + Write> Pca9685Device<Io> {
    fn initialize(&mut self, cancelled: &dyn Fn() -> bool) -> Result<Pca9685Snapshot> {
        ensure!(!cancelled(), "motion initialization cancelled");
        let mode = self.read_reg(PCA_MODE1)?;
        ensure!(
            mode & PCA_EXTCLK == 0,
            "PCA9685 external clock is unsupported; no configuration registers changed"
        );
        ensure!(!cancelled(), "motion initialization cancelled");
        let original = Pca9685Snapshot {
            mode,
            prescale: self.read_reg(PCA_PRESCALE)?,
        };
        ensure!(!cancelled(), "motion initialization cancelled");
        let active_mode = (original.mode & !(PCA_RESTART | PCA_SLEEP)) | PCA_AI;
        // Even a failed first write may enter SLEEP; this owner must finish recovery.
        let initialization = (|| {
            self.write_reg(PCA_MODE1, active_mode | PCA_SLEEP)?;
            ensure!(!cancelled(), "motion initialization cancelled");
            self.write_reg(PCA_PRESCALE, self.prescale())?;
            ensure!(!cancelled(), "motion initialization cancelled");
            self.wake(active_mode)?;
            ensure!(!cancelled(), "motion initialization cancelled");
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = initialization {
            match self.restore(original) {
                Ok(()) => {
                    return Err(error.context(
                        "PCA9685 initialization failed; original register restoration submitted",
                    ));
                }
                Err(recovery) => bail!(
                    "PCA9685 initialization failed; output state unknown; cause: {error:#}; recovery: {recovery:#}"
                ),
            }
        }
        info!(
            address = format_args!("0x{:02x}", self.address),
            frequency_hz = self.frequency_hz,
            "PCA9685 initialized"
        );
        Ok(original)
    }

    fn wake(&mut self, mode: u8) -> Result<()> {
        let active_mode = mode & !(PCA_RESTART | PCA_SLEEP);
        self.write_reg(PCA_MODE1, active_mode)
            .context("clearing PCA9685 SLEEP")?;
        // NXP requires at least 500 us after clearing SLEEP before RESTART/PWM.
        std::thread::sleep(Duration::from_micros(500));
        let current_mode = self.read_reg(PCA_MODE1)?;
        ensure!(
            current_mode & !PCA_RESTART == active_mode,
            "PCA9685 awake mode readback mismatch"
        );
        if current_mode & PCA_RESTART != 0 {
            self.write_reg(PCA_MODE1, active_mode | PCA_RESTART)
                .context("restarting PCA9685 channels")?;
        }
        Ok(())
    }

    fn restore(&mut self, original: Pca9685Snapshot) -> Result<()> {
        let mode = original.mode & !PCA_RESTART;
        let mut failures = Vec::new();
        match self.write_reg(PCA_MODE1, mode | PCA_SLEEP) {
            Ok(()) => {
                if let Err(error) = self.write_reg(PCA_PRESCALE, original.prescale) {
                    failures.push(format!("restoring PRE_SCALE: {error:#}"));
                }
            }
            Err(error) => failures.push(format!("establishing recovery SLEEP: {error:#}")),
        }
        // A failed prescale restoration must not abandon our own successful SLEEP.
        if mode & PCA_SLEEP == 0 {
            if let Err(error) = self.wake(mode) {
                failures.push(format!("restoring running mode: {error:#}"));
            }
        }
        ensure!(
            failures.is_empty(),
            "PCA9685 recovery failed; output state unknown: {}",
            failures.join("; ")
        );
        Ok(())
    }

    fn set_servo_angle(&mut self, channel: usize, angle: f32) -> Result<()> {
        if channel >= 16 {
            bail!("PCA9685 channel {channel} out of range 0..16");
        }
        let angle = angle.clamp(0.0, 180.0);
        let pulse = self.min_pulse_us as f32
            + (self.max_pulse_us - self.min_pulse_us) as f32 * (angle / 180.0);
        let period = 1_000_000.0 / self.frequency_hz.max(1) as f32;
        let off = ((pulse / period) * 4096.0).round().clamp(0.0, 4095.0) as u16;
        self.set_pwm(channel, 0, off)
    }

    fn set_pwm(&mut self, channel: usize, on: u16, off: u16) -> Result<()> {
        let base = 0x06 + 4 * channel as u8;
        self.write_many(&[
            base,
            (on & 0xff) as u8,
            (on >> 8) as u8,
            (off & 0xff) as u8,
            (off >> 8) as u8,
        ])
    }

    fn prescale(&self) -> u8 {
        let prescale = 25_000_000.0 / (4096.0 * self.frequency_hz.max(1) as f32) - 1.0;
        prescale.round().clamp(3.0, 255.0) as u8
    }

    fn read_reg(&mut self, reg: u8) -> Result<u8> {
        self.file.write_all(&[reg]).with_context(|| {
            format!(
                "writing register pointer 0x{reg:02x} to PCA9685 address 0x{:02x}",
                self.address
            )
        })?;
        let mut out = [0_u8; 1];
        self.file.read_exact(&mut out).with_context(|| {
            format!(
                "reading register 0x{reg:02x} from PCA9685 address 0x{:02x}",
                self.address
            )
        })?;
        Ok(out[0])
    }

    fn write_reg(&mut self, reg: u8, value: u8) -> Result<()> {
        self.write_many(&[reg, value])
    }

    fn write_many(&mut self, bytes: &[u8]) -> Result<()> {
        let register = bytes.first().copied().unwrap_or_default();
        write_i2c_transaction(&mut self.file, bytes).with_context(|| {
            format!(
                "writing {} bytes starting at register 0x{register:02x} to PCA9685 address 0x{:02x}",
                bytes.len(),
                self.address
            )
        })?;
        Ok(())
    }
}

fn serial_frame_command(angles: &[f32]) -> String {
    let mut command = format!("F {}", angles.len());
    for angle in angles {
        command.push(' ');
        command.push_str(&servo_angle_deci(*angle).to_string());
    }
    command.push('\n');
    command
}

fn servo_angle_deci(angle: f32) -> u16 {
    (angle.clamp(0.0, 180.0) * 10.0).round() as u16
}

#[cfg(unix)]
fn open_serial_port(path: &Path, baud_rate: u32) -> Result<std::fs::File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening serial device {}", path.display()))?;
    configure_serial_port(file.as_raw_fd(), baud_rate).context("configuring serial device")?;
    Ok(file)
}

#[cfg(not(unix))]
fn open_serial_port(path: &Path, baud_rate: u32) -> Result<std::fs::File> {
    let _ = (path, baud_rate);
    bail!("Arduino servo UART currently requires a Unix serial device")
}

#[cfg(unix)]
fn configure_serial_port(fd: std::os::fd::RawFd, baud_rate: u32) -> Result<()> {
    let baud = match baud_rate {
        9_600 => libc::B9600,
        19_200 => libc::B19200,
        38_400 => libc::B38400,
        57_600 => libc::B57600,
        115_200 => libc::B115200,
        other => bail!(
            "unsupported Arduino servo UART baud rate {other}; expected one of 9600, 19200, 38400, 57600, 115200"
        ),
    };

    let mut termios = unsafe {
        let mut termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut termios) != 0 {
            return Err(std::io::Error::last_os_error()).context("tcgetattr");
        }
        termios
    };

    unsafe { libc::cfmakeraw(&mut termios) };
    termios.c_cflag |= libc::CLOCAL | libc::CREAD;
    termios.c_cflag &= !libc::CSTOPB;
    termios.c_cflag &= !libc::PARENB;
    termios.c_cflag &= !libc::CSIZE;
    termios.c_cflag |= libc::CS8;
    termios.c_cc[libc::VMIN] = 0;
    termios.c_cc[libc::VTIME] = 0;

    unsafe {
        if libc::cfsetispeed(&mut termios, baud) != 0 {
            return Err(std::io::Error::last_os_error()).context("cfsetispeed");
        }
        if libc::cfsetospeed(&mut termios, baud) != 0 {
            return Err(std::io::Error::last_os_error()).context("cfsetospeed");
        }
        if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
            return Err(std::io::Error::last_os_error()).context("tcsetattr");
        }
        libc::tcflush(fd, libc::TCIOFLUSH);
    }
    Ok(())
}

fn set_i2c_slave(file: &std::fs::File, address: u16) -> Result<()> {
    const I2C_SLAVE: libc::c_ulong = 0x0703;
    let result = unsafe { libc::ioctl(file.as_raw_fd(), I2C_SLAVE, address as libc::c_ulong) };
    if result < 0 {
        bail!(
            "ioctl(I2C_SLAVE) failed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

#[cfg(test)]
mod pca_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct ShortWriter {
        answers: VecDeque<io::Result<usize>>,
        bytes: Vec<u8>,
        calls: usize,
        flush_failure: bool,
    }

    impl ShortWriter {
        fn new(answers: Vec<io::Result<usize>>) -> Self {
            Self {
                answers: answers.into(),
                bytes: vec![],
                calls: 0,
                flush_failure: false,
            }
        }
    }
    impl Write for ShortWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            let count = self.answers.pop_front().unwrap_or(Ok(bytes.len()))?;
            let count = count.min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.flush_failure {
                Err(io::Error::other("injected flush failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn serial_short_writes_keep_exact_byte_prefix_without_replaying_frames() {
        let frame = b"F 2 900 901\n";
        let mut writer =
            ShortWriter::new(vec![Ok(2), Err(io::ErrorKind::Interrupted.into()), Ok(3)]);
        let result = submit_serial(&mut writer, frame, 2, &|| false);
        assert_eq!(writer.bytes, frame);
        assert_eq!(result.accepted_bytes, frame.len());
        assert_eq!(result.submitted_channels, vec![0, 1]);
        assert!(result.uncertain_channels.is_empty());
        assert!(result.fully_submitted(2));
    }

    #[test]
    fn serial_partial_failure_has_uncertain_channels_and_does_not_retry() {
        let mut writer =
            ShortWriter::new(vec![Ok(3), Err(io::ErrorKind::BrokenPipe.into()), Ok(100)]);
        let result = submit_serial(&mut writer, b"F 2 900 901\n", 2, &|| false);
        assert_eq!(writer.calls, 2);
        assert_eq!(writer.bytes, b"F 2");
        assert_eq!(result.accepted_bytes, 3);
        assert!(result.submitted_channels.is_empty());
        assert_eq!(result.uncertain_channels, vec![0, 1]);
        assert!(result.error.is_some());
    }

    #[test]
    fn serial_cancellation_is_checked_before_every_short_write_segment() {
        let cancelled = std::cell::Cell::new(false);
        struct CancelAfterWrite<'a> {
            bytes: Vec<u8>,
            cancelled: &'a std::cell::Cell<bool>,
        }
        impl Write for CancelAfterWrite<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.bytes.extend_from_slice(&bytes[..3]);
                self.cancelled.set(true);
                Ok(3)
            }
            fn flush(&mut self) -> io::Result<()> {
                panic!("partial cancelled command must not flush")
            }
        }
        let mut writer = CancelAfterWrite {
            bytes: vec![],
            cancelled: &cancelled,
        };
        let result = submit_serial(&mut writer, b"F 2 900 901\n", 2, &|| cancelled.get());
        assert!(result.cancelled);
        assert_eq!(result.accepted_bytes, 3);
        assert_eq!(result.uncertain_channels, vec![0, 1]);
        assert_eq!(writer.bytes, b"F 2");
    }

    #[test]
    fn serial_full_host_write_remains_a_fact_after_flush_failure() {
        let frame = b"F 2 900 901\n";
        let mut writer = ShortWriter::new(vec![]);
        writer.flush_failure = true;
        let result = submit_serial(&mut writer, frame, 2, &|| false);
        assert_eq!(result.submitted_channels, vec![0, 1]);
        assert_eq!(result.accepted_bytes, frame.len());
        assert!(result.uncertain_channels.is_empty());
        assert!(result.error.is_some());
    }

    #[test]
    fn i2c_short_transaction_is_never_retried_as_a_suffix_register_write() {
        let mut writer = ShortWriter::new(vec![Ok(2), Ok(3)]);
        assert!(write_i2c_transaction(&mut writer, &[0x06, 0, 0, 0x12, 1]).is_err());
        assert_eq!(writer.calls, 1);
        assert_eq!(writer.bytes, [0x06, 0]);
    }

    #[test]
    fn i2c_channel_failure_distinguishes_prior_channels_from_the_uncertain_channel() {
        let mut writes = Vec::new();
        let result = submit_i2c_channels(&[90.0, 91.0, 92.0], &|| false, |channel, _| {
            writes.push(channel);
            ensure!(channel != 1, "partial second channel");
            Ok(())
        });
        assert_eq!(writes, vec![0, 1]);
        assert_eq!(result.submitted_channels, vec![0]);
        assert_eq!(result.uncertain_channels, vec![1]);
        assert!(result.error.is_some());
    }

    #[test]
    fn i2c_authorization_is_checked_before_each_channel() {
        let cancelled = AtomicBool::new(false);
        let mut calls = 0;
        let result = submit_i2c_channels(
            &[90.0, 91.0, 92.0],
            &|| cancelled.load(Ordering::Acquire),
            |_, _| {
                calls += 1;
                cancelled.store(true, Ordering::Release);
                Ok(())
            },
        );
        assert_eq!(calls, 1);
        assert!(result.cancelled);
        assert_eq!(result.submitted_channels, vec![0]);
        assert!(result.uncertain_channels.is_empty());
    }

    #[test]
    fn deferred_backend_does_not_open_hardware_until_owned_submission() {
        let config = MotionConfig {
            hardware_enabled: true,
            servo_backend: "serial".into(),
            serial_path: "/definitely-absent-motion-test-device".into(),
            serial_startup_delay_ms: 0,
            ..MotionConfig::default()
        };
        let backend = create_servo_backend(&config).unwrap();
        let cancelled = backend.submit_frame(&[90.0; 8], &|| true);
        assert!(cancelled.cancelled);
        assert!(cancelled.error.is_none());
        let open = backend.submit_frame(&[90.0; 8], &|| false);
        assert!(open.error.unwrap().contains("initialization"));
        assert!(open.submitted_channels.is_empty());
        backend.close();
    }

    #[test]
    fn invalid_target_is_rejected_before_any_native_submission() {
        let backend = MockServoBackend::new(3);
        let result = backend.submit_frame(&[90.0, f32::NAN, 90.0], &|| false);
        assert!(result.error.is_some());
        assert!(result.submitted_channels.is_empty());
        assert_eq!(*backend.last_angles.lock().unwrap(), vec![0.0; 3]);
    }

    #[test]
    fn formats_servo_frame_serial_command_as_deci_degrees() {
        assert_eq!(
            serial_frame_command(&[90.0, 91.24, 180.4, -5.0]),
            "F 4 900 912 1800 0\n"
        );
    }
}
