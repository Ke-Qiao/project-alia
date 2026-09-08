use super::*;

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
    let mut emitted_frames = 0_u64;
    let mut level_peak = 0_u64;
    let mut silence_warned = false;
    let mut clipping_warned = false;
    let mut detected_s32_shift = alsa_capture_s32_shift_override(&config);
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
) -> Result<()> {
    let runtime = match create_alsa_playback_runtime(&config, alsa_device) {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready_tx.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let target_buffer_ms = playback_target_buffer_ms(&config);
    receiver.set_backend(SubmissionBackend {
        instance_id: uuid::Uuid::new_v4(),
        boundary: SubmissionBoundary::AlsaWriteAccepted,
        output_sample_rate_hz: runtime.output_sample_rate,
        output_channels: runtime.output_channels as u32,
    });
    let mut sink = AlsaPlaybackSink {
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
    buffer: PlaybackBuffer,
    renderer: Renderer,
    volume: AudioVolumeState,
}

impl PlaybackSink for AlsaPlaybackSink {
    fn buffered_ms(&self) -> u64 {
        playback_buffered_ms(
            self.buffer.len(),
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
        self.renderer.clear();
        self.buffer.clear();
        self.runtime.pcm.drop().context("stopping ALSA playback")?;
        self.runtime
            .pcm
            .prepare()
            .context("preparing ALSA after stop")?;
        Ok(())
    }

    fn pump(&mut self) -> Result<()> {
        if self.buffer.len() != 0 {
            write_alsa_chunk(
                &self.runtime.pcm,
                &mut self.buffer,
                self.runtime.output_channels,
                self.runtime.chunk_frames,
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
    Ok(AlsaPlaybackRuntime {
        pcm,
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
) -> Result<()> {
    let channels = channels.max(1);
    let available_frames = buffer.len() / channels;
    if available_frames == 0 {
        return Ok(());
    }

    let frames_to_write = available_frames.min(chunk_frames.max(1));
    let (chunk, output_write) = buffer.alsa_chunk(frames_to_write);
    if chunk.is_empty() {
        return Ok(());
    }
    let io = pcm.io_i16().context("opening ALSA i16 playback IO")?;

    match io.writei(&chunk) {
        Ok(written_frames) => {
            buffer.alsa_submitted(written_frames);
            if let Some(lease) = output_write {
                lease.finish(written_frames as u64);
            }
            if pcm.state() != AlsaState::Running {
                let _ = pcm.start();
            }
            Ok(())
        }
        Err(error) if matches!(error.errno(), libc::EAGAIN | libc::EINTR) => Ok(()),
        Err(error) if error.errno() == libc::EPIPE => {
            pcm.prepare().context("recovering ALSA playback underrun")
        }
        Err(error) if error.errno() == libc::ESTRPIPE => match pcm.resume() {
            Ok(()) => Ok(()),
            Err(error) if error.errno() == libc::EAGAIN => Ok(()),
            Err(_) => pcm.prepare().context("recovering suspended ALSA playback"),
        },
        Err(error) => Err(error).context("writing ALSA playback audio"),
    }
}

pub(super) fn alsa_capture_device_available(device_name: &str) -> bool {
    AlsaPcm::new(device_name, AlsaDirection::Capture, true).is_ok()
}

pub(super) fn alsa_playback_device_available(device_name: &str) -> bool {
    AlsaPcm::new(device_name, AlsaDirection::Playback, true).is_ok()
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
