use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tracing::{debug, info};

use crate::config::MotionConfig;

#[tonic::async_trait]
pub trait ServoBackend: Send + Sync {
    async fn set_angle(&self, channel: usize, angle: f32) -> Result<()>;

    async fn set_angles(&self, angles: &[f32]) -> Result<()> {
        for (channel, angle) in angles.iter().enumerate() {
            self.set_angle(channel, *angle).await?;
        }
        Ok(())
    }
}

pub fn create_servo_backend(config: &MotionConfig) -> Result<Box<dyn ServoBackend>> {
    if !config.hardware_enabled {
        return Ok(Box::new(MockServoBackend::new(config.servo_count)));
    }

    match config.servo_backend.as_str() {
        "serial" | "arduino" | "arduino_serial" => Ok(Box::new(SerialServoBackend::new(config)?)),
        "pca9685" | "i2c" => Ok(Box::new(Pca9685ServoBackend::new(config)?)),
        "mock" => Ok(Box::new(MockServoBackend::new(config.servo_count))),
        other => bail!("unknown motion.servo_backend '{other}'; expected serial, pca9685, or mock"),
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

#[tonic::async_trait]
impl ServoBackend for MockServoBackend {
    async fn set_angle(&self, channel: usize, angle: f32) -> Result<()> {
        if channel >= self.servo_count {
            bail!("servo channel {channel} out of range {}", self.servo_count);
        }
        let mut last = self
            .last_angles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        last[channel] = angle;
        info!(channel, angle, "mock servo angle");
        Ok(())
    }

    async fn set_angles(&self, angles: &[f32]) -> Result<()> {
        if angles.len() > self.servo_count {
            bail!(
                "servo angle frame has {} channels, configured servo count is {}",
                angles.len(),
                self.servo_count
            );
        }
        let mut last = self
            .last_angles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (index, angle) in angles.iter().enumerate() {
            last[index] = *angle;
        }
        info!(angles = ?angles, "mock servo frame");
        Ok(())
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

    fn write_command(&self, command: String) -> Result<()> {
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        file.write_all(command.as_bytes())
            .context("writing Arduino servo command")?;
        file.flush().context("flushing Arduino servo command")?;
        debug!(command = %command.trim_end(), "Arduino servo command");
        Ok(())
    }
}

#[tonic::async_trait]
impl ServoBackend for SerialServoBackend {
    async fn set_angle(&self, channel: usize, angle: f32) -> Result<()> {
        if channel >= self.servo_count {
            bail!("servo channel {channel} out of range {}", self.servo_count);
        }
        self.write_command(serial_angle_command(channel, angle))
    }

    async fn set_angles(&self, angles: &[f32]) -> Result<()> {
        if angles.len() > self.servo_count {
            bail!(
                "servo angle frame has {} channels, configured servo count is {}",
                angles.len(),
                self.servo_count
            );
        }
        self.write_command(serial_frame_command(angles))
    }
}

pub struct Pca9685ServoBackend {
    servo_count: usize,
    boards: Vec<Mutex<Pca9685Device>>,
}

impl Pca9685ServoBackend {
    pub fn new(config: &MotionConfig) -> Result<Self> {
        if config.pca9685_addresses.is_empty() {
            bail!("motion.pca9685_addresses must not be empty when hardware is enabled");
        }
        let mut boards = Vec::new();
        for address in &config.pca9685_addresses {
            let mut device = Pca9685Device::open(
                &config.i2c_device,
                *address,
                config.pca_frequency_hz,
                config.min_pulse_us,
                config.max_pulse_us,
            )
            .with_context(|| {
                format!(
                    "opening PCA9685 servo controller at 0x{address:02x} on {}",
                    config.i2c_device.display()
                )
            })?;
            device.initialize().with_context(|| {
                format!(
                    "initializing PCA9685 servo controller at 0x{address:02x} on {}",
                    config.i2c_device.display()
                )
            })?;
            boards.push(Mutex::new(device));
        }
        Ok(Self {
            servo_count: config.servo_count,
            boards,
        })
    }
}

#[tonic::async_trait]
impl ServoBackend for Pca9685ServoBackend {
    async fn set_angle(&self, channel: usize, angle: f32) -> Result<()> {
        if channel >= self.servo_count {
            bail!("servo channel {channel} out of range {}", self.servo_count);
        }
        let board_index = channel / 16;
        let local_channel = channel % 16;
        let Some(board) = self.boards.get(board_index) else {
            bail!("missing PCA9685 board for channel {channel}");
        };
        let mut board = board
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        board.set_servo_angle(local_channel, angle)
    }

    async fn set_angles(&self, angles: &[f32]) -> Result<()> {
        if angles.len() > self.servo_count {
            bail!(
                "servo angle frame has {} channels, configured servo count is {}",
                angles.len(),
                self.servo_count
            );
        }
        for (channel, angle) in angles.iter().enumerate() {
            self.set_angle(channel, *angle).await?;
        }
        Ok(())
    }
}

struct Pca9685Device {
    file: std::fs::File,
    address: u16,
    frequency_hz: u16,
    min_pulse_us: u16,
    max_pulse_us: u16,
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

    fn initialize(&mut self) -> Result<()> {
        const MODE1: u8 = 0x00;
        const PRESCALE: u8 = 0xfe;
        const RESTART: u8 = 0x80;
        const SLEEP: u8 = 0x10;
        const ALLCALL: u8 = 0x01;

        self.write_reg(MODE1, ALLCALL)?;
        std::thread::sleep(std::time::Duration::from_millis(5));

        let old_mode = self.read_reg(MODE1)?;
        let sleep_mode = (old_mode & !RESTART) | SLEEP;
        self.write_reg(MODE1, sleep_mode)?;
        self.write_reg(PRESCALE, self.prescale())?;
        self.write_reg(MODE1, old_mode)?;
        std::thread::sleep(std::time::Duration::from_millis(5));
        self.write_reg(MODE1, old_mode | RESTART | ALLCALL)?;
        info!(
            address = format_args!("0x{:02x}", self.address),
            frequency_hz = self.frequency_hz,
            "PCA9685 initialized"
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
        self.file.write_all(bytes).with_context(|| {
            format!(
                "writing {} bytes starting at register 0x{register:02x} to PCA9685 address 0x{:02x}",
                bytes.len(),
                self.address
            )
        })?;
        Ok(())
    }
}

fn serial_angle_command(channel: usize, angle: f32) -> String {
    format!("A {channel} {}\n", servo_angle_deci(angle))
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
mod tests {
    use super::*;

    #[test]
    fn formats_single_servo_serial_command_as_deci_degrees() {
        assert_eq!(serial_angle_command(3, 91.24), "A 3 912\n");
        assert_eq!(serial_angle_command(3, -10.0), "A 3 0\n");
        assert_eq!(serial_angle_command(3, 220.0), "A 3 1800\n");
    }

    #[test]
    fn formats_servo_frame_serial_command_as_deci_degrees() {
        assert_eq!(
            serial_frame_command(&[90.0, 91.24, 180.4, -5.0]),
            "F 4 900 912 1800 0\n"
        );
    }
}
