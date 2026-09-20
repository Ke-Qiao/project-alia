use alia_core::audio_clock::*;
use alia_core::delivery::SubmissionBoundary;
use cpal::StreamInstant;

use super::super::context::{
    AudioReferenceBlock, AudioStreamClock, AudioStreamProbe, REFERENCE_BLOCK_SAMPLES,
    ReferenceContent,
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

/// f64 retains the exact floating value delivered to a floating point backend.
/// Integer output originates in i16, so its normalized shifted value is exact.
pub(super) trait NativeReferenceSample: Copy {
    fn normalized(self) -> f64;
}
macro_rules! signed {
    ($($t:ty => $scale:expr),*) => { $(impl NativeReferenceSample for $t {
        fn normalized(self) -> f64 { self as f64 / $scale }
    })* };
}
signed!(i8 => 128.0, i16 => 32768.0, i32 => 2147483648.0, i64 => 9223372036854775808.0);
macro_rules! unsigned {
    ($($t:ty => $scale:expr),*) => { $(impl NativeReferenceSample for $t {
        fn normalized(self) -> f64 { self as f64 / $scale - 1.0 }
    })* };
}
unsigned!(u8 => 128.0, u16 => 32768.0, u32 => 2147483648.0, u64 => 9223372036854775808.0);
impl NativeReferenceSample for f32 {
    fn normalized(self) -> f64 {
        f64::from(self)
    }
}
impl NativeReferenceSample for f64 {
    fn normalized(self) -> f64 {
        self
    }
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
            boundary: Some(boundary),
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
