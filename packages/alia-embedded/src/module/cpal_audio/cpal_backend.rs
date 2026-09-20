use super::super::context::{AudioStreamClock, AudioStreamOwner, ReferenceContent};
use super::clock::{CpalTimeOrigin, NativeReferenceSample, record_samples};
use super::*;
use alia_core::audio_clock::*;

pub(super) struct CpalCaptureRuntime {
    _stream: Stream,
    _clock_owner: AudioStreamOwner,
}

pub(super) fn start_cpal_capture_runtime(
    config: &AudioConfig,
    bus: AudioBus,
    volume: AudioVolumeState,
) -> Result<CpalCaptureRuntime> {
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
    let clock = bus.reference().open_stream(
        AudioDirection::Capture,
        AudioFormat {
            sample_rate_hz: input_sample_rate,
            channels: input_channels as u16,
        },
    );
    let clock_owner = AudioStreamOwner(clock.probe.clone());
    let failed = clock.probe.clone();
    let err_fn = move |error| {
        failed.mark(AudioDiscontinuity::BackendError);
        warn!(%error, "cpal input stream error");
    };

    let stream = match selected.sample_format() {
        SampleFormat::I8 => build_input_stream::<i8>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            clock,
            err_fn,
        )?,
        SampleFormat::I16 => build_input_stream::<i16>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            clock,
            err_fn,
        )?,
        SampleFormat::I32 => build_input_stream::<i32>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            clock,
            err_fn,
        )?,
        SampleFormat::I64 => build_input_stream::<i64>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            clock,
            err_fn,
        )?,
        SampleFormat::U8 => build_input_stream::<u8>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            clock,
            err_fn,
        )?,
        SampleFormat::U16 => build_input_stream::<u16>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            clock,
            err_fn,
        )?,
        SampleFormat::U32 => build_input_stream::<u32>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            clock,
            err_fn,
        )?,
        SampleFormat::U64 => build_input_stream::<u64>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            clock,
            err_fn,
        )?,
        SampleFormat::F32 => build_input_stream::<f32>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            clock,
            err_fn,
        )?,
        SampleFormat::F64 => build_input_stream::<f64>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            clock,
            err_fn,
        )?,
        _ => bail!(
            "unsupported input sample format: {:?}",
            selected.sample_format()
        ),
    };

    stream.play().context("starting cpal input stream")?;
    clock_owner.0.activate();
    info!(
        device = %device_name(&device),
        input_sample_rate,
        input_channels,
        target_sample_rate,
        target_channels,
        "started cpal audio capture"
    );
    Ok(CpalCaptureRuntime {
        _stream: stream,
        _clock_owner: clock_owner,
    })
}

pub(super) fn start_cpal_playback_runtime(
    config: &AudioConfig,
    generation: Arc<AtomicU64>,
    volume: AudioVolumeState,
    device_failed: impl Fn() + Send + 'static,
    reference: AudioReferenceBus,
) -> Result<CpalPlaybackSink> {
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
    let buffer = Arc::new(Mutex::new(PlaybackBuffer::new(generation)));
    let clock = reference.open_stream(
        AudioDirection::Playback,
        AudioFormat {
            sample_rate_hz: sample_rate,
            channels: channels as u16,
        },
    );
    let clock_owner = AudioStreamOwner(clock.probe.clone());
    let probe = clock.probe.clone();
    let failed = probe.clone();
    let err_fn = move |error| {
        failed.mark(AudioDiscontinuity::BackendError);
        device_failed();
        warn!(%error, "cpal output stream error; closing playback runtime");
    };

    let stream = match selected.sample_format() {
        SampleFormat::I8 => {
            build_output_stream::<i8>(&device, &stream_config, Arc::clone(&buffer), clock, err_fn)?
        }
        SampleFormat::I16 => {
            build_output_stream::<i16>(&device, &stream_config, Arc::clone(&buffer), clock, err_fn)?
        }
        SampleFormat::I32 => {
            build_output_stream::<i32>(&device, &stream_config, Arc::clone(&buffer), clock, err_fn)?
        }
        SampleFormat::I64 => {
            build_output_stream::<i64>(&device, &stream_config, Arc::clone(&buffer), clock, err_fn)?
        }
        SampleFormat::U8 => {
            build_output_stream::<u8>(&device, &stream_config, Arc::clone(&buffer), clock, err_fn)?
        }
        SampleFormat::U16 => {
            build_output_stream::<u16>(&device, &stream_config, Arc::clone(&buffer), clock, err_fn)?
        }
        SampleFormat::U32 => {
            build_output_stream::<u32>(&device, &stream_config, Arc::clone(&buffer), clock, err_fn)?
        }
        SampleFormat::U64 => {
            build_output_stream::<u64>(&device, &stream_config, Arc::clone(&buffer), clock, err_fn)?
        }
        SampleFormat::F32 => {
            build_output_stream::<f32>(&device, &stream_config, Arc::clone(&buffer), clock, err_fn)?
        }
        SampleFormat::F64 => {
            build_output_stream::<f64>(&device, &stream_config, Arc::clone(&buffer), clock, err_fn)?
        }
        _ => bail!(
            "unsupported output sample format: {:?}",
            selected.sample_format()
        ),
    };

    stream.play().context("starting cpal output stream")?;
    clock_owner.0.activate();
    info!(
        device = %device_name(&device),
        sample_rate,
        channels,
        "started cpal audio playback"
    );
    Ok(CpalPlaybackSink {
        _stream: stream,
        _clock_owner: clock_owner,
        probe: probe.clone(),
        buffer,
        sample_rate,
        channels,
        backend: SubmissionBackend {
            instance_id: probe.stream_id(),
            boundary: SubmissionBoundary::CpalCallbackBuffer,
            output_sample_rate_hz: sample_rate,
            output_channels: channels as u32,
        },
        renderer: Renderer::default(),
        volume,
    })
}

pub(super) struct CpalPlaybackSink {
    _stream: Stream,
    _clock_owner: AudioStreamOwner,
    probe: super::super::context::AudioStreamProbe,
    buffer: Arc<Mutex<PlaybackBuffer>>,
    sample_rate: u32,
    channels: usize,
    pub(super) backend: SubmissionBackend,
    renderer: Renderer,
    volume: AudioVolumeState,
}

impl PlaybackSink for CpalPlaybackSink {
    fn buffered_ms(&self) -> u64 {
        let buffer = self
            .buffer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        playback_buffered_ms(buffer.len(), self.sample_rate, self.channels)
    }

    fn enqueue(&mut self, block: AudioBlock) -> Result<()> {
        let mut rendered = self
            .renderer
            .render(block, self.sample_rate, self.channels)?;
        apply_gain_in_place(&mut rendered.samples, self.volume.playback_volume());
        let mut buffer = lock_mutex(&self.buffer, "playback buffer")?;
        buffer.append_rendered(rendered)?;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.probe.mark(AudioDiscontinuity::Cancelled);
        self.renderer.clear();
        lock_mutex(&self.buffer, "playback buffer")?.clear();
        Ok(())
    }

    fn pump(&mut self) -> Result<()> {
        // The CPAL callback consumes samples independently.
        Ok(())
    }
}

fn build_input_stream<T>(
    device: &Device,
    config: &StreamConfig,
    converter: Arc<Mutex<CaptureConverter>>,
    bus: AudioBus,
    emitted_frames: Arc<AtomicU64>,
    volume: AudioVolumeState,
    mut clock: AudioStreamClock,
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
    let mut time_origin = CpalTimeOrigin::default();
    let mut continuity = clock.probe.continuity();
    device
        .build_input_stream(
            config,
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                let timestamp = info.timestamp();
                let reading = time_origin.read(&clock.probe, timestamp.callback, timestamp.capture);
                let native_format = clock.probe.format();
                let complete = data.len() / usize::from(native_format.channels) * usize::from(native_format.channels);
                if complete != data.len() { clock.probe.mark(AudioDiscontinuity::BackendError); }
                let native = clock.observe((complete / usize::from(native_format.channels)) as u64, reading);
                let mut pcm = match converter.lock() {
                    Ok(mut converter) => {
                        if continuity != native.continuity {
                            converter.reset();
                            continuity = native.continuity;
                        }
                        converter.convert(&data[..complete])
                    },
                    Err(error) => {
                        warn!(%error, "capture converter lock poisoned");
                        return;
                    }
                };
                if pcm.is_empty() {
                    return;
                }
                apply_gain_in_place(&mut pcm, volume.capture_volume());
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
                    timing: Some(CaptureTiming {
                        native, converted_format: AudioFormat { sample_rate_hz: sample_rate, channels: channels as u16 },
                        converted_start_frame: start_frame, converted_frames: frames,
                        resampled: native_format.sample_rate_hz != sample_rate,
                    }),
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
    buffer: Arc<Mutex<PlaybackBuffer>>,
    mut clock: AudioStreamClock,
    err_fn: impl Fn(cpal::StreamError) + Send + 'static,
) -> Result<Stream>
where
    T: FromI16Sample + NativeReferenceSample + cpal::SizedSample + Send + 'static,
{
    let mut time_origin = CpalTimeOrigin::default();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let timestamp = info.timestamp();
                let reading =
                    time_origin.read(&clock.probe, timestamp.callback, timestamp.playback);
                fill_output_buffer(data, &buffer, &mut clock, reading);
            },
            err_fn,
            None,
        )
        .context("building cpal output stream")
}

// Production callback seam: tests exercise the same conversion, output writes,
// submission gates and reference publication without claiming native hardware.
pub(super) fn fill_output_buffer<T: FromI16Sample + NativeReferenceSample>(
    data: &mut [T],
    buffer: &Mutex<PlaybackBuffer>,
    clock: &mut AudioStreamClock,
    reading: AudioClockReading,
) {
    if data.len() % usize::from(clock.probe.format().channels) != 0 {
        for output in data.iter_mut() {
            *output = T::from_i16_sample(0);
        }
        clock.probe.mark(AudioDiscontinuity::BackendError);
        record_samples(
            clock,
            data,
            reading,
            SubmissionBoundary::CpalCallbackBuffer,
            ReferenceContent::ZeroFill,
        );
        return;
    }
    let mut media = 0;
    match buffer.try_lock() {
        Ok(mut buffer) => {
            for output in data.iter_mut() {
                if buffer.write_next(|sample| *output = T::from_i16_sample(sample)) {
                    media += 1;
                }
            }
        }
        Err(_) => {
            clock
                .probe
                .mark(AudioDiscontinuity::CallbackBufferUnavailable);
            for output in data.iter_mut() {
                *output = T::from_i16_sample(0);
            }
        }
    }
    let content = if media == 0 {
        ReferenceContent::ZeroFill
    } else if media == data.len() {
        ReferenceContent::Media
    } else {
        ReferenceContent::Mixed
    };
    record_samples(
        clock,
        data,
        reading,
        SubmissionBoundary::CpalCallbackBuffer,
        content,
    );
}
