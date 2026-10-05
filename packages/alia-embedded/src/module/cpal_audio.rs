use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use alia_core::delivery::{PlaybackIdentity, SubmissionBackend, SubmissionBoundary};
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

use super::context::{AudioBus, AudioFrame, AudioReferenceBus};
use super::lifecycle::{ModuleHealth, ModuleLifecycle};
use super::traits::{AudioCaptureBackend, AudioPlaybackBackend};
use super::volume::{AudioVolumeState, apply_gain_in_place};

#[cfg(target_os = "linux")]
mod alsa_backend;
mod clock;
mod cpal_backend;
mod playback;

use super::playback_session::AudioSubmission;
use playback::{
    AudioBlock, PlaybackBuffer, PlaybackReceiver, PlaybackSender, PlaybackSink, Renderer,
};

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
    runtime: Mutex<Option<PlaybackRuntime>>,
    generation: Arc<AtomicU64>,
    reference: Option<AudioReferenceBus>,
}

struct PlaybackRuntime {
    sender: PlaybackSender,
    thread: JoinHandle<()>,
}

impl CpalAudioPlayback {
    pub fn new(config: AudioConfig, volume: AudioVolumeState) -> Self {
        Self {
            config,
            volume,
            runtime: Mutex::new(None),
            generation: Arc::new(AtomicU64::new(0)),
            reference: None,
        }
    }

    /// Scoped ingress never initializes or resurrects native audio. The module
    /// lifecycle owns device startup; an unhealthy backend rejects new delivery.
    fn active_sender(&self) -> Result<PlaybackSender> {
        lock_mutex(&self.runtime, "playback runtime")?
            .as_ref()
            .filter(|runtime| !runtime.thread.is_finished())
            .map(|runtime| runtime.sender.clone())
            .context("playback backend is not running")
    }

    fn start_stream(&self) -> Result<PlaybackSender> {
        let reference = self
            .reference
            .clone()
            .context("playback audio clock is not initialized")?;
        let mut runtime = lock_mutex(&self.runtime, "playback runtime")?;
        if let Some(current) = runtime.as_ref()
            && !current.thread.is_finished()
        {
            return Ok(current.sender.clone());
        }
        if let Some(previous) = runtime.take() {
            previous.sender.shutdown();
            let _ = previous.thread.join();
        }

        let (sender, receiver) = playback::channel_with_generation(
            playback_command_queue_capacity(&self.config),
            self.generation.clone(),
        );
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let config = self.config.clone();
        let volume = self.volume.clone();

        let handle = thread::Builder::new()
            .name("alia-cpal-playback".to_string())
            .spawn(move || {
                let result = run_playback_thread(config, receiver, ready_tx, volume, reference);
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
                *runtime = Some(PlaybackRuntime {
                    sender: sender.clone(),
                    thread: handle,
                });
                Ok(sender)
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

    async fn init(&mut self, ctx: &super::context::ModuleContext) -> Result<()> {
        self.reference = Some(ctx.audio_bus.reference());
        Ok(())
    }

    async fn start(&self) -> Result<()> {
        self.start_stream().map(|_| ())
    }

    async fn stop(&self) -> Result<()> {
        let mut runtime = lock_mutex(&self.runtime, "playback runtime")?;
        if let Some(current) = runtime.take() {
            current.sender.shutdown();
            current
                .thread
                .join()
                .map_err(|_| anyhow!("playback worker panicked"))?;
        }
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
    fn playback_control(
        &self,
        scope: &alia_core::delivery::VoiceScope,
        generation: u64,
    ) -> Result<Arc<super::playback_session::PlaybackControl>> {
        self.active_sender()?.playback_control(*scope, generation)
    }
    fn apply_playback_control(
        &self,
        owner: &Arc<super::playback_session::PlaybackControl>,
        command: &alia_core::playback_control::ControlCommand,
    ) -> Result<alia_core::playback_control::ControlReceipt> {
        self.active_sender()?.apply_control(owner, command)
    }
    fn reference_bus(&self) -> Option<AudioReferenceBus> {
        self.reference.clone()
    }
    fn open_submission(
        &self,
        identity: PlaybackIdentity,
        sample_rate: u32,
        generation: u64,
    ) -> Result<Arc<AudioSubmission>> {
        self.active_sender()?
            .open_submission(identity, sample_rate, generation)
    }

    fn open_cue_submission(
        &self,
        identity: PlaybackIdentity,
        sample_rate: u32,
        generation: u64,
    ) -> Result<Arc<AudioSubmission>> {
        self.active_sender()?
            .open_cue_submission(identity, sample_rate, generation)
    }

    fn submit_audio(
        &self,
        submission: &Arc<AudioSubmission>,
        offset: u64,
        pcm: &[i16],
    ) -> Result<bool> {
        let sender = lock_mutex(&self.runtime, "playback runtime")?
            .as_ref()
            .map(|r| r.sender.clone());
        match sender {
            Some(sender) => sender.submit(submission, offset, pcm, false),
            None => Ok(false),
        }
    }

    fn end_submission(&self, submission: &Arc<AudioSubmission>, total: u64) -> Result<bool> {
        let sender = lock_mutex(&self.runtime, "playback runtime")?
            .as_ref()
            .map(|r| r.sender.clone());
        match sender {
            Some(sender) => sender.submit(submission, total, &[], true),
            None => Ok(false),
        }
    }

    fn cancel_submission(&self, submission: &Arc<AudioSubmission>, reason: &str) -> Result<bool> {
        Ok(lock_mutex(&self.runtime, "playback runtime")?
            .as_ref()
            .is_some_and(|r| r.sender.cancel_submission(submission, reason)))
    }
    fn begin_scoped_playback(&self) -> Result<u64> {
        self.active_sender()?.begin()
    }

    fn stop_if_current(&self, generation: u64) -> Result<bool> {
        Ok(lock_mutex(&self.runtime, "playback runtime")?
            .as_ref()
            .is_some_and(|runtime| runtime.sender.stop_if_current(generation)))
    }

    fn is_playback_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::Acquire) == generation
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedAudioBackend {
    Cpal,
    Alsa,
}

fn playback_target_buffer_ms(config: &AudioConfig) -> u64 {
    u64::from(
        config
            .playback_target_buffer_ms
            .max(1)
            .min(config.playback_max_buffer_ms.max(1)),
    )
}

fn playback_command_queue_capacity(config: &AudioConfig) -> usize {
    config.playback_command_queue_capacity.max(1)
}

#[cfg(target_os = "linux")]
fn alsa_playback_idle_sample_rate(config: &AudioConfig) -> u32 {
    if config.alsa_playback_lock_to_capture_rate {
        config.capture_sample_rate
    } else {
        config.playback_sample_rate
    }
    .max(1)
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
            return alsa_backend::run_alsa_capture_thread(
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

fn run_playback_thread(
    config: AudioConfig,
    receiver: PlaybackReceiver,
    ready_tx: mpsc::SyncSender<std::result::Result<(), String>>,
    volume: AudioVolumeState,
    reference: AudioReferenceBus,
) -> Result<()> {
    if matches!(
        resolved_playback_backend(&config),
        ResolvedAudioBackend::Alsa
    ) {
        #[cfg(target_os = "linux")]
        if let Some(alsa_device) = resolved_alsa_playback_device(&config) {
            return alsa_backend::run_alsa_playback_thread(
                config,
                receiver,
                ready_tx,
                &alsa_device,
                volume,
                reference,
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

    let started = cpal_backend::start_cpal_playback_runtime(
        &config,
        receiver.generation(),
        volume.clone(),
        receiver.device_failed(),
        reference.clone(),
    );
    let mut sink = match started {
        Ok(sink) => sink,
        Err(error) => {
            #[cfg(target_os = "linux")]
            if let Some(alsa_device) = resolved_alsa_playback_device(&config) {
                warn!(cpal_error = %error, alsa_device = %alsa_device,
                    "cpal playback unavailable; trying ALSA playback fallback");
                return alsa_backend::run_alsa_playback_thread(
                    config,
                    receiver,
                    ready_tx,
                    &alsa_device,
                    volume,
                    reference,
                );
            }
            let _ = ready_tx.send(Err(error.to_string()));
            return Err(error);
        }
    };
    receiver.set_backend(sink.backend.clone());
    if ready_tx.send(Ok(())).is_err() {
        return Ok(());
    }
    let result = playback::run(receiver, &mut sink, playback_target_buffer_ms(&config));
    info!("stopped cpal audio playback");
    result
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

    fn reset(&mut self) {
        self.last_sample = None;
        self.phase = 0.0;
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

pub fn resolved_capture_backend(config: &AudioConfig) -> ResolvedAudioBackend {
    resolve_backend(config.capture_backend, &config.capture_device)
}

pub fn resolved_playback_backend(config: &AudioConfig) -> ResolvedAudioBackend {
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
pub fn alsa_device_name(configured: &str, allow_raw: bool) -> Option<String> {
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

pub fn buffer_frames_for_ms(sample_rate: u32, buffer_ms: u32) -> usize {
    sample_rate
        .saturating_mul(buffer_ms.max(1))
        .saturating_div(1_000)
        .max(1) as usize
}

pub fn playback_buffered_ms(buffered_samples: usize, sample_rate: u32, channels: usize) -> u64 {
    let denom = u64::from(sample_rate).saturating_mul(channels.max(1) as u64);
    if denom == 0 {
        return 0;
    }
    (buffered_samples as u64)
        .saturating_mul(1_000)
        .div_ceil(denom)
}

fn i32_sample_to_i16_with_shift(sample: i32, shift: u8) -> i16 {
    (sample >> shift).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

pub fn detect_alsa_s32_capture_shift(samples: &[i32]) -> Option<u8> {
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
