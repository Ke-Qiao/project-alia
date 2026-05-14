use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, SampleFormat, SampleRate, Stream, StreamConfig, SupportedBufferSize,
    SupportedStreamConfig, SupportedStreamConfigRange,
};
use tracing::{debug, error, info, warn};

use crate::config::AudioConfig;

use super::context::{AudioBus, AudioFrame};
use super::lifecycle::{ModuleHealth, ModuleLifecycle};
use super::traits::{AudioCaptureBackend, AudioPlaybackBackend};

pub struct CpalAudioCapture {
    config: AudioConfig,
    control_tx: Mutex<Option<mpsc::Sender<()>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    active: AtomicBool,
}

impl CpalAudioCapture {
    pub fn new(config: AudioConfig) -> Self {
        Self {
            config,
            control_tx: Mutex::new(None),
            thread: Mutex::new(None),
            active: AtomicBool::new(false),
        }
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for CpalAudioCapture {
    fn name(&self) -> &'static str {
        "cpal_audio_capture"
    }

    async fn stop(&self) -> Result<()> {
        self.stop_capture()
    }

    async fn health_check(&self) -> ModuleHealth {
        if self.is_available() {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Unhealthy
        }
    }
}

impl AudioCaptureBackend for CpalAudioCapture {
    fn start_capture(&self, bus: &AudioBus) -> Result<()> {
        let mut thread_slot = lock_mutex(&self.thread, "capture thread")?;
        if thread_slot.is_some() {
            return Ok(());
        }

        let (control_tx, control_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let config = self.config.clone();
        let bus = bus.clone();

        let handle = thread::Builder::new()
            .name("alia-cpal-capture".to_string())
            .spawn(move || {
                let result = run_capture_thread(config, bus, control_rx, ready_tx);
                if let Err(error) = result {
                    error!(%error, "cpal capture thread stopped with error");
                }
            })
            .context("spawning cpal capture thread")?;

        match ready_rx
            .recv()
            .context("waiting for cpal capture startup")?
        {
            Ok(()) => {
                *lock_mutex(&self.control_tx, "capture control")? = Some(control_tx);
                *thread_slot = Some(handle);
                self.active.store(true, Ordering::Release);
                Ok(())
            }
            Err(error) => {
                let _ = handle.join();
                bail!(error);
            }
        }
    }

    fn stop_capture(&self) -> Result<()> {
        if let Some(tx) = lock_mutex(&self.control_tx, "capture control")?.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = lock_mutex(&self.thread, "capture thread")?.take() {
            let _ = handle.join();
        }
        self.active.store(false, Ordering::Release);
        Ok(())
    }

    fn is_available(&self) -> bool {
        let host = cpal::default_host();
        select_input_device(&host, &self.config.capture_device).is_ok()
    }
}

pub struct CpalAudioPlayback {
    config: AudioConfig,
    command_tx: Mutex<Option<mpsc::Sender<PlaybackCommand>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    active: AtomicBool,
}

impl CpalAudioPlayback {
    pub fn new(config: AudioConfig) -> Self {
        Self {
            config,
            command_tx: Mutex::new(None),
            thread: Mutex::new(None),
            active: AtomicBool::new(false),
        }
    }

    fn start_stream(&self) -> Result<()> {
        let mut thread_slot = lock_mutex(&self.thread, "playback thread")?;
        if thread_slot.is_some() {
            return Ok(());
        }

        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let config = self.config.clone();

        let handle = thread::Builder::new()
            .name("alia-cpal-playback".to_string())
            .spawn(move || {
                let result = run_playback_thread(config, command_rx, ready_tx);
                if let Err(error) = result {
                    error!(%error, "cpal playback thread stopped with error");
                }
            })
            .context("spawning cpal playback thread")?;

        match ready_rx
            .recv()
            .context("waiting for cpal playback startup")?
        {
            Ok(()) => {
                *lock_mutex(&self.command_tx, "playback command")? = Some(command_tx);
                *thread_slot = Some(handle);
                self.active.store(true, Ordering::Release);
                Ok(())
            }
            Err(error) => {
                let _ = handle.join();
                bail!(error);
            }
        }
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for CpalAudioPlayback {
    fn name(&self) -> &'static str {
        "cpal_audio_playback"
    }

    async fn start(&self) -> Result<()> {
        self.start_stream()
    }

    async fn stop(&self) -> Result<()> {
        if let Some(tx) = lock_mutex(&self.command_tx, "playback command")?.take() {
            let _ = tx.send(PlaybackCommand::Shutdown);
        }
        if let Some(handle) = lock_mutex(&self.thread, "playback thread")?.take() {
            let _ = handle.join();
        }
        self.active.store(false, Ordering::Release);
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if self.is_available() {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Unhealthy
        }
    }
}

impl AudioPlaybackBackend for CpalAudioPlayback {
    fn play(&self, pcm: &[i16], sample_rate: u32) -> Result<()> {
        if pcm.is_empty() {
            return Ok(());
        }
        self.start_stream()?;
        let tx = lock_mutex(&self.command_tx, "playback command")?
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow!("cpal playback thread is not started"))?;
        tx.send(PlaybackCommand::Play {
            pcm: pcm.to_vec(),
            sample_rate,
        })
        .context("sending playback command")
    }

    fn stop_playback(&self) -> Result<()> {
        if let Some(tx) = lock_mutex(&self.command_tx, "playback command")?.as_ref() {
            tx.send(PlaybackCommand::Stop)
                .context("sending stop playback command")?;
        }
        Ok(())
    }

    fn is_available(&self) -> bool {
        let host = cpal::default_host();
        select_output_device(&host, &self.config.playback_device).is_ok()
    }
}

enum PlaybackCommand {
    Play { pcm: Vec<i16>, sample_rate: u32 },
    Stop,
    Shutdown,
}

fn run_capture_thread(
    config: AudioConfig,
    bus: AudioBus,
    control_rx: mpsc::Receiver<()>,
    ready_tx: mpsc::SyncSender<std::result::Result<(), String>>,
) -> Result<()> {
    let started = (|| {
        let host = cpal::default_host();
        let device = select_input_device(&host, &config.capture_device)?;
        let selected = select_stream_config(
            device.supported_input_configs()?,
            config.capture_channels,
            config.capture_sample_rate,
        )
        .or_else(|error| {
            warn!(%error, "falling back to default input config");
            device
                .default_input_config()
                .context("default input config")
        })?;
        let stream_config = selected.config();
        let input_channels = stream_config.channels as usize;
        let input_sample_rate = stream_config.sample_rate.0;
        let target_channels = config.capture_channels.max(1) as usize;
        let target_sample_rate = config.capture_sample_rate;
        let converter = Arc::new(Mutex::new(CaptureConverter::new(
            input_channels,
            input_sample_rate,
            target_channels,
            target_sample_rate,
        )));
        let emitted_frames = Arc::new(AtomicU64::new(0));
        let err_fn = |error| warn!(%error, "cpal input stream error");

        let stream = match selected.sample_format() {
            SampleFormat::I8 => build_input_stream::<i8>(
                &device,
                &stream_config,
                converter,
                bus,
                emitted_frames,
                err_fn,
            )?,
            SampleFormat::I16 => build_input_stream::<i16>(
                &device,
                &stream_config,
                converter,
                bus,
                emitted_frames,
                err_fn,
            )?,
            SampleFormat::I32 => build_input_stream::<i32>(
                &device,
                &stream_config,
                converter,
                bus,
                emitted_frames,
                err_fn,
            )?,
            SampleFormat::I64 => build_input_stream::<i64>(
                &device,
                &stream_config,
                converter,
                bus,
                emitted_frames,
                err_fn,
            )?,
            SampleFormat::U8 => build_input_stream::<u8>(
                &device,
                &stream_config,
                converter,
                bus,
                emitted_frames,
                err_fn,
            )?,
            SampleFormat::U16 => build_input_stream::<u16>(
                &device,
                &stream_config,
                converter,
                bus,
                emitted_frames,
                err_fn,
            )?,
            SampleFormat::U32 => build_input_stream::<u32>(
                &device,
                &stream_config,
                converter,
                bus,
                emitted_frames,
                err_fn,
            )?,
            SampleFormat::U64 => build_input_stream::<u64>(
                &device,
                &stream_config,
                converter,
                bus,
                emitted_frames,
                err_fn,
            )?,
            SampleFormat::F32 => build_input_stream::<f32>(
                &device,
                &stream_config,
                converter,
                bus,
                emitted_frames,
                err_fn,
            )?,
            SampleFormat::F64 => build_input_stream::<f64>(
                &device,
                &stream_config,
                converter,
                bus,
                emitted_frames,
                err_fn,
            )?,
            _ => bail!(
                "unsupported input sample format: {:?}",
                selected.sample_format()
            ),
        };

        stream.play().context("starting cpal input stream")?;
        info!(
            device = %device_name(&device),
            input_sample_rate,
            input_channels,
            target_sample_rate,
            target_channels,
            "started cpal audio capture"
        );
        Ok::<_, anyhow::Error>(stream)
    })();

    let stream = match started {
        Ok(stream) => {
            let _ = ready_tx.send(Ok(()));
            stream
        }
        Err(error) => {
            let message = error.to_string();
            let _ = ready_tx.send(Err(message));
            return Err(error);
        }
    };

    let _ = control_rx.recv();
    drop(stream);
    info!("stopped cpal audio capture");
    Ok(())
}

fn run_playback_thread(
    config: AudioConfig,
    command_rx: mpsc::Receiver<PlaybackCommand>,
    ready_tx: mpsc::SyncSender<std::result::Result<(), String>>,
) -> Result<()> {
    let started = (|| {
        let host = cpal::default_host();
        let device = select_output_device(&host, &config.playback_device)?;
        let selected = select_stream_config(
            device.supported_output_configs()?,
            config.playback_channels,
            config.playback_sample_rate,
        )
        .or_else(|error| {
            warn!(%error, "falling back to default output config");
            device
                .default_output_config()
                .context("default output config")
        })?;
        let stream_config = with_buffer_size(
            selected.config(),
            selected.buffer_size(),
            config.playback_buffer_ms,
        );
        let sample_rate = stream_config.sample_rate.0;
        let channels = stream_config.channels.max(1) as usize;
        let buffer = Arc::new(Mutex::new(VecDeque::new()));
        let err_fn = |error| warn!(%error, "cpal output stream error");

        let stream = match selected.sample_format() {
            SampleFormat::I8 => {
                build_output_stream::<i8>(&device, &stream_config, Arc::clone(&buffer), err_fn)?
            }
            SampleFormat::I16 => {
                build_output_stream::<i16>(&device, &stream_config, Arc::clone(&buffer), err_fn)?
            }
            SampleFormat::I32 => {
                build_output_stream::<i32>(&device, &stream_config, Arc::clone(&buffer), err_fn)?
            }
            SampleFormat::I64 => {
                build_output_stream::<i64>(&device, &stream_config, Arc::clone(&buffer), err_fn)?
            }
            SampleFormat::U8 => {
                build_output_stream::<u8>(&device, &stream_config, Arc::clone(&buffer), err_fn)?
            }
            SampleFormat::U16 => {
                build_output_stream::<u16>(&device, &stream_config, Arc::clone(&buffer), err_fn)?
            }
            SampleFormat::U32 => {
                build_output_stream::<u32>(&device, &stream_config, Arc::clone(&buffer), err_fn)?
            }
            SampleFormat::U64 => {
                build_output_stream::<u64>(&device, &stream_config, Arc::clone(&buffer), err_fn)?
            }
            SampleFormat::F32 => {
                build_output_stream::<f32>(&device, &stream_config, Arc::clone(&buffer), err_fn)?
            }
            SampleFormat::F64 => {
                build_output_stream::<f64>(&device, &stream_config, Arc::clone(&buffer), err_fn)?
            }
            _ => bail!(
                "unsupported output sample format: {:?}",
                selected.sample_format()
            ),
        };

        stream.play().context("starting cpal output stream")?;
        info!(
            device = %device_name(&device),
            sample_rate,
            channels,
            "started cpal audio playback"
        );
        Ok::<_, anyhow::Error>((stream, buffer, sample_rate, channels))
    })();

    let (stream, buffer, output_sample_rate, output_channels) = match started {
        Ok(started) => {
            let _ = ready_tx.send(Ok(()));
            started
        }
        Err(error) => {
            let message = error.to_string();
            let _ = ready_tx.send(Err(message));
            return Err(error);
        }
    };

    while let Ok(command) = command_rx.recv() {
        match command {
            PlaybackCommand::Play { pcm, sample_rate } => {
                let rendered =
                    render_playback_pcm(&pcm, sample_rate, output_sample_rate, output_channels);
                let mut buffer = lock_mutex(&buffer, "playback buffer")?;
                buffer.extend(rendered);
                debug!(queued_samples = buffer.len(), "queued playback audio");
            }
            PlaybackCommand::Stop => {
                lock_mutex(&buffer, "playback buffer")?.clear();
            }
            PlaybackCommand::Shutdown => break,
        }
    }

    drop(stream);
    info!("stopped cpal audio playback");
    Ok(())
}

trait ToI16Sample {
    fn to_i16_sample(self) -> i16;
}

impl ToI16Sample for i8 {
    fn to_i16_sample(self) -> i16 {
        (self as i16) << 8
    }
}
impl ToI16Sample for i16 {
    fn to_i16_sample(self) -> i16 {
        self
    }
}
impl ToI16Sample for i32 {
    fn to_i16_sample(self) -> i16 {
        (self >> 16).clamp(i16::MIN as i32, i16::MAX as i32) as i16
    }
}
impl ToI16Sample for i64 {
    fn to_i16_sample(self) -> i16 {
        (self >> 48).clamp(i16::MIN as i64, i16::MAX as i64) as i16
    }
}
impl ToI16Sample for u8 {
    fn to_i16_sample(self) -> i16 {
        ((self as i16) - 128) << 8
    }
}
impl ToI16Sample for u16 {
    fn to_i16_sample(self) -> i16 {
        (self as i32 - 32_768) as i16
    }
}
impl ToI16Sample for u32 {
    fn to_i16_sample(self) -> i16 {
        ((self as i64 - 2_147_483_648) >> 16) as i16
    }
}
impl ToI16Sample for u64 {
    fn to_i16_sample(self) -> i16 {
        ((self as i128 - 9_223_372_036_854_775_808) >> 48) as i16
    }
}
impl ToI16Sample for f32 {
    fn to_i16_sample(self) -> i16 {
        (self.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    }
}
impl ToI16Sample for f64 {
    fn to_i16_sample(self) -> i16 {
        (self.clamp(-1.0, 1.0) * i16::MAX as f64) as i16
    }
}

trait FromI16Sample {
    fn from_i16_sample(sample: i16) -> Self;
}

impl FromI16Sample for i8 {
    fn from_i16_sample(sample: i16) -> Self {
        (sample >> 8) as i8
    }
}
impl FromI16Sample for i16 {
    fn from_i16_sample(sample: i16) -> Self {
        sample
    }
}
impl FromI16Sample for i32 {
    fn from_i16_sample(sample: i16) -> Self {
        (sample as i32) << 16
    }
}
impl FromI16Sample for i64 {
    fn from_i16_sample(sample: i16) -> Self {
        (sample as i64) << 48
    }
}
impl FromI16Sample for u8 {
    fn from_i16_sample(sample: i16) -> Self {
        ((sample as i32 + 32_768) >> 8).clamp(u8::MIN as i32, u8::MAX as i32) as u8
    }
}
impl FromI16Sample for u16 {
    fn from_i16_sample(sample: i16) -> Self {
        (sample as i32 + 32_768).clamp(u16::MIN as i32, u16::MAX as i32) as u16
    }
}
impl FromI16Sample for u32 {
    fn from_i16_sample(sample: i16) -> Self {
        ((sample as i64 + 32_768) << 16).clamp(u32::MIN as i64, u32::MAX as i64) as u32
    }
}
impl FromI16Sample for u64 {
    fn from_i16_sample(sample: i16) -> Self {
        ((sample as i128 + 32_768) << 48).clamp(u64::MIN as i128, u64::MAX as i128) as u64
    }
}
impl FromI16Sample for f32 {
    fn from_i16_sample(sample: i16) -> Self {
        sample as f32 / i16::MAX as f32
    }
}
impl FromI16Sample for f64 {
    fn from_i16_sample(sample: i16) -> Self {
        sample as f64 / i16::MAX as f64
    }
}

struct CaptureConverter {
    input_channels: usize,
    input_sample_rate: u32,
    target_channels: usize,
    target_sample_rate: u32,
    last_sample: Option<i16>,
    phase: f64,
}

impl CaptureConverter {
    fn new(
        input_channels: usize,
        input_sample_rate: u32,
        target_channels: usize,
        target_sample_rate: u32,
    ) -> Self {
        Self {
            input_channels: input_channels.max(1),
            input_sample_rate,
            target_channels: target_channels.max(1),
            target_sample_rate,
            last_sample: None,
            phase: 0.0,
        }
    }

    fn convert<T: Copy + ToI16Sample>(&mut self, input: &[T]) -> Vec<i16> {
        if input.is_empty() {
            return Vec::new();
        }

        let mono = downmix_to_mono(input, self.input_channels);
        if mono.is_empty() {
            return Vec::new();
        }

        let samples = if self.input_sample_rate == self.target_sample_rate {
            mono
        } else {
            self.resample_mono(&mono)
        };
        interleave_mono(&samples, self.target_channels)
    }

    fn resample_mono(&mut self, mono: &[i16]) -> Vec<i16> {
        let mut source = Vec::with_capacity(mono.len() + usize::from(self.last_sample.is_some()));
        if let Some(sample) = self.last_sample {
            source.push(sample);
        }
        source.extend_from_slice(mono);
        if source.len() < 2 {
            self.last_sample = source.last().copied();
            return Vec::new();
        }

        let step = self.input_sample_rate as f64 / self.target_sample_rate as f64;
        let mut output = Vec::with_capacity(
            ((source.len() as f64) * self.target_sample_rate as f64 / self.input_sample_rate as f64)
                .ceil() as usize,
        );

        while self.phase + 1.0 < source.len() as f64 {
            let index = self.phase.floor() as usize;
            let frac = self.phase - index as f64;
            let left = source[index] as f64;
            let right = source[index + 1] as f64;
            output.push((left + (right - left) * frac).round() as i16);
            self.phase += step;
        }

        self.phase -= (source.len() - 1) as f64;
        self.last_sample = source.last().copied();
        output
    }
}

fn build_input_stream<T>(
    device: &Device,
    config: &StreamConfig,
    converter: Arc<Mutex<CaptureConverter>>,
    bus: AudioBus,
    emitted_frames: Arc<AtomicU64>,
    err_fn: impl Fn(cpal::StreamError) + Send + 'static,
) -> Result<Stream>
where
    T: Copy + ToI16Sample + cpal::SizedSample + Send + 'static,
{
    let sample_rate = {
        let converter = lock_mutex(&converter, "capture converter")?;
        converter.target_sample_rate
    };
    let channels = {
        let converter = lock_mutex(&converter, "capture converter")?;
        converter.target_channels
    };
    let level_peak = Arc::new(AtomicU64::new(0));
    let silence_warned = Arc::new(AtomicBool::new(false));
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let pcm = match converter.lock() {
                    Ok(mut converter) => converter.convert(data),
                    Err(error) => {
                        warn!(%error, "capture converter lock poisoned");
                        return;
                    }
                };
                if pcm.is_empty() {
                    return;
                }
                let frames = (pcm.len() / channels.max(1)) as u64;
                let start_frame = emitted_frames.fetch_add(frames, Ordering::Relaxed);
                let peak = pcm
                    .iter()
                    .map(|sample| sample.unsigned_abs() as u64)
                    .max()
                    .unwrap_or(0);
                level_peak.fetch_max(peak, Ordering::Relaxed);
                let end_frame = start_frame.saturating_add(frames);
                if end_frame >= sample_rate as u64 * 5
                    && !silence_warned.swap(true, Ordering::Relaxed)
                {
                    let peak = level_peak.load(Ordering::Relaxed);
                    if peak < 256 {
                        warn!(
                            peak,
                            "microphone input appears silent; check macOS microphone permission and selected input device"
                        );
                    } else {
                        debug!(peak, "microphone input level observed");
                    }
                }
                bus.publish(AudioFrame {
                    pcm,
                    sample_rate,
                    timestamp_us: start_frame.saturating_mul(1_000_000) / sample_rate as u64,
                });
            },
            err_fn,
            None,
        )
        .context("building cpal input stream")
}

fn build_output_stream<T>(
    device: &Device,
    config: &StreamConfig,
    buffer: Arc<Mutex<VecDeque<i16>>>,
    err_fn: impl Fn(cpal::StreamError) + Send + 'static,
) -> Result<Stream>
where
    T: FromI16Sample + cpal::SizedSample + Send + 'static,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| match buffer.lock() {
                Ok(mut buffer) => {
                    for output in data {
                        let sample = buffer.pop_front().unwrap_or(0);
                        *output = T::from_i16_sample(sample);
                    }
                }
                Err(error) => {
                    warn!(%error, "playback buffer lock poisoned");
                    for output in data {
                        *output = T::from_i16_sample(0);
                    }
                }
            },
            err_fn,
            None,
        )
        .context("building cpal output stream")
}

fn select_input_device(host: &cpal::Host, configured: &str) -> Result<Device> {
    if configured == "default" || configured.trim().is_empty() {
        return host
            .default_input_device()
            .ok_or_else(|| anyhow!("no default input device available"));
    }

    find_device(host.input_devices()?, configured)
        .with_context(|| format!("finding input device '{configured}'"))
}

fn select_output_device(host: &cpal::Host, configured: &str) -> Result<Device> {
    if configured == "default" || configured.trim().is_empty() {
        return host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default output device available"));
    }

    find_device(host.output_devices()?, configured)
        .with_context(|| format!("finding output device '{configured}'"))
}

fn find_device(devices: impl Iterator<Item = Device>, configured: &str) -> Result<Device> {
    for device in devices {
        if device_name(&device).contains(configured) {
            return Ok(device);
        }
    }
    bail!("device not found")
}

fn select_stream_config(
    ranges: impl Iterator<Item = SupportedStreamConfigRange>,
    preferred_channels: u16,
    preferred_sample_rate: u32,
) -> Result<SupportedStreamConfig> {
    let mut channel_match = None;
    let mut fallback = None;

    for range in ranges {
        if fallback.is_none() {
            fallback = Some(range.with_max_sample_rate());
        }

        let has_preferred_rate = range.min_sample_rate().0 <= preferred_sample_rate
            && preferred_sample_rate <= range.max_sample_rate().0;

        if range.channels() == preferred_channels && has_preferred_rate {
            return Ok(range.with_sample_rate(SampleRate(preferred_sample_rate)));
        }

        if range.channels() == preferred_channels && channel_match.is_none() {
            channel_match = Some(range.with_max_sample_rate());
        }
    }

    channel_match
        .or(fallback)
        .ok_or_else(|| anyhow!("device reports no supported stream configs"))
}

fn with_buffer_size(
    mut config: StreamConfig,
    supported: &SupportedBufferSize,
    buffer_ms: u32,
) -> StreamConfig {
    if buffer_ms > 0 {
        let frames = config
            .sample_rate
            .0
            .saturating_mul(buffer_ms)
            .saturating_div(1_000)
            .max(1);
        config.buffer_size = match supported {
            SupportedBufferSize::Range { min, max } => BufferSize::Fixed(frames.clamp(*min, *max)),
            SupportedBufferSize::Unknown => BufferSize::Default,
        };
    }
    config
}

fn downmix_to_mono<T: Copy + ToI16Sample>(input: &[T], input_channels: usize) -> Vec<i16> {
    input
        .chunks(input_channels.max(1))
        .map(|frame| {
            let sum: i32 = frame
                .iter()
                .map(|sample| sample.to_i16_sample() as i32)
                .sum();
            (sum / frame.len() as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16
        })
        .collect()
}

fn interleave_mono(samples: &[i16], channels: usize) -> Vec<i16> {
    if channels <= 1 {
        return samples.to_vec();
    }

    let mut output = Vec::with_capacity(samples.len() * channels);
    for sample in samples {
        for _ in 0..channels {
            output.push(*sample);
        }
    }
    output
}

fn render_playback_pcm(
    pcm: &[i16],
    source_sample_rate: u32,
    target_sample_rate: u32,
    target_channels: usize,
) -> Vec<i16> {
    let mono = if source_sample_rate == target_sample_rate {
        pcm.to_vec()
    } else {
        resample_mono_block(pcm, source_sample_rate, target_sample_rate)
    };
    interleave_mono(&mono, target_channels)
}

fn resample_mono_block(
    input: &[i16],
    source_sample_rate: u32,
    target_sample_rate: u32,
) -> Vec<i16> {
    if input.len() < 2 || source_sample_rate == target_sample_rate {
        return input.to_vec();
    }

    let output_len = ((input.len() as u64 * target_sample_rate as u64) / source_sample_rate as u64)
        .max(1) as usize;
    let step = source_sample_rate as f64 / target_sample_rate as f64;
    let mut output = Vec::with_capacity(output_len);

    for index in 0..output_len {
        let source_pos = index as f64 * step;
        let left_index = source_pos.floor() as usize;
        let right_index = (left_index + 1).min(input.len() - 1);
        let frac = source_pos - left_index as f64;
        let left = input[left_index] as f64;
        let right = input[right_index] as f64;
        output.push((left + (right - left) * frac).round() as i16);
    }

    output
}

fn lock_mutex<'a, T>(mutex: &'a Mutex<T>, name: &str) -> Result<std::sync::MutexGuard<'a, T>> {
    mutex.lock().map_err(|_| anyhow!("{name} lock poisoned"))
}

fn device_name(device: &Device) -> String {
    device.name().unwrap_or_else(|_| "<unknown>".to_string())
}
