use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use tracing::info;

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
    if config.hardware_enabled {
        return Ok(Box::new(Pca9685ServoBackend::new(config)?));
    }
    Ok(Box::new(MockServoBackend::new(config.servo_count)))
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
            )?;
            device.initialize()?;
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
        self.file.write_all(&[reg])?;
        let mut out = [0_u8; 1];
        self.file.read_exact(&mut out)?;
        Ok(out[0])
    }

    fn write_reg(&mut self, reg: u8, value: u8) -> Result<()> {
        self.write_many(&[reg, value])
    }

    fn write_many(&mut self, bytes: &[u8]) -> Result<()> {
        self.file.write_all(bytes)?;
        Ok(())
    }
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
