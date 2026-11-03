use super::super::context::{
    AudioSampleBoundary, AudioStreamClock, AudioStreamOwner, AudioStreamProbe, ReferenceContent,
};
use super::clock::{AlsaTimeOrigin, observe_capture_samples};
use super::*;
use alia_core::audio_clock::*;
use anyhow::ensure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlsaCaptureSampleFormat {
    S16,
    S32,
}

struct AlsaPlaybackRuntime {
    pcm: AlsaPcm,
    output_sample_rate: u32,
    output_channels: usize,
    chunk_frames: usize,
    timestamps_enabled: bool,
}

struct AlsaCaptureOwner {
    pcm: AlsaPcm,
    _clock_owner: AudioStreamOwner,
}

pub(super) fn run_alsa_capture_thread(
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
    let mut clock = bus.reference().open_stream(
        AudioDirection::Capture,
        AudioFormat {
            sample_rate_hz: input_sample_rate,
            channels: input_channels as u16,
        },
    );
    let owner = AlsaCaptureOwner {
        pcm,
        _clock_owner: AudioStreamOwner(clock.probe.clone()),
    };
    let pcm = &owner.pcm;
    owner._clock_owner.0.activate();
    let timestamps_enabled = enable_timestamps(pcm);
    let mut time_origin = AlsaTimeOrigin::default();
    let mut continuity = clock.probe.continuity();
    let mut emitted_frames = 0_u64;
    let mut level_peak = 0_u64;
    let mut silence_warned = false;
    let mut clipping_warned = false;
    let mut detected_s32_shift = alsa_capture_s32_shift_override(&config);
    let sample_count = chunk_frames.saturating_mul(input_channels.max(1));
    let mut raw_buffer_i16 = vec![0_i16; sample_count];
    let mut raw_buffer_i32 = vec![0_i32; sample_count];

    loop {
        match control_rx.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        let ready = handle_capture_wait(pcm, &mut converter, &clock.probe, pcm.wait(Some(100)))?;
        if !ready {
            continue;
        }

        let reading = read_clock(pcm, &mut time_origin, &clock.probe, timestamps_enabled);
        let (frames_read, captured_pcm, native) = match sample_format {
            AlsaCaptureSampleFormat::S16 => {
                let io = pcm
                    .io_i16()
                    .inspect_err(|_| clock.probe.mark(AudioDiscontinuity::BackendError))
                    .context("opening ALSA i16 capture IO")?;
                match io.readi(&mut raw_buffer_i16) {
                    Ok(frames) => {
                        let samples_read = frames.saturating_mul(input_channels);
                        let native = observe_capture_samples(
                            &mut clock,
                            &raw_buffer_i16[..samples_read],
                            reading,
                            AudioSampleBoundary::AlsaCaptureRead,
                        );
                        if native.continuity != continuity {
                            converter.reset();
                            continuity = native.continuity;
                        }
                        (
                            frames,
                            converter.convert(&raw_buffer_i16[..samples_read]),
                            native,
                        )
                    }
                    Err(error) => {
                        recover_capture_error(pcm, &mut converter, &clock.probe, error)?;
                        continue;
                    }
                }
            }
            AlsaCaptureSampleFormat::S32 => {
                let io = pcm
                    .io_i32()
                    .inspect_err(|_| clock.probe.mark(AudioDiscontinuity::BackendError))
                    .context("opening ALSA i32 capture IO")?;
                match io.readi(&mut raw_buffer_i32) {
                    Ok(frames) => {
                        let samples_read = frames.saturating_mul(input_channels);
                        let raw = &raw_buffer_i32[..samples_read];
                        let native = observe_capture_samples(
                            &mut clock,
                            raw,
                            reading,
                            AudioSampleBoundary::AlsaCaptureRead,
                        );
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
                        if emitted_frames == 0 {
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
                        if native.continuity != continuity {
                            converter.reset();
                            continuity = native.continuity;
                        }
                        (frames, converter.convert(&converted), native)
                    }
                    Err(error) => {
                        recover_capture_error(pcm, &mut converter, &clock.probe, error)?;
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
            timing: Some(CaptureTiming {
                native,
                converted_format: AudioFormat {
                    sample_rate_hz: target_sample_rate,
                    channels: target_channels as u16,
                },
                converted_start_frame: start_frame,
                converted_frames: frames,
                resampled: input_sample_rate != target_sample_rate,
            }),
        });
    }

    let stopped = pcm
        .drop()
        .inspect_err(|_| clock.probe.mark(AudioDiscontinuity::BackendError))
        .context("stopping ALSA capture");
    drop(owner);
    info!("stopped ALSA audio capture");
    stopped
}

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

pub(super) fn run_alsa_playback_thread(
    config: AudioConfig,
    receiver: PlaybackReceiver,
    ready_tx: mpsc::SyncSender<std::result::Result<(), String>>,
    alsa_device: &str,
    volume: AudioVolumeState,
    reference: AudioReferenceBus,
) -> Result<()> {
    let runtime = match create_alsa_playback_runtime(&config, alsa_device) {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready_tx.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let target_buffer_ms = playback_target_buffer_ms(&config);
    let clock = reference.open_stream(
        AudioDirection::Playback,
        AudioFormat {
            sample_rate_hz: runtime.output_sample_rate,
            channels: runtime.output_channels as u16,
        },
    );
    receiver.set_backend(SubmissionBackend {
        instance_id: clock.probe.stream_id(),
        boundary: SubmissionBoundary::AlsaWriteAccepted,
        output_sample_rate_hz: runtime.output_sample_rate,
        output_channels: runtime.output_channels as u32,
    });
    let mut sink = AlsaPlaybackSink {
        _clock_owner: AudioStreamOwner(clock.probe.clone()),
        clock,
        time_origin: AlsaTimeOrigin::default(),
        runtime,
        buffer: PlaybackBuffer::new(receiver.generation()),
        renderer: Renderer::default(),
        volume,
    };
    if ready_tx.send(Ok(())).is_err() {
        return Ok(());
    }
    let result = playback::run(receiver, &mut sink, target_buffer_ms);
    info!("stopped ALSA audio playback");
    result
}

struct AlsaPlaybackSink {
    runtime: AlsaPlaybackRuntime,
    _clock_owner: AudioStreamOwner,
    clock: AudioStreamClock,
    time_origin: AlsaTimeOrigin,
    buffer: PlaybackBuffer,
    renderer: Renderer,
    volume: AudioVolumeState,
}

impl PlaybackSink for AlsaPlaybackSink {
    fn set_control(
        &mut self,
        control: Option<Arc<super::super::playback_session::PlaybackControl>>,
    ) {
        self.buffer.set_control(control);
    }
    fn buffered_ms(&self) -> u64 {
        playback_buffered_ms(
            self.buffer.len(),
            self.runtime.output_sample_rate,
            self.runtime.output_channels,
        )
    }

    fn source_buffered_ms(&self, source: crate::module::mixer::MixerSource) -> u64 {
        playback_buffered_ms(
            self.buffer.source_len(source),
            self.runtime.output_sample_rate,
            self.runtime.output_channels,
        )
    }

    fn enqueue(&mut self, block: AudioBlock) -> Result<()> {
        if !self.buffer.is_current(block.generation) {
            return Ok(());
        }
        let mut rendered = self.renderer.render(
            block,
            self.runtime.output_sample_rate,
            self.runtime.output_channels,
        )?;
        apply_gain_in_place(&mut rendered.samples, self.volume.playback_volume());
        self.buffer.append_rendered(rendered)?;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.clock.probe.mark(AudioDiscontinuity::Cancelled);
        self.renderer.clear();
        self.buffer.clear();
        let state_before = self.runtime.pcm.state();
        self.runtime
            .pcm
            .drop()
            .inspect_err(|error| {
                self.clock.probe.mark(AudioDiscontinuity::BackendError);
                error!(
                    stage = "drop",
                    ?state_before,
                    state_after = ?self.runtime.pcm.state(),
                    errno = error.errno(),
                    %error,
                    "ALSA playback stop failed"
                );
            })
            .context("stopping ALSA playback")?;
        let state_before = self.runtime.pcm.state();
        self.runtime
            .pcm
            .prepare()
            .inspect_err(|error| {
                self.clock.probe.mark(AudioDiscontinuity::BackendError);
                error!(
                    stage = "prepare",
                    ?state_before,
                    state_after = ?self.runtime.pcm.state(),
                    errno = error.errno(),
                    %error,
                    "ALSA playback stop failed"
                );
            })
            .context("preparing ALSA after stop")?;
        Ok(())
    }

    fn pump(&mut self) -> Result<()> {
        if self.buffer.len() != 0 || self.buffer.has_control() {
            write_alsa_chunk(
                &self.runtime.pcm,
                &mut self.buffer,
                self.runtime.output_channels,
                self.runtime.chunk_frames,
                &mut self.clock,
                &mut self.time_origin,
                self.runtime.timestamps_enabled,
            )?;
        }
        Ok(())
    }
}

impl Drop for AlsaPlaybackSink {
    fn drop(&mut self) {
        let _ = self.runtime.pcm.drop();
    }
}

fn create_alsa_playback_runtime(
    config: &AudioConfig,
    alsa_device: &str,
) -> Result<AlsaPlaybackRuntime> {
    let pcm = AlsaPcm::new(alsa_device, AlsaDirection::Playback, true)
        .with_context(|| format!("opening ALSA playback PCM '{alsa_device}'"))?;
    let requested_rate = alsa_playback_idle_sample_rate(config).max(1);
    let (actual_sample_rate, output_channels, chunk_frames) =
        configure_alsa_playback_pcm(&pcm, config, requested_rate)?;

    info!(
        alsa_device,
        requested_rate,
        sample_rate = actual_sample_rate,
        channels = output_channels,
        "started ALSA audio playback fallback"
    );
    let timestamps_enabled = enable_timestamps(&pcm);
    Ok(AlsaPlaybackRuntime {
        pcm,
        timestamps_enabled,
        output_sample_rate: actual_sample_rate,
        output_channels,
        chunk_frames,
    })
}

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

fn write_alsa_chunk(
    pcm: &AlsaPcm,
    buffer: &mut PlaybackBuffer,
    channels: usize,
    chunk_frames: usize,
    clock: &mut AudioStreamClock,
    time_origin: &mut AlsaTimeOrigin,
    timestamps_enabled: bool,
) -> Result<()> {
    let channels = channels.max(1);
    let controlled = buffer.has_control();
    let priming = controlled && pcm.state() == AlsaState::Prepared;
    let silence = if buffer.is_held() {
        Some(ReferenceContent::HeldSilence)
    } else if priming || (buffer.len() == 0 && controlled) {
        // A streamed response can wait seconds for its next synthesized segment.
        Some(ReferenceContent::ZeroFill)
    } else {
        None
    };
    let available_frames = if silence.is_some() {
        chunk_frames.clamp(1, 4096)
    } else {
        buffer.len() / channels
    };
    if available_frames == 0 {
        return Ok(());
    }

    let frames_to_write = if priming {
        // Prime the existing native buffer before accepting even a short End.
        pcm.get_params()
            .inspect_err(|_| clock.probe.mark(AudioDiscontinuity::BackendError))
            .context("reading ALSA startup buffer size")?
            .0
            .clamp(1, 4096) as usize
    } else {
        available_frames.min(chunk_frames.max(1))
    };
    let (chunk, output_write) = if silence.is_some() {
        (vec![0; frames_to_write * channels], None)
    } else {
        buffer.alsa_chunk(frames_to_write)
    };
    if chunk.is_empty() {
        return Ok(());
    }
    let io = pcm
        .io_i16()
        .inspect_err(|_| clock.probe.mark(AudioDiscontinuity::BackendError))
        .context("opening ALSA i16 playback IO")?;

    match io.writei(&chunk) {
        Ok(written_frames) => {
            ensure!(
                written_frames <= chunk.len() / channels,
                "ALSA accepted beyond the offered output buffer"
            );
            let reading = read_clock(pcm, time_origin, &clock.probe, timestamps_enabled);
            if let Some(content) = silence {
                super::clock::record_samples(
                    clock,
                    &chunk[..written_frames * channels],
                    reading,
                    SubmissionBoundary::AlsaWriteAccepted,
                    content,
                );
            } else {
                playback::confirm_alsa_write(
                    buffer,
                    &chunk,
                    written_frames,
                    output_write,
                    clock,
                    reading,
                )?;
            }
            if controlled {
                // ALSA starts only when the configured buffer threshold is met.
                Ok(())
            } else {
                start_accepted_playback(pcm, written_frames, &clock.probe)
            }
        }
        Err(error) if matches!(error.errno(), libc::EAGAIN | libc::EINTR) => Ok(()),
        Err(error) if error.errno() == libc::EPIPE => {
            clock.probe.mark(AudioDiscontinuity::Underrun);
            ensure!(
                !buffer.has_control(),
                "native underrun lost controlled playback continuity"
            );
            pcm.prepare().context("recovering ALSA playback underrun")
        }
        Err(error) if error.errno() == libc::ESTRPIPE => {
            clock.probe.mark(AudioDiscontinuity::Suspended);
            ensure!(
                !buffer.has_control(),
                "native suspend lost controlled playback continuity"
            );
            match pcm.resume() {
                Ok(()) => Ok(()),
                Err(error) if error.errno() == libc::EAGAIN => Ok(()),
                Err(_) => pcm.prepare().context("recovering suspended ALSA playback"),
            }
        }
        Err(error) => {
            clock.probe.mark(AudioDiscontinuity::BackendError);
            Err(error).context("writing ALSA playback audio")
        }
    }
}

fn start_accepted_playback(
    pcm: &AlsaPcm,
    written_frames: usize,
    probe: &AudioStreamProbe,
) -> Result<()> {
    if written_frames > 0 && pcm.state() != AlsaState::Running {
        pcm.start()
            .inspect_err(|_| probe.mark(AudioDiscontinuity::BackendError))
            .context("starting ALSA after accepted playback write")?;
    }
    Ok(())
}

fn handle_capture_wait(
    pcm: &AlsaPcm,
    converter: &mut CaptureConverter,
    probe: &AudioStreamProbe,
    waited: alsa::Result<bool>,
) -> Result<bool> {
    match waited {
        Ok(ready) => Ok(ready),
        Err(error) => {
            recover_capture_error(pcm, converter, probe, error)?;
            Ok(false)
        }
    }
}

fn recover_capture_error(
    pcm: &AlsaPcm,
    converter: &mut CaptureConverter,
    probe: &AudioStreamProbe,
    error: alsa::Error,
) -> Result<()> {
    if matches!(error.errno(), libc::EAGAIN | libc::EINTR) {
        return Ok(());
    }
    probe.mark(capture_error_reason(error.errno()));
    converter.reset();
    if matches!(error.errno(), libc::EPIPE | libc::ESTRPIPE) {
        // Treat the unknown native tail as a gap and restart capture once. The
        // libasound recover helper can loop forever while suspend returns
        // EAGAIN, which would prevent this owner's stop and join from finishing.
        pcm.prepare()
            .and_then(|()| pcm.start())
            .inspect_err(|_| probe.mark(AudioDiscontinuity::BackendError))
            .context("restarting ALSA capture after a discontinuity")
    } else {
        Err(error).context("ALSA capture backend failed")
    }
}

fn preferred_alsa_capture_format(hwp: &AlsaHwParams<'_>) -> AlsaCaptureSampleFormat {
    if hwp.test_format(AlsaFormat::s32()).is_ok() {
        AlsaCaptureSampleFormat::S32
    } else {
        AlsaCaptureSampleFormat::S16
    }
}

fn alsa_capture_format(sample_format: AlsaCaptureSampleFormat) -> AlsaFormat {
    match sample_format {
        AlsaCaptureSampleFormat::S16 => AlsaFormat::s16(),
        AlsaCaptureSampleFormat::S32 => AlsaFormat::s32(),
    }
}

// Failure here only downgrades timing: it never discards a successful write's
// accepted prefix. Timestamp configuration is explicit, never wall-clock guessed.
fn enable_timestamps(pcm: &AlsaPcm) -> bool {
    (|| -> Result<()> {
        let swp = pcm.sw_params_current()?;
        swp.set_tstamp_mode(true)?;
        swp.set_tstamp_type(alsa::pcm::TstampType::Monotonic)?;
        pcm.sw_params(&swp)?;
        Ok(())
    })()
    .is_ok()
}
fn read_clock(
    pcm: &AlsaPcm,
    origin: &mut AlsaTimeOrigin,
    probe: &AudioStreamProbe,
    enabled: bool,
) -> AudioClockReading {
    if !enabled {
        return probe.unknown_clock();
    }
    let Ok(status) = pcm.status() else {
        return probe.unknown_clock();
    };
    let stamp = status.get_htstamp();
    let timestamp = if stamp.tv_sec >= 0
        && (0..1_000_000_000).contains(&stamp.tv_nsec)
        && (stamp.tv_sec != 0 || stamp.tv_nsec != 0)
    {
        Some(i128::from(stamp.tv_sec) * 1_000_000_000 + i128::from(stamp.tv_nsec))
    } else {
        None
    };
    origin.read(probe, timestamp, Some(status.get_delay()))
}
fn capture_error_reason(errno: i32) -> AudioDiscontinuity {
    match errno {
        libc::EPIPE => AudioDiscontinuity::Overrun,
        libc::ESTRPIPE => AudioDiscontinuity::Suspended,
        _ => AudioDiscontinuity::BackendError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn alsa_controlled_start_primes_native_buffer_before_short_end_across_generations() {
        use alia_core::delivery::{ObservationEnd, VoiceScope};

        let runtime = create_alsa_playback_runtime(
            &AudioConfig {
                playback_sample_rate: 16000,
                playback_channels: 2,
                ..Default::default()
            },
            "null",
        )
        .unwrap();
        let native_frames = runtime
            .pcm
            .hw_params_current()
            .unwrap()
            .get_buffer_size()
            .unwrap() as u64;
        assert!(native_frames > runtime.chunk_frames as u64);
        let (sender, receiver) = playback::channel(8);
        let bus = AudioBus::new(2).reference();
        let mut reader = bus.try_claim_reader().unwrap();
        let clock = bus.open_stream(
            AudioDirection::Playback,
            AudioFormat {
                sample_rate_hz: runtime.output_sample_rate,
                channels: runtime.output_channels as u16,
            },
        );
        receiver.set_backend(SubmissionBackend {
            instance_id: clock.probe.stream_id(),
            boundary: SubmissionBoundary::AlsaWriteAccepted,
            output_sample_rate_hz: runtime.output_sample_rate,
            output_channels: runtime.output_channels as u32,
        });
        let mut sink = AlsaPlaybackSink {
            _clock_owner: AudioStreamOwner(clock.probe.clone()),
            clock,
            time_origin: AlsaTimeOrigin::default(),
            runtime,
            buffer: PlaybackBuffer::new(receiver.generation()),
            renderer: Renderer::default(),
            volume: AudioVolumeState::new(Default::default()),
        };
        let session_id = uuid::Uuid::new_v4();
        let device_boot_id = uuid::Uuid::new_v4();
        for epoch in [1, 2] {
            let generation = sender.begin().unwrap();
            assert!(matches!(
                receiver.take(true),
                Some(playback::PlaybackAction::Stop)
            ));
            sink.stop().unwrap();
            assert_eq!(sink.runtime.pcm.state(), AlsaState::Prepared);
            let scope = VoiceScope {
                device_boot_id,
                session_id,
                generation_epoch: epoch,
            };
            let submission = sender
                .open_submission(
                    PlaybackIdentity {
                        scope,
                        playback_id: uuid::Uuid::new_v4(),
                        playback_sequence: epoch,
                    },
                    16000,
                    generation,
                )
                .unwrap();
            // The complete short reply is queued before the first native pump.
            sender.submit(&submission, 0, &[1234; 160], false).unwrap();
            sender.submit(&submission, 160, &[], true).unwrap();
            sink.set_control(Some(sender.playback_control(scope, generation).unwrap()));
            while let Some(action) = receiver.take(true) {
                let playback::PlaybackAction::Play(block) = action else {
                    panic!("unexpected control action")
                };
                sink.enqueue(block).unwrap();
            }
            while reader.try_read().is_some() {}
            let before = bus.snapshot(AudioDirection::Playback).unwrap().cursor;
            sink.pump().unwrap();
            let first = submission.snapshot(1);
            assert_eq!(
                first.submitted_source_frames,
                Some(0),
                "controlled startup must prime before submitting short media"
            );
            assert!(first.observation_end.is_none());
            assert_eq!(sink.runtime.pcm.state(), AlsaState::Running);
            assert_eq!(
                bus.snapshot(AudioDirection::Playback).unwrap().cursor - before,
                native_frames
            );
            let mut zeros = 0;
            while let Some(block) = reader.try_read() {
                if block.sample_count == 0 {
                    continue;
                }
                assert_eq!(block.content, ReferenceContent::ZeroFill);
                assert_eq!(block.boundary, Some(AudioSampleBoundary::AlsaWriteAccepted));
                assert!(
                    block.samples[..block.sample_count]
                        .iter()
                        .all(|sample| *sample == 0.0)
                );
                zeros += block.timing.frames;
            }
            assert_eq!(zeros, native_frames);
            sink.pump().unwrap();
            let finished = submission.snapshot(2);
            assert_eq!(finished.submitted_source_frames, Some(160));
            assert_eq!(
                finished.observation_end,
                Some(ObservationEnd::SubmissionFinished)
            );
            assert!(sender.stop_if_current(generation));
            assert!(matches!(
                receiver.take(true),
                Some(playback::PlaybackAction::Stop)
            ));
            sink.stop().unwrap();
            sink.set_control(None);
            assert_eq!(sink.runtime.pcm.state(), AlsaState::Prepared);
            let stopped = bus.snapshot(AudioDirection::Playback).unwrap().cursor;
            sink.pump().unwrap();
            assert_eq!(
                bus.snapshot(AudioDirection::Playback).unwrap().cursor,
                stopped
            );
        }
    }

    #[test]
    fn alsa_stream_gaps_write_silence_without_advancing_or_finishing_source_audio() {
        use crate::module::context::ReferenceContent;
        use alia_core::delivery::{ObservationEnd, PlaybackIdentity, VoiceScope};

        std::thread::scope(|threads| {
            let (sender, receiver) = playback::channel(8);
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let bus = AudioBus::new(2).reference();
            let mut reader = bus.try_claim_reader().unwrap();
            let reference = bus.clone();
            let worker = threads.spawn(move || {
                run_alsa_playback_thread(
                    AudioConfig {
                        playback_sample_rate: 16000,
                        playback_channels: 2,
                        ..Default::default()
                    },
                    receiver,
                    ready_tx,
                    "null",
                    AudioVolumeState::new(Default::default()),
                    reference,
                )
            });
            ready_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap();
            assert_eq!(bus.snapshot(AudioDirection::Playback).unwrap().cursor, 0);
            let generation = sender.begin().unwrap();
            let submission = sender
                .open_submission(
                    PlaybackIdentity {
                        scope: VoiceScope {
                            device_boot_id: uuid::Uuid::new_v4(),
                            session_id: uuid::Uuid::new_v4(),
                            generation_epoch: 1,
                        },
                        playback_id: uuid::Uuid::new_v4(),
                        playback_sequence: 1,
                    },
                    16000,
                    generation,
                )
                .unwrap();
            let mut next_content = |content| {
                let deadline = Instant::now() + Duration::from_secs(1);
                loop {
                    if let Some(block) = reader.try_read() {
                        if block.content == content && block.sample_count > 0 {
                            assert_eq!(
                                block.boundary,
                                Some(AudioSampleBoundary::AlsaWriteAccepted)
                            );
                            if content == ReferenceContent::ZeroFill {
                                assert!(
                                    block.samples[..block.sample_count]
                                        .iter()
                                        .all(|x| *x == 0.0)
                                );
                            }
                            break;
                        }
                    }
                    assert!(
                        Instant::now() < deadline,
                        "missing actual {content:?} write"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
            };
            // Before the first segment and between segments, real null writes
            // must keep the native stream fed without inventing source progress.
            next_content(ReferenceContent::ZeroFill);
            assert_eq!(submission.snapshot(1).submitted_source_frames, Some(0));
            sender.submit(&submission, 0, &[1234; 160], false).unwrap();
            next_content(ReferenceContent::Media);
            next_content(ReferenceContent::ZeroFill);
            let gap = submission.snapshot(2);
            assert_eq!(gap.submitted_source_frames, Some(160));
            assert!(!gap.end_accepted && gap.observation_end.is_none());
            sender
                .submit(&submission, 160, &[2345; 160], false)
                .unwrap();
            sender.submit(&submission, 320, &[], true).unwrap();
            next_content(ReferenceContent::Media);
            next_content(ReferenceContent::ZeroFill);
            let finished = submission.snapshot(3);
            assert_eq!(finished.submitted_source_frames, Some(320));
            assert_eq!(
                finished.observation_end,
                Some(ObservationEnd::SubmissionFinished)
            );
            sender.shutdown();
            worker.join().unwrap().unwrap();
            assert!(bus.snapshot(AudioDirection::Playback).unwrap().closed);
            assert_eq!(submission.snapshot(4).submitted_source_frames, Some(320));
        });
    }

    #[test]
    fn alsa_null_software_hold_keeps_media_and_records_only_actual_silence_until_resume() {
        use alia_core::delivery::VoiceScope;
        use alia_core::playback_control::*;
        let config = AudioConfig {
            playback_sample_rate: 16000,
            playback_channels: 1,
            ..Default::default()
        };
        // This is a real capability query; stage 1 deliberately uses its
        // software gate regardless of null's reported native-pause capability.
        let capabilities = create_alsa_playback_runtime(&config, "null").unwrap();
        eprintln!(
            "ALSA software null can_pause={}; tested mode=SoftwareGate",
            capabilities.pcm.hw_params_current().unwrap().can_pause()
        );
        drop(capabilities);
        let (sender, receiver) = playback::channel(8);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let bus = AudioBus::new(2).reference();
        let mut reader = bus.try_claim_reader().unwrap();
        let reference = bus.clone();
        let task = std::thread::spawn(move || {
            run_alsa_playback_thread(
                config,
                receiver,
                ready_tx,
                "null",
                AudioVolumeState::new(Default::default()),
                reference,
            )
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        let generation = sender.begin().unwrap();
        let scope = VoiceScope {
            device_boot_id: uuid::Uuid::new_v4(),
            session_id: uuid::Uuid::new_v4(),
            generation_epoch: 1,
        };
        let s = sender
            .open_submission(
                PlaybackIdentity {
                    scope,
                    playback_id: uuid::Uuid::new_v4(),
                    playback_sequence: 1,
                },
                16000,
                generation,
            )
            .unwrap();
        let control = sender.playback_control(scope, generation).unwrap();
        control.observe_capture(7, true).unwrap();
        let hold = ControlCommand {
            scope,
            hold_id: uuid::Uuid::new_v4(),
            command_id: uuid::Uuid::new_v4(),
            sequence: 1,
            action: ControlAction::Pause {
                reason: HoldReason::AcousticCandidate,
                through_sequence: 7,
                budget_ms: 3000,
            },
        };
        assert_eq!(
            sender
                .apply_control(&control, &hold)
                .unwrap()
                .snapshot
                .phase,
            ControlPhase::Held
        );
        sender.submit(&s, 0, &[4000; 160], false).unwrap();
        sender.submit(&s, 160, &[], true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut zero_frames = 0;
        while Instant::now() < deadline && zero_frames == 0 {
            if let Some(block) = reader.try_read() {
                if block.content == crate::module::context::ReferenceContent::HeldSilence
                    && block.sample_count > 0
                {
                    assert!(
                        block.samples[..block.sample_count]
                            .iter()
                            .all(|v| *v == 0.0)
                    );
                    zero_frames += block.timing.frames;
                }
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert!(zero_frames > 0, "real null writes must expose held silence");
        assert_eq!(s.snapshot(1).submitted_source_frames, Some(0));
        while reader.try_read().is_some() {}
        let resume = ControlCommand {
            command_id: uuid::Uuid::new_v4(),
            sequence: 2,
            action: ControlAction::Resume {
                authorization: ResumeAuthorization {
                    kind: ResumeKind::FreshBackchannel,
                    detection_id: uuid::Uuid::new_v4(),
                    through_sequence: 7,
                },
            },
            ..hold
        };
        sender.apply_control(&control, &resume).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while s.snapshot(2).observation_end.is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(s.snapshot(3).submitted_source_frames, Some(160));
        assert_eq!(
            s.snapshot(3).observation_end,
            Some(alia_core::delivery::ObservationEnd::SubmissionFinished)
        );
        let mut media_frames = 0;
        while let Some(block) = reader.try_read() {
            if block.content == crate::module::context::ReferenceContent::Media {
                media_frames += block.timing.frames;
            }
        }
        assert!(media_frames <= 160);
        if media_frames < 160 {
            assert!(
                bus.snapshot(AudioDirection::Playback).unwrap().lost_blocks > 0,
                "missing reference must have explicit loss evidence"
            );
        }
        sender.shutdown();
        task.join().unwrap().unwrap();
        assert!(bus.snapshot(AudioDirection::Playback).unwrap().closed);
    }

    // ALSA's explicit software null PCM performs real libasound calls without
    // requiring a sound card. It cannot validate DAC timing or acoustic output.
    #[test]
    fn alsa_null_write_records_the_real_accepted_samples_when_clock_is_unavailable() {
        let config = AudioConfig {
            playback_channels: 1,
            ..Default::default()
        };
        let runtime = create_alsa_playback_runtime(&config, "null").unwrap();
        let bus = AudioBus::new(2).reference();
        let mut reader = bus.try_claim_reader().unwrap();
        let mut clock = bus.open_stream(
            AudioDirection::Playback,
            AudioFormat {
                sample_rate_hz: runtime.output_sample_rate,
                channels: runtime.output_channels as u16,
            },
        );
        let owner = AudioStreamOwner(clock.probe.clone());
        clock.probe.activate();
        reader.try_read();
        let mut buffer = PlaybackBuffer::new(Arc::new(AtomicU64::new(1)));
        buffer.append(1, vec![1234; 64]);
        write_alsa_chunk(
            &runtime.pcm,
            &mut buffer,
            1,
            64,
            &mut clock,
            &mut AlsaTimeOrigin::default(),
            false,
        )
        .unwrap();
        let block = reader
            .try_read()
            .expect("successful native null write must be observable");
        assert_eq!(block.boundary, Some(AudioSampleBoundary::AlsaWriteAccepted));
        assert_eq!(block.timing.frames, 64);
        assert_eq!(block.sample_count, 64);
        assert_eq!(block.timing.clock.quality, AudioTimeQuality::Unknown);
        assert_eq!(&block.samples[..64], &[1234.0 / 32768.0; 64]);
        assert_eq!(buffer.len(), 0);
        drop(runtime);
        drop(owner);
        assert!(!bus.snapshot(AudioDirection::Playback).unwrap().running);
    }

    #[test]
    fn alsa_failed_native_write_never_creates_a_reference_prefix() {
        let config = AudioConfig {
            playback_channels: 1,
            ..Default::default()
        };
        let runtime = create_alsa_playback_runtime(&config, "null").unwrap();
        let bus = AudioBus::new(2).reference();
        let mut reader = bus.try_claim_reader().unwrap();
        let mut clock = bus.open_stream(
            AudioDirection::Playback,
            AudioFormat {
                sample_rate_hz: runtime.output_sample_rate,
                channels: 1,
            },
        );
        clock.probe.activate();
        reader.try_read();
        let mut buffer = PlaybackBuffer::new(Arc::new(AtomicU64::new(1)));
        buffer.append(1, vec![1234; 64]);
        runtime.pcm.drop().unwrap();
        assert!(
            write_alsa_chunk(
                &runtime.pcm,
                &mut buffer,
                1,
                64,
                &mut clock,
                &mut AlsaTimeOrigin::default(),
                true
            )
            .is_err()
        );
        while let Some(block) = reader.try_read() {
            assert_eq!(block.sample_count, 0);
        }
        let state = bus.snapshot(AudioDirection::Playback).unwrap();
        assert_eq!(state.cursor, 0);
        assert_eq!(state.last_fault, Some(AudioDiscontinuity::BackendError));
        assert_eq!(buffer.len(), 64);

        // An empty controlled stream must surface the same real device error,
        // without recording the zero fill it attempted but never submitted.
        buffer.clear();
        buffer.set_control(Some(
            crate::module::playback_session::PlaybackControl::new(
                alia_core::delivery::VoiceScope {
                    device_boot_id: uuid::Uuid::new_v4(),
                    session_id: uuid::Uuid::new_v4(),
                    generation_epoch: 1,
                },
                1,
                SubmissionBackend {
                    instance_id: clock.probe.stream_id(),
                    boundary: SubmissionBoundary::AlsaWriteAccepted,
                    output_sample_rate_hz: runtime.output_sample_rate,
                    output_channels: 1,
                },
            )
            .unwrap(),
        ));
        assert!(
            write_alsa_chunk(
                &runtime.pcm,
                &mut buffer,
                1,
                64,
                &mut clock,
                &mut AlsaTimeOrigin::default(),
                true,
            )
            .is_err()
        );
        while let Some(block) = reader.try_read() {
            assert_eq!(block.sample_count, 0);
        }
        assert_eq!(bus.snapshot(AudioDirection::Playback).unwrap().cursor, 0);
    }

    #[test]
    fn native_start_failure_keeps_the_already_accepted_prefix_and_final_fault() {
        let runtime = create_alsa_playback_runtime(
            &AudioConfig {
                playback_channels: 1,
                ..Default::default()
            },
            "null",
        )
        .unwrap();
        let bus = AudioBus::new(2).reference();
        let mut reader = bus.try_claim_reader().unwrap();
        let mut clock = bus.open_stream(
            AudioDirection::Playback,
            AudioFormat {
                sample_rate_hz: runtime.output_sample_rate,
                channels: 1,
            },
        );
        let owner = AudioStreamOwner(clock.probe.clone());
        clock.probe.activate();
        reader.try_read();
        let mut buffer = PlaybackBuffer::new(Arc::new(AtomicU64::new(1)));
        buffer.append(1, vec![2345; 64]);
        let (chunk, lease) = buffer.alsa_chunk(64);
        let accepted = runtime.pcm.io_i16().unwrap().writei(&chunk).unwrap();
        assert_eq!(accepted, 64);
        let unknown = clock.probe.unknown_clock();
        playback::confirm_alsa_write(&mut buffer, &chunk, accepted, lease, &mut clock, unknown)
            .unwrap();
        // A native state change after acceptance forces the production start
        // helper to fail; the successful write remains a separate known fact.
        runtime.pcm.drop().unwrap();
        assert!(start_accepted_playback(&runtime.pcm, accepted, &clock.probe).is_err());
        clock.probe.mark(AudioDiscontinuity::Cancelled);
        drop(runtime);
        drop(owner);
        let prefix = reader.try_read().unwrap();
        assert_eq!(
            prefix.boundary,
            Some(AudioSampleBoundary::AlsaWriteAccepted)
        );
        assert_eq!(prefix.sample_count, accepted);
        assert_eq!(&prefix.samples[..accepted], &[2345.0 / 32768.0; 64]);
        assert_eq!(buffer.len(), 0);
        let state = bus.snapshot(AudioDirection::Playback).unwrap();
        assert!(state.closed);
        assert_eq!(state.cursor, accepted as u64);
        assert_eq!(state.last_fault, Some(AudioDiscontinuity::BackendError));
    }

    #[test]
    fn capture_wait_failure_keeps_backend_fault_through_native_close() {
        let (pcm, rate, channels, _, _) =
            start_alsa_capture_runtime(&AudioConfig::default(), "null").unwrap();
        let bus = AudioBus::new(2).reference();
        let clock = bus.open_stream(
            AudioDirection::Capture,
            AudioFormat {
                sample_rate_hz: rate,
                channels: channels as u16,
            },
        );
        let owner = AlsaCaptureOwner {
            pcm,
            _clock_owner: AudioStreamOwner(clock.probe.clone()),
        };
        clock.probe.activate();
        owner.pcm.drop().unwrap();
        let mut converter = CaptureConverter::new(channels, rate, 1, 16000);
        // null PCM readiness does not reproduce real device disconnection.
        // Inject the wait result at the production error gate, then close the
        // real native owner. No physical wait-failure claim is made here.
        assert!(
            handle_capture_wait(
                &owner.pcm,
                &mut converter,
                &clock.probe,
                Err(alsa::Error::new("snd_pcm_wait", libc::ENODEV)),
            )
            .is_err()
        );
        drop(owner);
        let state = bus.snapshot(AudioDirection::Capture).unwrap();
        assert!(state.closed);
        assert_eq!(state.cursor, 0);
        assert_eq!(state.last_fault, Some(AudioDiscontinuity::BackendError));
    }

    #[test]
    fn wait_and_read_discontinuities_restart_native_capture_without_old_converter_tail() {
        for function in ["snd_pcm_wait", "snd_pcm_readi"] {
            for errno in [libc::EPIPE, libc::ESTRPIPE] {
                let (pcm, rate, channels, _, _) =
                    start_alsa_capture_runtime(&AudioConfig::default(), "null").unwrap();
                let bus = AudioBus::new(2).reference();
                let clock = bus.open_stream(
                    AudioDirection::Capture,
                    AudioFormat {
                        sample_rate_hz: rate,
                        channels: channels as u16,
                    },
                );
                let owner = AlsaCaptureOwner {
                    pcm,
                    _clock_owner: AudioStreamOwner(clock.probe.clone()),
                };
                clock.probe.activate();
                let mut converter = CaptureConverter::new(1, 48000, 1, 16000);
                converter.convert(&[-20000_i16; 17]);
                owner.pcm.drop().unwrap();
                // The error identity is injected. prepare/start themselves use
                // the real software PCM; this does not simulate hardware xrun.
                recover_capture_error(
                    &owner.pcm,
                    &mut converter,
                    &clock.probe,
                    alsa::Error::new(function, errno),
                )
                .unwrap();
                assert_eq!(owner.pcm.state(), AlsaState::Running);
                let next = converter.convert(&[20000_i16; 23]);
                assert!(!next.is_empty() && next.iter().all(|&x| x == 20000));
                let state = bus.snapshot(AudioDirection::Capture).unwrap();
                assert_eq!(state.last_fault, Some(capture_error_reason(errno)));
                assert_eq!(state.cursor, 0);
                let continuity = state.continuity;
                for retry in [libc::EAGAIN, libc::EINTR] {
                    recover_capture_error(
                        &owner.pcm,
                        &mut converter,
                        &clock.probe,
                        alsa::Error::new(function, retry),
                    )
                    .unwrap();
                    assert_eq!(clock.probe.continuity(), continuity);
                }
                drop(owner);
                assert!(bus.snapshot(AudioDirection::Capture).unwrap().closed);
            }
        }
    }

    #[test]
    fn null_capture_stop_joins_native_owner_and_preserves_independent_timing() {
        let bus = AudioBus::new(8);
        let reference = bus.reference();
        let mut receiver = bus.subscribe();
        let (control_tx, control_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            run_alsa_capture_thread(
                AudioConfig::default(),
                bus,
                control_rx,
                ready_tx,
                "null",
                AudioVolumeState::new(Default::default()),
            )
        });
        let ready = ready_rx.recv_timeout(std::time::Duration::from_secs(2));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let frame = loop {
            match receiver.try_recv() {
                Ok(frame) => break Some(frame),
                // null PCM has no hardware pacing. Catch up immediately rather
                // than sleeping after every lag and continuously falling behind.
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))
                    if std::time::Instant::now() < deadline =>
                {
                    continue;
                }
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                }
                Err(_) => break None,
            }
        };
        control_tx.send(()).unwrap();
        let result = worker.join().unwrap();
        ready.unwrap().unwrap();
        result.unwrap();
        let timing = frame
            .expect("real null capture must publish a frame")
            .timing
            .unwrap();
        assert_eq!(timing.native.stream.direction, AudioDirection::Capture);
        assert_eq!(timing.native.clock.sample_ns, None);
        assert!(timing.native.frames > 0 && timing.converted_frames > 0);
        let state = reference.snapshot(AudioDirection::Capture).unwrap();
        assert!(state.closed && !state.running);
        assert_eq!(state.stream, timing.native.stream);
        assert!(state.cursor >= timing.native.start_frame + timing.native.frames);
        assert!(reference.snapshot(AudioDirection::Playback).is_none());
    }
}
