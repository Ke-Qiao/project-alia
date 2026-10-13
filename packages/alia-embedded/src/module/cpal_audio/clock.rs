use alia_core::audio_clock::*;
use alia_core::delivery::SubmissionBoundary;
use cpal::StreamInstant;

use super::super::context::{
    AudioReferenceBlock, AudioSampleBoundary, AudioStreamClock, AudioStreamProbe,
    NativeSampleFormat, REFERENCE_BLOCK_SAMPLES, ReferenceContent,
};

#[derive(Default)]
pub(super) struct CpalTimeOrigin(Option<StreamInstant>);
impl CpalTimeOrigin {
    pub fn read(
        &mut self,
        probe: &AudioStreamProbe,
        callback: StreamInstant,
        sample: StreamInstant,
    ) -> AudioClockReading {
        let origin = *self.0.get_or_insert(callback);
        let offset = |instant: StreamInstant| -> Option<i64> {
            if let Some(duration) = instant.duration_since(&origin) {
                i64::try_from(duration.as_nanos()).ok()
            } else {
                origin
                    .duration_since(&instant)
                    .and_then(|d| i64::try_from(d.as_nanos()).ok())
                    .and_then(i64::checked_neg)
            }
        };
        let callback_ns = offset(callback);
        let sample_ns = offset(sample);
        AudioClockReading {
            clock_id: probe.stream_id(),
            callback_ns,
            sample_ns,
            queued_frames: None,
            quality: if callback_ns.is_some() && sample_ns.is_some() {
                AudioTimeQuality::BackendEstimate
            } else {
                AudioTimeQuality::Unknown
            },
        }
    }
}

/// Normalized observations at the native boundary, before capture conversion.
/// Wide integer capture containers may lose low bits when represented as f64;
/// the source container is retained separately and is not a measured ADC depth.
pub(super) trait NativeReferenceSample: Copy {
    const FORMAT: NativeSampleFormat;
    fn normalized(self) -> f64;
}
macro_rules! signed {
    ($($t:ty => $format:ident, $scale:expr);*) => { $(impl NativeReferenceSample for $t {
        const FORMAT: NativeSampleFormat = NativeSampleFormat::$format;
        fn normalized(self) -> f64 { self as f64 / $scale }
    })* };
}
signed!(i8 => I8, 128.0; i16 => I16, 32768.0; i32 => I32, 2147483648.0; i64 => I64, 9223372036854775808.0);
macro_rules! unsigned {
    ($($t:ty => $format:ident, $scale:expr);*) => { $(impl NativeReferenceSample for $t {
        const FORMAT: NativeSampleFormat = NativeSampleFormat::$format;
        fn normalized(self) -> f64 { self as f64 / $scale - 1.0 }
    })* };
}
unsigned!(u8 => U8, 128.0; u16 => U16, 32768.0; u32 => U32, 2147483648.0; u64 => U64, 9223372036854775808.0);
impl NativeReferenceSample for f32 {
    const FORMAT: NativeSampleFormat = NativeSampleFormat::F32;
    fn normalized(self) -> f64 {
        f64::from(self)
    }
}
impl NativeReferenceSample for f64 {
    const FORMAT: NativeSampleFormat = NativeSampleFormat::F64;
    fn normalized(self) -> f64 {
        self
    }
}

/// Observe the complete native batch once for CaptureTiming, then copy it into
/// fixed, frame-aligned evidence blocks without advancing the clock again.
pub(super) fn observe_capture_samples<T: NativeReferenceSample>(
    clock: &mut AudioStreamClock,
    samples: &[T],
    reading: AudioClockReading,
    boundary: AudioSampleBoundary,
) -> AudioTiming {
    let format = clock.probe.format();
    let channels = usize::from(format.channels);
    let complete = samples.len() / channels * channels;
    if complete != samples.len() {
        clock.probe.mark(AudioDiscontinuity::BackendError);
    }
    let native = clock.observe((complete / channels) as u64, reading);
    let block_samples = REFERENCE_BLOCK_SAMPLES / channels * channels;
    let mut offset_frames = 0_u64;
    for chunk in samples[..complete].chunks(block_samples) {
        let mut timing = native;
        timing.start_frame += offset_frames;
        timing.frames = (chunk.len() / channels) as u64;
        timing.clock.sample_ns = native.clock.sample_ns.and_then(|ns| {
            let offset =
                u128::from(offset_frames) * 1_000_000_000 / u128::from(format.sample_rate_hz);
            ns.checked_add(i64::try_from(offset).ok()?)
        });
        let mut block = AudioReferenceBlock {
            timing,
            boundary: Some(boundary),
            sample_format: Some(T::FORMAT),
            content: ReferenceContent::Media,
            discontinuity: None,
            sequence: 0,
            lost_blocks_before: 0,
            lost_frames_before: 0,
            samples: [0.0; REFERENCE_BLOCK_SAMPLES],
            sample_count: chunk.len(),
        };
        for (output, sample) in block.samples.iter_mut().zip(chunk) {
            *output = sample.normalized();
        }
        clock.probe.publish(block);
        offset_frames += timing.frames;
    }
    native
}

pub(super) fn record_samples<T: NativeReferenceSample>(
    clock: &mut AudioStreamClock,
    samples: &[T],
    reading: AudioClockReading,
    boundary: SubmissionBoundary,
    content: ReferenceContent,
) {
    let format = clock.probe.format();
    let channels = usize::from(format.channels);
    let complete = samples.len() / channels * channels;
    if complete != samples.len() {
        clock.probe.mark(AudioDiscontinuity::BackendError);
    }
    let block_samples = REFERENCE_BLOCK_SAMPLES / channels * channels;
    let mut offset_frames = 0_u64;
    for chunk in samples[..complete].chunks(block_samples) {
        let frames = (chunk.len() / channels) as u64;
        let mut chunk_reading = reading;
        chunk_reading.sample_ns = reading.sample_ns.and_then(|ns| {
            let offset =
                u128::from(offset_frames) * 1_000_000_000 / u128::from(format.sample_rate_hz);
            ns.checked_add(i64::try_from(offset).ok()?)
        });
        let timing = clock.observe(frames, chunk_reading);
        let mut block = AudioReferenceBlock {
            timing,
            boundary: Some(boundary.into()),
            sample_format: Some(T::FORMAT),
            content,
            discontinuity: None,
            sequence: 0,
            lost_blocks_before: 0,
            lost_frames_before: 0,
            samples: [0.0; REFERENCE_BLOCK_SAMPLES],
            sample_count: chunk.len(),
        };
        for (output, sample) in block.samples.iter_mut().zip(chunk) {
            *output = sample.normalized();
        }
        clock.probe.publish(block);
        offset_frames += frames;
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Default)]
pub(super) struct AlsaTimeOrigin(Option<i128>);
#[cfg(any(target_os = "linux", test))]
impl AlsaTimeOrigin {
    pub fn read(
        &mut self,
        probe: &AudioStreamProbe,
        timestamp_ns: Option<i128>,
        delay: Option<i64>,
    ) -> AudioClockReading {
        let callback_ns = timestamp_ns.and_then(|timestamp| {
            let origin = *self.0.get_or_insert(timestamp);
            i64::try_from(timestamp.checked_sub(origin)?).ok()
        });
        AudioClockReading {
            clock_id: probe.stream_id(),
            callback_ns,
            // htstamp + delay are observations. They do not establish the time
            // of a particular accepted sample or a calibrated capture clock.
            sample_ns: None,
            queued_frames: delay,
            quality: if callback_ns.is_some() {
                AudioTimeQuality::BackendEstimate
            } else {
                AudioTimeQuality::Unknown
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_capture_entry_publishes_actual_pcm_in_the_bounded_dual_stream_bus() {
        let bus = crate::module::context::AudioBus::new(2).reference();
        let mut reader = bus.try_claim_reader().unwrap();
        let mut clock = bus.open_stream(
            AudioDirection::Capture,
            AudioFormat {
                sample_rate_hz: 48000,
                channels: 2,
            },
        );
        let samples = [0_i16, 16384, -16384, 8192, -32768, 32767];
        let reading = clock.probe.unknown_clock();
        let native = observe_capture_samples(
            &mut clock,
            &samples,
            reading,
            AudioSampleBoundary::CpalInputCallback,
        );
        clock.probe.close();
        let mut blocks = Vec::new();
        while let Some(block) = reader.try_read() {
            if block.sample_count > 0 {
                blocks.push(block);
            }
        }
        assert_eq!(native.frames, 3);
        assert_eq!(bus.snapshot(AudioDirection::Capture).unwrap().cursor, 3);
        assert_eq!(
            blocks.len(),
            1,
            "native capture PCM was lost before the evidence consumer"
        );
        assert_eq!(blocks[0].timing, native);
        assert_eq!(blocks[0].sample_format, Some(NativeSampleFormat::I16));
        assert_eq!(
            blocks[0].boundary,
            Some(AudioSampleBoundary::CpalInputCallback)
        );
        assert_eq!(blocks[0].sample_count, samples.len());
        assert_eq!(blocks[0].samples[1], 0.5);
        assert_eq!(blocks[0].samples[2], -0.5);
    }

    #[test]
    fn capture_evidence_split_preserves_one_native_batch_and_clock_offsets() {
        let bus = crate::module::context::AudioBus::new(2).reference();
        let mut reader = bus.try_claim_reader().unwrap();
        let mut clock = bus.open_stream(
            AudioDirection::Capture,
            AudioFormat {
                sample_rate_hz: 48000,
                channels: 2,
            },
        );
        let samples: Vec<i16> = (0..2052).map(|i| i as i16 - 1026).collect();
        let reading = AudioClockReading {
            clock_id: clock.probe.stream_id(),
            callback_ns: Some(50_000_000),
            sample_ns: Some(20_000_000),
            queued_frames: None,
            quality: AudioTimeQuality::BackendEstimate,
        };
        let native = observe_capture_samples(
            &mut clock,
            &samples,
            reading,
            AudioSampleBoundary::CpalInputCallback,
        );
        let mut blocks = Vec::new();
        while let Some(block) = reader.try_read() {
            if block.sample_count > 0 {
                blocks.push(block);
            }
        }
        assert_eq!((native.start_frame, native.frames), (0, 1026));
        assert_eq!(native.clock, reading);
        assert_eq!(bus.snapshot(AudioDirection::Capture).unwrap().cursor, 1026);
        assert_eq!(
            blocks
                .iter()
                .map(|b| (b.timing.start_frame, b.timing.frames))
                .collect::<Vec<_>>(),
            [(0, 512), (512, 512), (1024, 2)]
        );
        let reconstructed: Vec<_> = blocks
            .iter()
            .flat_map(|b| b.samples[..b.sample_count].iter().copied())
            .collect();
        assert_eq!(
            reconstructed,
            samples
                .iter()
                .map(|&s| f64::from(s) / 32768.0)
                .collect::<Vec<_>>()
        );
        for block in &blocks {
            assert_eq!(block.timing.observed_boot_ns, native.observed_boot_ns);
            assert_eq!(block.timing.continuity, native.continuity);
            assert_eq!(block.timing.clock.callback_ns, reading.callback_ns);
            assert_eq!(block.timing.clock.queued_frames, None);
            assert_eq!(
                block.timing.clock.sample_ns,
                Some(20_000_000 + (block.timing.start_frame * 1_000_000_000 / 48000) as i64)
            );
            assert_eq!(block.sample_format, Some(NativeSampleFormat::I16));
        }
    }

    #[test]
    fn native_capture_keeps_s32_before_conversion_and_nonfinite_float_evidence() {
        let bus = crate::module::context::AudioBus::new(2).reference();
        let mut reader = bus.try_claim_reader().unwrap();
        let mut clock = bus.open_stream(
            AudioDirection::Capture,
            AudioFormat {
                sample_rate_hz: 16000,
                channels: 1,
            },
        );
        let reading = clock.probe.unknown_clock();
        observe_capture_samples(
            &mut clock,
            &[65537_i32, -65537],
            reading,
            AudioSampleBoundary::AlsaCaptureRead,
        );
        reader.try_read().unwrap(); // Started contains no samples.
        let raw = reader.try_read().unwrap();
        assert_eq!(raw.sample_format, Some(NativeSampleFormat::I32));
        assert_eq!(raw.boundary, Some(AudioSampleBoundary::AlsaCaptureRead));
        assert_eq!(
            &raw.samples[..2],
            &[65537.0 / 2147483648.0, -65537.0 / 2147483648.0]
        );
        assert_eq!(raw.timing.clock.sample_ns, None);
        assert_eq!(raw.timing.clock.queued_frames, None);
        assert_eq!(raw.timing.clock.quality, AudioTimeQuality::Unknown);
        clock.probe.close();
        assert_eq!(
            reader.try_read().unwrap().discontinuity,
            Some(AudioDiscontinuity::StreamStopped)
        );

        // Backends can deliver nonfinite floats. The evidence consumer must
        // reject them explicitly, rather than the producer inventing silence.
        let mut floating = bus.open_stream(
            AudioDirection::Capture,
            AudioFormat {
                sample_rate_hz: 48000,
                channels: 1,
            },
        );
        let reading = floating.probe.unknown_clock();
        observe_capture_samples(
            &mut floating,
            &[f64::NAN, f64::INFINITY, -0.0],
            reading,
            AudioSampleBoundary::CpalInputCallback,
        );
        reader.try_read().unwrap();
        let raw = reader.try_read().unwrap();
        assert_eq!(raw.sample_format, Some(NativeSampleFormat::F64));
        assert!(raw.samples[0].is_nan());
        assert_eq!(raw.samples[1], f64::INFINITY);
        assert_eq!(raw.samples[2].to_bits(), (-0.0_f64).to_bits());
    }
    #[test]
    fn alsa_clock_loss_does_not_create_a_timestamp_or_reset_the_old_origin() {
        let bus = crate::module::context::AudioBus::new(2).reference();
        let mut clock = bus.open_stream(
            AudioDirection::Capture,
            AudioFormat {
                sample_rate_hz: 48000,
                channels: 1,
            },
        );
        let mut origin = AlsaTimeOrigin::default();
        let first = origin.read(&clock.probe, Some(4_000_000_000), Some(240));
        assert_eq!(first.callback_ns, Some(0));
        assert_eq!(first.sample_ns, None);
        let missing = origin.read(&clock.probe, None, None);
        assert_eq!(missing.quality, AudioTimeQuality::Unknown);
        assert_eq!(missing.callback_ns, None);
        assert_eq!(clock.observe(100, missing).frames, 100);
        let later = origin.read(&clock.probe, Some(4_002_000_000), Some(40));
        assert_eq!(later.callback_ns, Some(2_000_000));
        assert_eq!(later.queued_frames, Some(40));
        assert_eq!(later.sample_ns, None);
        assert_eq!(clock.observe(100, later).start_frame, 100);
    }
    #[test]
    fn capture_conversion_reset_cannot_blend_samples_from_before_a_native_gap() {
        let mut converter = super::super::CaptureConverter::new(1, 48000, 1, 16000);
        converter.convert(&[-20000_i16; 17]);
        converter.reset();
        let next = converter.convert(&[20000_i16; 23]);
        let mut fresh = super::super::CaptureConverter::new(1, 48000, 1, 16000);
        assert_eq!(next, fresh.convert(&[20000_i16; 23]));
        assert!(!next.is_empty());
        assert!(next.iter().all(|&x| x == 20000));
    }
}
