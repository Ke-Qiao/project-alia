use super::*;

pub(super) fn start_cpal_capture_runtime(
    config: &AudioConfig,
    bus: AudioBus,
    volume: AudioVolumeState,
) -> Result<Stream> {
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
            volume,
            err_fn,
        )?,
        SampleFormat::I16 => build_input_stream::<i16>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            err_fn,
        )?,
        SampleFormat::I32 => build_input_stream::<i32>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            err_fn,
        )?,
        SampleFormat::I64 => build_input_stream::<i64>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            err_fn,
        )?,
        SampleFormat::U8 => build_input_stream::<u8>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            err_fn,
        )?,
        SampleFormat::U16 => build_input_stream::<u16>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            err_fn,
        )?,
        SampleFormat::U32 => build_input_stream::<u32>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            err_fn,
        )?,
        SampleFormat::U64 => build_input_stream::<u64>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            err_fn,
        )?,
        SampleFormat::F32 => build_input_stream::<f32>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
            err_fn,
        )?,
        SampleFormat::F64 => build_input_stream::<f64>(
            &device,
            &stream_config,
            converter,
            bus,
            emitted_frames,
            volume,
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
    Ok(stream)
}

pub(super) fn start_cpal_playback_runtime(
    config: &AudioConfig,
    generation: Arc<AtomicU64>,
    volume: AudioVolumeState,
    device_failed: impl Fn() + Send + 'static,
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
    let err_fn = move |error| {
        device_failed();
        warn!(%error, "cpal output stream error; closing playback runtime");
    };

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
    Ok(CpalPlaybackSink {
        _stream: stream,
        buffer,
        sample_rate,
        channels,
        max_buffer_ms: playback_max_buffer_ms(config),
        volume,
    })
}

pub(super) struct CpalPlaybackSink {
    _stream: Stream,
    buffer: Arc<Mutex<PlaybackBuffer>>,
    sample_rate: u32,
    channels: usize,
    max_buffer_ms: u64,
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
        let mut rendered = block.render(self.sample_rate, self.channels);
        apply_gain_in_place(&mut rendered, self.volume.playback_volume());
        let mut buffer = lock_mutex(&self.buffer, "playback buffer")?;
        if buffer.append(block.generation, rendered) {
            clamp_playback_buffer(
                &mut buffer.samples,
                self.sample_rate,
                self.channels,
                self.max_buffer_ms,
            );
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
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
                let mut pcm = match converter.lock() {
                    Ok(mut converter) => converter.convert(data),
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
    err_fn: impl Fn(cpal::StreamError) + Send + 'static,
) -> Result<Stream>
where
    T: FromI16Sample + cpal::SizedSample + Send + 'static,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| match buffer.try_lock() {
                Ok(mut buffer) => {
                    for output in data {
                        let sample = buffer.next_sample();
                        *output = T::from_i16_sample(sample);
                    }
                }
                Err(_) => {
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
