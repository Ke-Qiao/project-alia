use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

#[cfg(target_os = "linux")]
use alsa::pcm::{
    Access as AlsaAccess, Format as AlsaFormat, Frames as AlsaFrames, HwParams as AlsaHwParams,
    PCM as AlsaPcm, State as AlsaState,
};
#[cfg(target_os = "linux")]
use alsa::{Direction as AlsaDirection, ValueOr as AlsaValueOr};
use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, SampleFormat, SampleRate, Stream, StreamConfig, SupportedBufferSize,
    SupportedStreamConfig, SupportedStreamConfigRange,
};
use tracing::{debug, error, info, warn};

#[cfg(target_os = "linux")]
use crate::config::AlsaCaptureS32Alignment;
use crate::config::{AudioBackend, AudioConfig};

use super::context::{AudioBus, AudioFrame};
use super::lifecycle::{ModuleHealth, ModuleLifecycle};
use super::traits::{AudioCaptureBackend, AudioPlaybackBackend};
use super::volume::{AudioVolumeState, apply_gain_in_place};

#[cfg(target_os = "linux")]
mod alsa_backend;
mod cpal_backend;

pub struct CpalAudioCapture {
    config: AudioConfig,
    volume: AudioVolumeState,
    control_tx: Mutex<Option<mpsc::Sender<()>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    active: AtomicBool,
}

impl CpalAudioCapture {
    pub fn new(config: AudioConfig, volume: AudioVolumeState) -> Self {
        Self {
            config,
            volume,
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
        let volume = self.volume.clone();
        let bus = bus.clone();

        let handle = thread::Builder::new()
            .name("alia-cpal-capture".to_string())
            .spawn(move || {
                let result = run_capture_thread(config, bus, control_rx, ready_tx, volume);
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
        if matches!(
            resolved_capture_backend(&self.config),
            ResolvedAudioBackend::Alsa
        ) {
            #[cfg(target_os = "linux")]
            {
                return resolved_alsa_capture_device(&self.config)
                    .is_some_and(|device| alsa_capture_device_available(&device));
            }

            #[cfg(not(target_os = "linux"))]
            {
                return false;
            }
        }

        let host = cpal::default_host();
        if select_input_device(&host, &self.config.capture_device).is_ok() {
            return true;
        }

        #[cfg(target_os = "linux")]
        {
            return resolved_alsa_capture_device(&self.config)
                .is_some_and(|device| alsa_capture_device_available(&device));
        }

        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
}

pub struct CpalAudioPlayback {
    config: AudioConfig,
    volume: AudioVolumeState,
    command_tx: Mutex<Option<mpsc::SyncSender<PlaybackCommand>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    active: AtomicBool,
}

impl CpalAudioPlayback {
    pub fn new(config: AudioConfig, volume: AudioVolumeState) -> Self {
        Self {
            config,
            volume,
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

        let (command_tx, command_rx) =
            mpsc::sync_channel(playback_command_queue_capacity(&self.config));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let config = self.config.clone();
        let volume = self.volume.clone();

        let handle = thread::Builder::new()
            .name("alia-cpal-playback".to_string())
            .spawn(move || {
                let result = run_playback_thread(config, command_rx, ready_tx, volume);
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
        if matches!(
            resolved_playback_backend(&self.config),
            ResolvedAudioBackend::Alsa
        ) {
            #[cfg(target_os = "linux")]
            {
                return resolved_alsa_playback_device(&self.config)
                    .is_some_and(|device| alsa_playback_device_available(&device));
            }

            #[cfg(not(target_os = "linux"))]
            {
                return false;
            }
        }

        let host = cpal::default_host();
        if select_output_device(&host, &self.config.playback_device).is_ok() {
            return true;
        }

        #[cfg(target_os = "linux")]
        {
            return resolved_alsa_playback_device(&self.config)
                .is_some_and(|device| alsa_playback_device_available(&device));
        }

        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
}

enum PlaybackCommand {
    Play { pcm: Vec<i16>, sample_rate: u32 },
    Stop,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedAudioBackend {
    Cpal,
    Alsa,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlsaCaptureSampleFormat {
    S16,
    S32,
}

fn playback_target_buffer_ms(config: &AudioConfig) -> u64 {
    u64::from(
        config
            .playback_target_buffer_ms
            .max(1)
            .min(config.playback_max_buffer_ms.max(1)),
    )
}

fn playback_max_buffer_ms(config: &AudioConfig) -> u64 {
    u64::from(config.playback_max_buffer_ms.max(1))
}

fn playback_command_queue_capacity(config: &AudioConfig) -> usize {
    config.playback_command_queue_capacity.max(1)
}

#[cfg(target_os = "linux")]
fn alsa_capture_s32_shift_override(config: &AudioConfig) -> Option<u8> {
    match config.alsa_capture_s32_alignment {
        AlsaCaptureS32Alignment::Auto => None,
        AlsaCaptureS32Alignment::LeftJustified => Some(16),
        AlsaCaptureS32Alignment::RightJustified => Some(8),
    }
}

fn run_capture_thread(
    config: AudioConfig,
    bus: AudioBus,
    control_rx: mpsc::Receiver<()>,
    ready_tx: mpsc::SyncSender<std::result::Result<(), String>>,
    volume: AudioVolumeState,
) -> Result<()> {
    if matches!(
        resolved_capture_backend(&config),
        ResolvedAudioBackend::Alsa
    ) {
        #[cfg(target_os = "linux")]
        if let Some(alsa_device) = resolved_alsa_capture_device(&config) {
            return run_alsa_capture_thread(
                config,
                bus,
                control_rx,
                ready_tx,
                &alsa_device,
                volume,
            );
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = ready_tx.send(Err("ALSA capture backend is only supported on Linux".into()));
            bail!("ALSA capture backend is only supported on Linux");
        }
    }

    let started = cpal_backend::start_cpal_capture_runtime(&config, bus.clone(), volume.clone());

    let stream = match started {
        Ok(stream) => {
            let _ = ready_tx.send(Ok(()));
            stream
        }
        Err(error) => {
            #[cfg(target_os = "linux")]
            if let Some(alsa_device) = resolved_alsa_capture_device(&config) {
                warn!(
                    cpal_error = %error,
                    alsa_device = %alsa_device,
                    "cpal capture unavailable; trying ALSA capture fallback"
                );
                return alsa_backend::run_alsa_capture_thread(
                    config,
                    bus,
                    control_rx,
                    ready_tx,
                    &alsa_device,
                    volume,
                );
            }

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

#[cfg(target_os = "linux")]
fn run_alsa_capture_thread(
    config: AudioConfig,
    bus: AudioBus,
    control_rx: mpsc::Receiver<()>,
    ready_tx: mpsc::SyncSender<std::result::Result<(), String>>,
    alsa_device: &str,
    volume: AudioVolumeState,
) -> Result<()> {
    let started = start_alsa_capture_runtime(&config, alsa_device);
    let (pcm, input_sample_rate, input_channels, chunk_frames, sample_format) = match started {
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

    let target_channels = config.capture_channels.max(1) as usize;
    let target_sample_rate = config.capture_sample_rate.max(1);
    let mut converter = CaptureConverter::new(
        input_channels,
        input_sample_rate,
        target_channels,
        target_sample_rate,
    );
    let mut emitted_frames = 0_u64;
    let mut level_peak = 0_u64;
    let mut silence_warned = false;
    let mut clipping_warned = false;
    let mut detected_s32_shift = None;
    let sample_count = chunk_frames.saturating_mul(input_channels.max(1));
    let mut raw_buffer_i16 = vec![0_i16; sample_count];
    let mut raw_buffer_i32 = vec![0_i32; sample_count];

    loop {
        if control_rx.try_recv().is_ok() {
            break;
        }

        let ready = pcm
            .wait(Some(100))
            .context("waiting for ALSA capture data")?;
        if !ready {
            continue;
        }

        let (frames_read, captured_pcm) = match sample_format {
            AlsaCaptureSampleFormat::S16 => {
                let io = pcm.io_i16().context("opening ALSA i16 capture IO")?;
                match io.readi(&mut raw_buffer_i16) {
                    Ok(frames) => {
                        let samples_read = frames.saturating_mul(input_channels);
                        (frames, converter.convert(&raw_buffer_i16[..samples_read]))
                    }
                    Err(error) if error.errno() == libc::EAGAIN => continue,
                    Err(error) => {
                        pcm.try_recover(error, true)
                            .context("recovering ALSA capture stream")?;
                        continue;
                    }
                }
            }
            AlsaCaptureSampleFormat::S32 => {
                let io = pcm.io_i32().context("opening ALSA i32 capture IO")?;
                match io.readi(&mut raw_buffer_i32) {
                    Ok(frames) => {
                        let samples_read = frames.saturating_mul(input_channels);
                        let raw = &raw_buffer_i32[..samples_read];
                        let shift = match detected_s32_shift {
                            Some(shift) => shift,
                            None => {
                                let shift = detect_alsa_s32_capture_shift(raw);
                                if let Some(shift) = shift {
                                    detected_s32_shift = Some(shift);
                                }
                                shift.unwrap_or(16)
                            }
                        };
                        if detected_s32_shift == Some(shift) && emitted_frames == 0 {
                            info!(
                                alsa_device,
                                s32_shift = shift,
                                "detected ALSA S32 capture bit alignment"
                            );
                        }
                        let converted: Vec<i16> = raw
                            .iter()
                            .copied()
                            .map(|sample| i32_sample_to_i16_with_shift(sample, shift))
                            .collect();
                        (frames, converter.convert(&converted))
                    }
                    Err(error) if error.errno() == libc::EAGAIN => continue,
                    Err(error) => {
                        pcm.try_recover(error, true)
                            .context("recovering ALSA capture stream")?;
                        continue;
                    }
                }
            }
        };

        let samples_read = frames_read.saturating_mul(input_channels);
        if samples_read == 0 {
            continue;
        }

        let mut pcm = captured_pcm;
        if pcm.is_empty() {
            continue;
        }
        apply_gain_in_place(&mut pcm, volume.capture_volume());

        let frames = (pcm.len() / target_channels.max(1)) as u64;
        let start_frame = emitted_frames;
        emitted_frames = emitted_frames.saturating_add(frames);
        let peak = pcm
            .iter()
            .map(|sample| sample.unsigned_abs() as u64)
            .max()
            .unwrap_or(0);
        level_peak = level_peak.max(peak);
        if peak >= 32_760 && !clipping_warned {
            clipping_warned = true;
            warn!(
                peak,
                "microphone input is clipping; lower capture_volume or adjust alsa_capture_s32_alignment"
            );
        }
        if emitted_frames >= target_sample_rate as u64 * 5 && !silence_warned {
            silence_warned = true;
            if level_peak < 256 {
                warn!(
                    peak = level_peak,
                    "microphone input appears silent; check ALSA capture source and selected input device"
                );
            } else {
                debug!(peak = level_peak, "microphone input level observed");
            }
        }

        bus.publish(AudioFrame {
            pcm,
            sample_rate: target_sample_rate,
            timestamp_us: start_frame.saturating_mul(1_000_000) / target_sample_rate as u64,
        });
    }

    let _ = pcm.drop();
    info!("stopped ALSA audio capture");
    Ok(())
}

#[cfg(target_os = "linux")]
fn start_alsa_capture_runtime(
    config: &AudioConfig,
    alsa_device: &str,
) -> Result<(AlsaPcm, u32, usize, usize, AlsaCaptureSampleFormat)> {
    let pcm = AlsaPcm::new(alsa_device, AlsaDirection::Capture, true)
        .with_context(|| format!("opening ALSA capture PCM '{alsa_device}'"))?;
    let requested_channels = u32::from(config.capture_channels.max(1));
    let requested_rate = config.capture_sample_rate.max(1);
    let (actual_channels, actual_sample_rate, sample_format) = {
        let hwp = AlsaHwParams::any(&pcm).context("reading ALSA capture hardware params")?;
        hwp.set_access(AlsaAccess::RWInterleaved)
            .context("setting ALSA capture interleaved access")?;
        let sample_format = preferred_alsa_capture_format(&hwp);
        hwp.set_format(alsa_capture_format(sample_format))
            .context("setting ALSA capture sample format")?;
        let _ = hwp.set_rate_resample(true);

        let actual_channels = hwp
            .set_channels_near(requested_channels)
            .context("setting ALSA capture channels")?;
        let actual_sample_rate = hwp
            .set_rate_near(requested_rate, AlsaValueOr::Nearest)
            .context("setting ALSA capture sample rate")?;

        let requested_period_frames =
            buffer_frames_for_ms(actual_sample_rate, config.opus_frame_ms.max(10));
        let _ =
            hwp.set_period_size_near(requested_period_frames as AlsaFrames, AlsaValueOr::Nearest);
        let requested_buffer_frames = requested_period_frames.saturating_mul(4);
        let _ = hwp.set_buffer_size_near(requested_buffer_frames as AlsaFrames);

        pcm.hw_params(&hwp)
            .context("applying ALSA capture hardware params")?;
        (actual_channels, actual_sample_rate, sample_format)
    };

    let chunk_frames = {
        let hw_params = pcm
            .hw_params_current()
            .context("reading ALSA capture params")?;
        hw_params
            .get_period_size()
            .context("reading ALSA capture period size")?
            .max(1) as usize
    };

    pcm.prepare().context("preparing ALSA capture stream")?;
    pcm.start().context("starting ALSA capture stream")?;
    info!(
        alsa_device,
        ?sample_format,
        input_sample_rate = actual_sample_rate,
        input_channels = actual_channels,
        target_sample_rate = config.capture_sample_rate,
        target_channels = config.capture_channels,
        "started ALSA audio capture"
    );
    Ok((
        pcm,
        actual_sample_rate,
        actual_channels as usize,
        chunk_frames,
        sample_format,
    ))
}

fn run_playback_thread(
    config: AudioConfig,
    command_rx: mpsc::Receiver<PlaybackCommand>,
    ready_tx: mpsc::SyncSender<std::result::Result<(), String>>,
    volume: AudioVolumeState,
) -> Result<()> {
    if matches!(
        resolved_playback_backend(&config),
        ResolvedAudioBackend::Alsa
    ) {
        #[cfg(target_os = "linux")]
        if let Some(alsa_device) = resolved_alsa_playback_device(&config) {
            return alsa_backend::run_alsa_playback_thread(
                config,
                command_rx,
                ready_tx,
                &alsa_device,
                volume,
            );
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = ready_tx.send(Err(
                "ALSA playback backend is only supported on Linux".into()
            ));
            bail!("ALSA playback backend is only supported on Linux");
        }
    }

    let started = cpal_backend::start_cpal_playback_runtime(&config);

    let (stream, buffer, output_sample_rate, output_channels) = match started {
        Ok(started) => {
            let _ = ready_tx.send(Ok(()));
            started
        }
        Err(error) => {
            #[cfg(target_os = "linux")]
            if let Some(alsa_device) = resolved_alsa_playback_device(&config) {
                warn!(
                    cpal_error = %error,
                    alsa_device = %alsa_device,
                    "cpal playback unavailable; trying ALSA playback fallback"
                );
                return alsa_backend::run_alsa_playback_thread(
                    config,
                    command_rx,
                    ready_tx,
                    &alsa_device,
                    volume,
                );
            }

            let message = error.to_string();
            let _ = ready_tx.send(Err(message));
            return Err(error);
        }
    };

    loop {
        let command = {
            let buffered_samples = lock_mutex(&buffer, "playback buffer")?.len();
            if buffered_samples == 0 {
                match command_rx.recv() {
                    Ok(command) => command,
                    Err(_) => break,
                }
            } else if playback_buffered_ms(buffered_samples, output_sample_rate, output_channels)
                < playback_target_buffer_ms(&config)
            {
                match command_rx.try_recv() {
                    Ok(command) => command,
                    Err(mpsc::TryRecvError::Empty) => {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        continue;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(4));
                continue;
            }
        };

        match command {
            PlaybackCommand::Play { pcm, sample_rate } => {
                let mut rendered =
                    render_playback_pcm(&pcm, sample_rate, output_sample_rate, output_channels);
                apply_gain_in_place(&mut rendered, volume.playback_volume());
                let mut buffer = lock_mutex(&buffer, "playback buffer")?;
                buffer.extend(rendered);
                clamp_playback_buffer(
                    &mut buffer,
                    output_sample_rate,
                    output_channels,
                    playback_max_buffer_ms(&config),
                );
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

fn start_cpal_playback_runtime(
    config: &AudioConfig,
) -> Result<(Stream, Arc<Mutex<VecDeque<i16>>>, u32, usize)> {
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
    Ok((stream, buffer, sample_rate, channels))
}

#[cfg(target_os = "linux")]
struct AlsaPlaybackRuntime {
    pcm: AlsaPcm,
    output_sample_rate: u32,
    output_channels: usize,
    chunk_frames: usize,
    requested_sample_rate: u32,
}

#[cfg(target_os = "linux")]
fn run_alsa_playback_thread(
    config: AudioConfig,
    command_rx: mpsc::Receiver<PlaybackCommand>,
    ready_tx: mpsc::SyncSender<std::result::Result<(), String>>,
    alsa_device: &str,
) -> Result<()> {
    let started = create_alsa_playback_runtime(&config, alsa_device, None);
    let mut runtime = match started {
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

    let mut buffer = VecDeque::new();

    loop {
        if buffer.is_empty() {
            let command = match command_rx.recv() {
                Ok(command) => command,
                Err(_) => break,
            };
            if apply_playback_command(command, &mut buffer, &mut runtime, &config, alsa_device)? {
                break;
            }
            continue;
        }

        if playback_buffered_ms(
            buffer.len(),
            runtime.output_sample_rate,
            runtime.output_channels,
        ) < playback_target_buffer_ms(&config)
        {
            match command_rx.try_recv() {
                Ok(command) => {
                    if apply_playback_command(
                        command,
                        &mut buffer,
                        &mut runtime,
                        &config,
                        alsa_device,
                    )? {
                        let _ = runtime.pcm.drop();
                        info!("stopped ALSA audio playback");
                        return Ok(());
                    }
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = runtime.pcm.drop();
                    info!("stopped ALSA audio playback");
                    return Ok(());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        write_alsa_chunk(
            &runtime.pcm,
            &mut buffer,
            runtime.output_channels,
            runtime.chunk_frames,
        )?;
    }

    let _ = runtime.pcm.drop();
    info!("stopped ALSA audio playback");
    Ok(())
}

#[cfg(target_os = "linux")]
fn start_alsa_playback_runtime(
    config: &AudioConfig,
    alsa_device: &str,
) -> Result<AlsaPlaybackRuntime> {
    create_alsa_playback_runtime(config, alsa_device, Some(config.playback_sample_rate))
}

#[cfg(target_os = "linux")]
fn create_alsa_playback_runtime(
    config: &AudioConfig,
    alsa_device: &str,
    requested_sample_rate: Option<u32>,
) -> Result<AlsaPlaybackRuntime> {
    let pcm = AlsaPcm::new(alsa_device, AlsaDirection::Playback, false)
        .with_context(|| format!("opening ALSA playback PCM '{alsa_device}'"))?;
    let requested_rate = requested_sample_rate
        .unwrap_or(config.playback_sample_rate)
        .max(1);
    let (actual_sample_rate, output_channels, chunk_frames) =
        configure_alsa_playback_pcm(&pcm, config, requested_rate)?;

    info!(
        alsa_device,
        requested_rate,
        sample_rate = actual_sample_rate,
        channels = output_channels,
        "started ALSA audio playback fallback"
    );
    Ok(AlsaPlaybackRuntime {
        pcm,
        output_sample_rate: actual_sample_rate,
        output_channels,
        chunk_frames,
        requested_sample_rate: requested_rate,
    })
}

#[cfg(target_os = "linux")]
fn configure_alsa_playback_pcm(
    pcm: &AlsaPcm,
    config: &AudioConfig,
    requested_rate: u32,
) -> Result<(u32, usize, usize)> {
    let requested_channels = u32::from(config.playback_channels.max(1));
    let (actual_channels, actual_sample_rate) = {
        let hwp = AlsaHwParams::any(pcm).context("reading ALSA hardware params")?;
        hwp.set_access(AlsaAccess::RWInterleaved)
            .context("setting ALSA interleaved access")?;
        hwp.set_format(AlsaFormat::s16())
            .context("setting ALSA sample format")?;
        let _ = hwp.set_rate_resample(true);

        let actual_channels = hwp
            .set_channels_near(requested_channels)
            .context("setting ALSA playback channels")?;
        let actual_sample_rate = hwp
            .set_rate_near(requested_rate, AlsaValueOr::Nearest)
            .context("setting ALSA playback sample rate")?;

        let requested_buffer_frames =
            buffer_frames_for_ms(actual_sample_rate, config.playback_buffer_ms);
        let _ = hwp.set_buffer_size_near(requested_buffer_frames as AlsaFrames);
        let requested_period_frames = (requested_buffer_frames / 2).max(1);
        let _ =
            hwp.set_period_size_near(requested_period_frames as AlsaFrames, AlsaValueOr::Nearest);

        pcm.hw_params(&hwp)
            .context("applying ALSA hardware params")?;
        (actual_channels, actual_sample_rate)
    };

    let (chunk_frames, start_threshold) = {
        let hw_params = pcm
            .hw_params_current()
            .context("reading ALSA playback params")?;
        let chunk_frames = hw_params
            .get_period_size()
            .context("reading ALSA playback period size")?
            .max(1) as usize;
        let start_threshold = hw_params
            .get_buffer_size()
            .context("reading ALSA playback buffer size")?
            .max(1);
        (chunk_frames, start_threshold)
    };

    {
        let swp = pcm
            .sw_params_current()
            .context("reading ALSA software params")?;
        swp.set_start_threshold(start_threshold)
            .context("setting ALSA start threshold")?;
        swp.set_avail_min(chunk_frames as AlsaFrames)
            .context("setting ALSA minimum available frames")?;
        pcm.sw_params(&swp)
            .context("applying ALSA software params")?;
    }
    pcm.prepare().context("preparing ALSA playback stream")?;

    Ok((actual_sample_rate, actual_channels as usize, chunk_frames))
}

#[cfg(target_os = "linux")]
fn apply_playback_command(
    command: PlaybackCommand,
    buffer: &mut VecDeque<i16>,
    runtime: &mut AlsaPlaybackRuntime,
    config: &AudioConfig,
    alsa_device: &str,
) -> Result<bool> {
    match command {
        PlaybackCommand::Play {
            pcm: input,
            sample_rate,
        } => {
            if buffer.is_empty() && sample_rate > 0 && runtime.requested_sample_rate != sample_rate
            {
                debug!(
                    alsa_device,
                    previous_rate = runtime.requested_sample_rate,
                    requested_rate = sample_rate,
                    "reconfiguring ALSA playback runtime for source sample rate"
                );
                let _ = runtime.pcm.drop();
                let (actual_sample_rate, output_channels, chunk_frames) =
                    configure_alsa_playback_pcm(&runtime.pcm, config, sample_rate)?;
                runtime.output_sample_rate = actual_sample_rate;
                runtime.output_channels = output_channels;
                runtime.chunk_frames = chunk_frames;
                runtime.requested_sample_rate = sample_rate;
                info!(
                    alsa_device,
                    requested_rate = sample_rate,
                    sample_rate = runtime.output_sample_rate,
                    channels = runtime.output_channels,
                    "reconfigured ALSA playback runtime"
                );
            }

            let rendered = render_playback_pcm(
                &input,
                sample_rate,
                runtime.output_sample_rate,
                runtime.output_channels,
            );
            buffer.extend(rendered);
            clamp_playback_buffer(
                buffer,
                runtime.output_sample_rate,
                runtime.output_channels,
                playback_max_buffer_ms(config),
            );
            debug!(queued_samples = buffer.len(), "queued playback audio");
            Ok(false)
        }
        PlaybackCommand::Stop => {
            buffer.clear();
            let _ = runtime.pcm.drop();
            let _ = runtime.pcm.prepare();
            Ok(false)
        }
        PlaybackCommand::Shutdown => Ok(true),
    }
}

#[cfg(target_os = "linux")]
fn write_alsa_chunk(
    pcm: &AlsaPcm,
    buffer: &mut VecDeque<i16>,
    channels: usize,
    chunk_frames: usize,
) -> Result<()> {
    let channels = channels.max(1);
    let available_frames = buffer.len() / channels;
    if available_frames == 0 {
        return Ok(());
    }

    let frames_to_write = available_frames.min(chunk_frames.max(1));
    let sample_count = frames_to_write * channels;
    let chunk: Vec<i16> = buffer.iter().take(sample_count).copied().collect();
    let io = pcm.io_i16().context("opening ALSA i16 playback IO")?;

    match io.writei(&chunk) {
        Ok(written_frames) => {
            for _ in 0..written_frames.saturating_mul(channels) {
                let _ = buffer.pop_front();
            }
            if pcm.state() != AlsaState::Running {
                let _ = pcm.start();
            }
            Ok(())
        }
        Err(error) => {
            pcm.try_recover(error, true)
                .context("recovering ALSA playback stream")?;
            Ok(())
        }
    }
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
        i32_sample_to_i16_with_shift(self, 16)
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

fn resolved_capture_backend(config: &AudioConfig) -> ResolvedAudioBackend {
    resolve_backend(config.capture_backend, &config.capture_device)
}

fn resolved_playback_backend(config: &AudioConfig) -> ResolvedAudioBackend {
    resolve_backend(config.playback_backend, &config.playback_device)
}

#[cfg(target_os = "linux")]
fn resolved_alsa_capture_device(config: &AudioConfig) -> Option<String> {
    alsa_device_name(
        &config.capture_device,
        matches!(config.capture_backend, AudioBackend::Alsa),
    )
}

#[cfg(target_os = "linux")]
fn resolved_alsa_playback_device(config: &AudioConfig) -> Option<String> {
    alsa_device_name(
        &config.playback_device,
        matches!(config.playback_backend, AudioBackend::Alsa),
    )
}

fn resolve_backend(configured: AudioBackend, device: &str) -> ResolvedAudioBackend {
    match configured {
        AudioBackend::Cpal => ResolvedAudioBackend::Cpal,
        AudioBackend::Alsa => ResolvedAudioBackend::Alsa,
        AudioBackend::Auto => {
            if is_explicit_alsa_device(device) {
                ResolvedAudioBackend::Alsa
            } else {
                ResolvedAudioBackend::Cpal
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn preferred_alsa_capture_format(hwp: &AlsaHwParams<'_>) -> AlsaCaptureSampleFormat {
    if hwp.test_format(AlsaFormat::s32()).is_ok() {
        AlsaCaptureSampleFormat::S32
    } else {
        AlsaCaptureSampleFormat::S16
    }
}

#[cfg(target_os = "linux")]
fn alsa_capture_format(sample_format: AlsaCaptureSampleFormat) -> AlsaFormat {
    match sample_format {
        AlsaCaptureSampleFormat::S16 => AlsaFormat::s16(),
        AlsaCaptureSampleFormat::S32 => AlsaFormat::s32(),
    }
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

#[cfg(target_os = "linux")]
fn alsa_capture_device_available(device_name: &str) -> bool {
    AlsaPcm::new(device_name, AlsaDirection::Capture, true).is_ok()
}

#[cfg(target_os = "linux")]
fn alsa_playback_device_available(device_name: &str) -> bool {
    AlsaPcm::new(device_name, AlsaDirection::Playback, true).is_ok()
}

#[cfg(target_os = "linux")]
fn alsa_device_name(configured: &str, allow_raw: bool) -> Option<String> {
    let trimmed = configured.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
        return Some("default".to_string());
    }

    if let Some(device) = trimmed.strip_prefix("alsa:") {
        let device = device.trim();
        return Some(if device.is_empty() {
            "default".to_string()
        } else {
            device.to_string()
        });
    }

    if looks_like_alsa_pcm_name(trimmed) {
        return Some(trimmed.to_string());
    }

    if allow_raw {
        return Some(trimmed.to_string());
    }

    None
}

fn is_explicit_alsa_device(device: &str) -> bool {
    let trimmed = device.trim();
    !trimmed.is_empty()
        && !trimmed.eq_ignore_ascii_case("default")
        && looks_like_alsa_pcm_name(trimmed)
}

fn looks_like_alsa_pcm_name(device: &str) -> bool {
    [
        "alsa:",
        "default",
        "hw:",
        "plughw:",
        "plug:",
        "sysdefault:",
        "front:",
        "surround",
        "iec958:",
        "dmix:",
        "dsnoop:",
        "usbstream:",
    ]
    .iter()
    .any(|prefix| device.starts_with(prefix))
}

fn buffer_frames_for_ms(sample_rate: u32, buffer_ms: u32) -> usize {
    sample_rate
        .saturating_mul(buffer_ms.max(1))
        .saturating_div(1_000)
        .max(1) as usize
}

fn clamp_playback_buffer(
    buffer: &mut VecDeque<i16>,
    sample_rate: u32,
    channels: usize,
    max_buffer_ms: u64,
) {
    let max_samples = u64::from(sample_rate)
        .saturating_mul(channels.max(1) as u64)
        .saturating_mul(max_buffer_ms.max(1))
        .saturating_div(1_000)
        .max(1) as usize;
    if buffer.len() <= max_samples {
        return;
    }

    let dropped = buffer.len() - max_samples;
    buffer.truncate(max_samples);
    warn!(
        dropped_samples = dropped,
        max_buffer_ms, "playback buffer full; dropping queued tail"
    );
}

fn playback_buffered_ms(buffered_samples: usize, sample_rate: u32, channels: usize) -> u64 {
    let denom = u64::from(sample_rate).saturating_mul(channels.max(1) as u64);
    if denom == 0 {
        return 0;
    }
    (buffered_samples as u64)
        .saturating_mul(1_000)
        .saturating_div(denom)
}

fn i32_sample_to_i16_with_shift(sample: i32, shift: u8) -> i16 {
    (sample >> shift).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn detect_alsa_s32_capture_shift(samples: &[i32]) -> Option<u8> {
    let mut nonzero = 0usize;
    let mut low_byte_zero = 0usize;

    for &sample in samples.iter().take(4096) {
        if sample == 0 {
            continue;
        }
        nonzero += 1;
        if (sample & 0xFF) == 0 {
            low_byte_zero += 1;
        }
    }

    if nonzero == 0 {
        return None;
    }

    let low_zero_ratio = low_byte_zero as f32 / nonzero as f32;
    let stats_8 = s32_shift_stats(samples, 8);
    let stats_16 = s32_shift_stats(samples, 16);
    let stats_24 = s32_shift_stats(samples, 24);

    if stats_8.clipped_ratio > 0.02 && stats_16.clipped_ratio < stats_8.clipped_ratio {
        if stats_16.clipped_ratio > 0.02 && stats_24.peak > 128 {
            return Some(24);
        }
        return Some(16);
    }

    if low_zero_ratio > 0.90 && stats_16.peak > 128 {
        return Some(16);
    }

    if stats_8.peak >= 32_760 && stats_16.peak > 128 {
        return Some(16);
    }

    Some(8)
}

#[derive(Debug, Clone, Copy)]
struct S32ShiftStats {
    peak: u64,
    clipped_ratio: f32,
}

fn s32_shift_stats(samples: &[i32], shift: u8) -> S32ShiftStats {
    let mut considered = 0usize;
    let mut clipped = 0usize;
    let mut peak = 0u64;

    for sample in samples.iter().take(4096) {
        if *sample == 0 {
            continue;
        }
        considered += 1;
        let converted = i32_sample_to_i16_with_shift(*sample, shift);
        let abs = converted.unsigned_abs() as u64;
        peak = peak.max(abs);
        if abs >= 32_760 {
            clipped += 1;
        }
    }

    S32ShiftStats {
        peak,
        clipped_ratio: if considered == 0 {
            0.0
        } else {
            clipped as f32 / considered as f32
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_buffer_frames_for_milliseconds() {
        assert_eq!(buffer_frames_for_ms(24_000, 60), 1_440);
    }

    #[test]
    fn computes_buffered_playback_milliseconds() {
        assert_eq!(playback_buffered_ms(9_600, 24_000, 2), 200);
    }

    #[test]
    fn detects_right_justified_alsa_s32_capture() {
        let samples = [0x0001_0203, -0x0001_0203, 0x0002_0304, -0x0002_0304];
        assert_eq!(detect_alsa_s32_capture_shift(&samples), Some(8));
    }

    #[test]
    fn detects_left_justified_alsa_s32_capture() {
        let samples = [0x0102_0300, -0x0102_0300, 0x0203_0400, -0x0203_0400];
        assert_eq!(detect_alsa_s32_capture_shift(&samples), Some(16));
    }

    #[test]
    fn detects_left_justified_alsa_s32_capture_with_noisy_low_byte() {
        let samples = [0x1102_0311, -0x1102_0322, 0x2203_0433, -0x2203_0444];
        assert_eq!(detect_alsa_s32_capture_shift(&samples), Some(16));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detects_alsa_pcm_names() {
        assert_eq!(
            alsa_device_name("alsa:plughw:0,0", false),
            Some("plughw:0,0".to_string())
        );
        assert_eq!(
            alsa_device_name("default", false),
            Some("default".to_string())
        );
        assert_eq!(alsa_device_name("MAX98375A", false), None);
        assert_eq!(
            alsa_device_name("I2S-Duplex", true),
            Some("I2S-Duplex".to_string())
        );
    }

    #[test]
    fn resolves_explicit_alsa_backend_from_device_name() {
        let mut config = AudioConfig::default();
        config.capture_device = "alsa:plughw:2,0".to_string();
        config.playback_device = "plughw:0,1".to_string();

        assert_eq!(
            resolved_capture_backend(&config),
            ResolvedAudioBackend::Alsa
        );
        assert_eq!(
            resolved_playback_backend(&config),
            ResolvedAudioBackend::Alsa
        );
    }

    #[test]
    fn respects_explicit_backend_selection() {
        let mut config = AudioConfig::default();
        config.capture_backend = AudioBackend::Alsa;
        config.playback_backend = AudioBackend::Cpal;

        assert_eq!(
            resolved_capture_backend(&config),
            ResolvedAudioBackend::Alsa
        );
        assert_eq!(
            resolved_playback_backend(&config),
            ResolvedAudioBackend::Cpal
        );
    }
}
