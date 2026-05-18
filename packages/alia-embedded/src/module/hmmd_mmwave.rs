use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::HmmdMmWaveConfig;

use super::context::ModuleContext;
use super::lifecycle::{ModuleHealth, ModuleLifecycle};
use super::sensor::{SensorDefinition, SensorProvider, SensorReading};

const REPORT_HEADER: [u8; 4] = [0xF4, 0xF3, 0xF2, 0xF1];
const REPORT_FOOTER: [u8; 4] = [0xF8, 0xF7, 0xF6, 0xF5];
const REPORT_MODE_COMMAND: [u8; 18] = [
    0xFD, 0xFC, 0xFB, 0xFA, 0x08, 0x00, 0x12, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x03,
    0x02, 0x01,
];
const MAX_FRAME_PAYLOAD_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq)]
pub struct HmmdMmWaveReading {
    pub presence: bool,
    pub target_distance_cm: Option<u16>,
    pub raw_payload_hex: String,
}

#[derive(Debug, Clone)]
struct LatestReading {
    reading: HmmdMmWaveReading,
    received_at: Instant,
}

#[derive(Debug, Default)]
struct HmmdState {
    latest: Option<LatestReading>,
    connected: bool,
    last_error: Option<String>,
}

pub struct HmmdMmWaveModule {
    config: HmmdMmWaveConfig,
    state: Arc<RwLock<HmmdState>>,
    cancel_token: CancellationToken,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl HmmdMmWaveModule {
    pub fn new(config: HmmdMmWaveConfig) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(HmmdState::default())),
            cancel_token: CancellationToken::new(),
            worker: Mutex::new(None),
        }
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for HmmdMmWaveModule {
    fn name(&self) -> &'static str {
        "hmmd-mmwave"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> Result<()> {
        self.cancel_token = ctx.cancel_token.child_token();
        let prefix = normalized_prefix(&self.config.sensor_prefix);
        let stale_after = Duration::from_millis(self.config.stale_after_ms.max(250));

        for kind in [
            HmmdSensorKind::Presence,
            HmmdSensorKind::TargetDistanceCm,
            HmmdSensorKind::Raw,
        ] {
            ctx.sensors
                .register(Arc::new(HmmdSensorProvider {
                    prefix: prefix.clone(),
                    kind,
                    state: self.state.clone(),
                    stale_after,
                }))
                .await?;
        }

        Ok(())
    }

    async fn start(&self) -> Result<()> {
        if self
            .worker
            .lock()
            .expect("hmmd worker mutex poisoned")
            .is_some()
        {
            return Ok(());
        }

        let config = self.config.clone();
        let state = self.state.clone();
        let cancel = self.cancel_token.clone();
        let handle = thread::Builder::new()
            .name("hmmd-mmwave-uart".to_string())
            .spawn(move || run_hmmd_serial_worker(config, state, cancel))
            .context("spawning HMMD mmWave UART worker")?;

        *self.worker.lock().expect("hmmd worker mutex poisoned") = Some(handle);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.cancel_token.cancel();
        if let Some(handle) = self
            .worker
            .lock()
            .expect("hmmd worker mutex poisoned")
            .take()
            && let Err(error) = handle.join()
        {
            warn!(error = ?error, "HMMD mmWave worker panicked during shutdown");
        }
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let state = self.state.read().expect("hmmd state poisoned");
        if state.connected {
            ModuleHealth::Healthy
        } else if state.last_error.is_some() {
            ModuleHealth::Degraded
        } else {
            ModuleHealth::Degraded
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum HmmdSensorKind {
    Presence,
    TargetDistanceCm,
    Raw,
}

struct HmmdSensorProvider {
    prefix: String,
    kind: HmmdSensorKind,
    state: Arc<RwLock<HmmdState>>,
    stale_after: Duration,
}

#[tonic::async_trait]
impl SensorProvider for HmmdSensorProvider {
    fn definition(&self) -> SensorDefinition {
        let mut definition = match self.kind {
            HmmdSensorKind::Presence => {
                SensorDefinition::new(sensor_id(&self.prefix, "presence"), "HMMD mmWave presence")
            }
            HmmdSensorKind::TargetDistanceCm => SensorDefinition::new(
                sensor_id(&self.prefix, "target_distance_cm"),
                "HMMD mmWave target distance",
            ),
            HmmdSensorKind::Raw => {
                SensorDefinition::new(sensor_id(&self.prefix, "raw"), "HMMD mmWave raw state")
            }
        };
        definition.source = "hmmd_mmwave".to_string();
        definition.capabilities = vec!["read".to_string()];
        definition.metadata = serde_json::json!({
            "sensor_family": "hmmd_mmwave",
            "transport": "uart",
        });

        match self.kind {
            HmmdSensorKind::Presence => {
                definition.unit = "boolean".to_string();
                definition.value_type = "bool".to_string();
            }
            HmmdSensorKind::TargetDistanceCm => {
                definition.unit = "cm".to_string();
                definition.value_type = "int".to_string();
            }
            HmmdSensorKind::Raw => {
                definition.value_type = "json".to_string();
            }
        }

        definition
    }

    async fn read(&self) -> Result<SensorReading> {
        let state = self.state.read().expect("hmmd state poisoned");
        let latest = state.latest.as_ref().with_context(|| {
            state
                .last_error
                .clone()
                .unwrap_or_else(|| "HMMD mmWave has not produced a UART reading yet".to_string())
        })?;
        let age = latest.received_at.elapsed();
        if age > self.stale_after {
            anyhow::bail!("HMMD mmWave reading is stale ({} ms old)", age.as_millis());
        }

        let value = match self.kind {
            HmmdSensorKind::Presence => serde_json::json!(latest.reading.presence),
            HmmdSensorKind::TargetDistanceCm => latest
                .reading
                .target_distance_cm
                .map(serde_json::Value::from)
                .unwrap_or(serde_json::Value::Null),
            HmmdSensorKind::Raw => serde_json::json!({
                "presence": latest.reading.presence,
                "target_distance_cm": latest.reading.target_distance_cm,
                "raw_payload_hex": latest.reading.raw_payload_hex,
                "age_ms": age.as_millis() as u64,
                "connected": state.connected,
            }),
        };

        let mut reading = SensorReading::now(sensor_id(&self.prefix, self.kind.slug()), value);
        let freshness = 1.0 - (age.as_secs_f32() / self.stale_after.as_secs_f32());
        reading.quality = freshness.clamp(0.1, 1.0);
        Ok(reading)
    }
}

impl HmmdSensorKind {
    fn slug(self) -> &'static str {
        match self {
            HmmdSensorKind::Presence => "presence",
            HmmdSensorKind::TargetDistanceCm => "target_distance_cm",
            HmmdSensorKind::Raw => "raw",
        }
    }
}

fn run_hmmd_serial_worker(
    config: HmmdMmWaveConfig,
    state: Arc<RwLock<HmmdState>>,
    cancel: CancellationToken,
) {
    let reconnect_delay = Duration::from_millis(config.reconnect_delay_ms.max(250));
    while !cancel.is_cancelled() {
        match run_hmmd_serial_session(&config, &state, &cancel) {
            Ok(()) => {
                if cancel.is_cancelled() {
                    break;
                }
            }
            Err(error) => {
                set_error(&state, error.to_string());
                warn!(
                    path = %config.serial_path.display(),
                    error = %error,
                    "HMMD mmWave UART session failed; retrying"
                );
            }
        }
        sleep_cancelable(&cancel, reconnect_delay);
    }
}

fn run_hmmd_serial_session(
    config: &HmmdMmWaveConfig,
    state: &Arc<RwLock<HmmdState>>,
    cancel: &CancellationToken,
) -> Result<()> {
    #[cfg(unix)]
    {
        run_hmmd_serial_session_unix(config, state, cancel)
    }

    #[cfg(not(unix))]
    {
        let _ = (config, state, cancel);
        anyhow::bail!("HMMD mmWave UART currently requires a Unix serial device")
    }
}

#[cfg(unix)]
fn run_hmmd_serial_session_unix(
    config: &HmmdMmWaveConfig,
    state: &Arc<RwLock<HmmdState>>,
    cancel: &CancellationToken,
) -> Result<()> {
    use std::os::fd::AsRawFd;

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&config.serial_path)
        .with_context(|| format!("opening HMMD UART: {}", config.serial_path.display()))?;
    configure_uart(file.as_raw_fd(), config.baud_rate, config.read_timeout_ms)
        .context("configuring HMMD UART")?;

    if config.configure_report_mode {
        file.write_all(&REPORT_MODE_COMMAND)
            .context("writing HMMD report-mode command")?;
        file.flush().context("flushing HMMD report-mode command")?;
    }

    {
        let mut state = state.write().expect("hmmd state poisoned");
        state.connected = true;
        state.last_error = None;
    }
    info!(
        path = %config.serial_path.display(),
        baud_rate = config.baud_rate,
        report_mode = config.configure_report_mode,
        "HMMD mmWave UART connected"
    );

    let mut serial_buf = [0u8; 128];
    let mut parser_buf = Vec::with_capacity(1024);
    while !cancel.is_cancelled() {
        match file.read(&mut serial_buf) {
            Ok(0) => continue,
            Ok(n) => {
                parser_buf.extend_from_slice(&serial_buf[..n]);
                for reading in parse_hmmd_serial_buffer(&mut parser_buf) {
                    debug!(
                        presence = reading.presence,
                        target_distance_cm = reading.target_distance_cm,
                        "HMMD mmWave reading"
                    );
                    set_latest(state, reading);
                }
                if parser_buf.len() > 4096 {
                    let keep_from = parser_buf
                        .windows(REPORT_HEADER.len())
                        .rposition(|window| window == REPORT_HEADER)
                        .unwrap_or(parser_buf.len());
                    parser_buf.drain(..keep_from);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error).context("reading HMMD UART"),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn configure_uart(fd: std::os::fd::RawFd, baud_rate: u32, read_timeout_ms: u64) -> Result<()> {
    let baud = match baud_rate {
        115_200 => libc::B115200,
        other => anyhow::bail!("unsupported HMMD UART baud rate {other}; expected 115200"),
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
    termios.c_cc[libc::VTIME] = ((read_timeout_ms.max(100) / 100).min(255)) as libc::cc_t;

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

pub fn parse_hmmd_serial_buffer(buffer: &mut Vec<u8>) -> Vec<HmmdMmWaveReading> {
    let mut out = Vec::new();

    loop {
        if let Some(pos) = find_bytes(buffer, &REPORT_HEADER) {
            if pos > 0 {
                drain_ascii_lines(&mut buffer[..pos], &mut out);
                buffer.drain(..pos);
            }
        } else {
            drain_ascii_lines(buffer, &mut out);
            if buffer.len() > 256 {
                buffer.clear();
            }
            break;
        }

        if buffer.len() < 10 {
            break;
        }

        let payload_len = u16::from_le_bytes([buffer[4], buffer[5]]) as usize;
        if payload_len > MAX_FRAME_PAYLOAD_BYTES {
            buffer.drain(..1);
            continue;
        }

        let frame_len = REPORT_HEADER.len() + 2 + payload_len + REPORT_FOOTER.len();
        if buffer.len() < frame_len {
            break;
        }

        let footer_start = REPORT_HEADER.len() + 2 + payload_len;
        if buffer[footer_start..frame_len] != REPORT_FOOTER {
            buffer.drain(..1);
            continue;
        }

        if let Some(reading) = parse_hmmd_report_payload(&buffer[6..footer_start]) {
            out.push(reading);
        }
        buffer.drain(..frame_len);
    }

    out
}

pub fn parse_hmmd_report_payload(payload: &[u8]) -> Option<HmmdMmWaveReading> {
    let (&detection_result, rest) = payload.split_first()?;
    let target_distance_cm = if rest.len() >= 2 {
        Some(u16::from_le_bytes([rest[0], rest[1]]))
    } else {
        None
    };
    Some(HmmdMmWaveReading {
        presence: detection_result != 0,
        target_distance_cm,
        raw_payload_hex: bytes_to_hex(payload),
    })
}

fn drain_ascii_lines(buffer: &mut [u8], out: &mut Vec<HmmdMmWaveReading>) {
    let Ok(text) = std::str::from_utf8(buffer) else {
        return;
    };
    for line in text.lines() {
        if let Some(reading) = parse_hmmd_ascii_line(line) {
            out.push(reading);
        }
    }
}

fn parse_hmmd_ascii_line(line: &str) -> Option<HmmdMmWaveReading> {
    let upper = line.to_ascii_uppercase();
    let presence = if upper.contains("OFF") || upper.contains("ABSENT") || upper.contains("NONE") {
        false
    } else if upper.contains("ON") || upper.contains("PRESENT") || upper.contains("HUMAN") {
        true
    } else {
        return None;
    };
    Some(HmmdMmWaveReading {
        presence,
        target_distance_cm: first_u16_in_text(line),
        raw_payload_hex: bytes_to_hex(line.as_bytes()),
    })
}

fn first_u16_in_text(text: &str) -> Option<u16> {
    let mut digits = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            break;
        }
    }
    digits.parse().ok()
}

fn find_bytes(buffer: &[u8], needle: &[u8]) -> Option<usize> {
    buffer
        .windows(needle.len())
        .position(|window| window == needle)
}

fn set_latest(state: &Arc<RwLock<HmmdState>>, reading: HmmdMmWaveReading) {
    let mut state = state.write().expect("hmmd state poisoned");
    state.connected = true;
    state.last_error = None;
    state.latest = Some(LatestReading {
        reading,
        received_at: Instant::now(),
    });
}

fn set_error(state: &Arc<RwLock<HmmdState>>, error: String) {
    let mut state = state.write().expect("hmmd state poisoned");
    state.connected = false;
    state.last_error = Some(error);
}

fn sleep_cancelable(cancel: &CancellationToken, duration: Duration) {
    let start = Instant::now();
    while !cancel.is_cancelled() && start.elapsed() < duration {
        thread::sleep(Duration::from_millis(50));
    }
}

fn normalized_prefix(prefix: &str) -> String {
    let prefix = prefix.trim().trim_matches([':', '/', '.']);
    if prefix.is_empty() {
        "hmmd".to_string()
    } else {
        prefix.to_string()
    }
}

fn sensor_id(prefix: &str, slug: &str) -> String {
    format!("{}:{}", normalized_prefix(prefix), slug)
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join("")
}

#[allow(dead_code)]
fn unix_timestamp_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros() as u64)
        .unwrap_or(0)
}
