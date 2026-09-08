//! One fixed-ratio, anti-aliased timeline per playback. Network packet and
//! hardware callback boundaries never reset its phase or filter history.
use anyhow::{Result, ensure};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, WindowFunction};

pub(super) struct PcmResampler {
    input_rate: u32,
    output_rate: u32,
    filter: Option<Async<f64>>,
    pending: Vec<f64>,
    scratch: Vec<f64>,
    chunk: usize,
    trim_remaining: usize,
    input_frames: u64,
    output_frames: u64,
    ended: bool,
}

pub(super) fn output_frames(source_frames: u64, input_rate: u32, output_rate: u32) -> u64 {
    ((u128::from(source_frames) * u128::from(output_rate)).div_ceil(u128::from(input_rate)))
        .try_into()
        .expect("validated playback length")
}

impl PcmResampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Result<Self> {
        ensure!(
            (8000..=192000).contains(&input_rate),
            "invalid input sample rate"
        );
        ensure!(
            (8000..=384000).contains(&output_rate),
            "invalid output sample rate"
        );
        let chunk = (input_rate as usize / 100).max(1);
        let filter = if input_rate == output_rate {
            None
        } else {
            // Keep the transition width in output-frequency coordinates under
            // large downsampling ratios. A fixed 256-tap input-domain filter
            // would lose rejection at e.g. 192 kHz -> 8 kHz.
            let taps = (256_u64 * u64::from(input_rate))
                .div_ceil(u64::from(output_rate))
                .max(256)
                .div_ceil(8)
                * 8;
            let params =
                SincInterpolationParameters::new(taps as usize, WindowFunction::BlackmanHarris2)
                    .f_cutoff(0.9);
            Some(Async::<f64>::new_sinc(
                f64::from(output_rate) / f64::from(input_rate),
                1.0,
                &params,
                chunk,
                1,
                FixedAsync::Input,
            )?)
        };
        let trim_remaining = filter.as_ref().map_or(0, Resampler::output_delay);
        let scratch = vec![0.0; filter.as_ref().map_or(chunk, Resampler::output_frames_max)];
        Ok(Self {
            input_rate,
            output_rate,
            filter,
            pending: Vec::with_capacity(chunk),
            scratch,
            chunk,
            trim_remaining,
            input_frames: 0,
            output_frames: 0,
            ended: false,
        })
    }

    pub fn push(&mut self, input: &[f64]) -> Result<Vec<f64>> {
        ensure!(!self.ended, "audio after resampler End");
        self.input_frames = self
            .input_frames
            .checked_add(input.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("resampler source length overflow"))?;
        if self.filter.is_none() {
            self.output_frames += input.len() as u64;
            return Ok(input.to_vec());
        }
        let mut output = Vec::new();
        let mut remaining = input;
        while !remaining.is_empty() {
            let count = remaining.len().min(self.chunk - self.pending.len());
            self.pending.extend_from_slice(&remaining[..count]);
            remaining = &remaining[count..];
            if self.pending.len() == self.chunk {
                self.process(None, &mut output)?;
                self.pending.clear();
            }
        }
        Ok(output)
    }

    pub fn finish(&mut self) -> Result<Vec<f64>> {
        ensure!(!self.ended, "duplicate resampler End");
        self.ended = true;
        let expected = output_frames(self.input_frames, self.input_rate, self.output_rate);
        let mut output = Vec::new();
        if self.filter.is_none() || expected == 0 {
            return Ok(output);
        }
        if !self.pending.is_empty() {
            let available = self.pending.len();
            self.pending.resize(self.chunk, 0.0);
            self.process(Some(available), &mut output)?;
            self.pending.clear();
        }
        self.pending.resize(self.chunk, 0.0);
        // Enough zeros for the delayed final real source interval. Padding is
        // neither accepted input nor emitted beyond the finite clip duration.
        while self.output_frames < expected {
            self.process(Some(0), &mut output)?;
        }
        Ok(output)
    }

    fn process(&mut self, partial: Option<usize>, output: &mut Vec<f64>) -> Result<()> {
        let filter = self.filter.as_mut().expect("resampling path");
        let input = InterleavedSlice::new(&self.pending, 1, self.chunk)?;
        let capacity = self.scratch.len();
        let mut destination = InterleavedSlice::new_mut(&mut self.scratch, 1, capacity)?;
        let indexing = Indexing {
            partial_len: partial,
            ..Default::default()
        };
        // partial_len may cause the library's consumed count to include zeros.
        // Only push() records actual source frames.
        let (_, written) = filter.process_into_buffer(&input, &mut destination, Some(&indexing))?;
        let trim = self.trim_remaining.min(written);
        self.trim_remaining -= trim;
        let available = written - trim;
        let keep = if self.ended {
            available.min(
                (output_frames(self.input_frames, self.input_rate, self.output_rate)
                    - self.output_frames) as usize,
            )
        } else {
            available
        };
        output.extend_from_slice(&self.scratch[trim..trim + keep]);
        self.output_frames += keep as u64;
        ensure!(
            self.output_frames
                <= output_frames(self.input_frames, self.input_rate, self.output_rate),
            "resampler emitted beyond real input timeline"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn render(input: &[f64], from: u32, to: u32, packet: usize) -> Vec<f64> {
        let mut resampler = PcmResampler::new(from, to).unwrap();
        let mut output = Vec::new();
        for chunk in input.chunks(packet) {
            output.extend(resampler.push(chunk).unwrap());
        }
        output.extend(resampler.finish().unwrap());
        assert_eq!(
            output.len() as u64,
            output_frames(input.len() as u64, from, to)
        );
        output
    }

    #[test]
    fn packet_boundaries_preserve_phase_and_tail_for_short_and_long_clips() {
        for (from, to) in [
            (44100, 48000),
            (48000, 16000),
            (192000, 8000),
            (8000, 48000),
        ] {
            for count in [0, 1, 79, 480, 1001, 19201] {
                let input: Vec<_> = (0..count).map(|n| (n as f64 * 0.03).sin()).collect();
                let whole = render(&input, from, to, count.max(1));
                for packet in [1, 17, 319] {
                    let split = render(&input, from, to, packet);
                    assert_eq!(whole, split, "{from}->{to}, count={count}, packet={packet}");
                }
            }
        }
    }

    #[test]
    fn impulse_alignment_and_final_impulse_survive_delay_compensation() {
        for (from, to) in [
            (44100, 48000),
            (48000, 16000),
            (192000, 8000),
            (16000, 48000),
        ] {
            let count = from as usize / 10;
            for position in [0, count / 2, count - 1] {
                let mut input = vec![0.0; count];
                input[position] = 1.0;
                let output = render(&input, from, to, 117);
                let peak = output
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                    .unwrap()
                    .0;
                let expected = (position as u64 * to as u64 / from as u64) as usize;
                assert!(
                    peak.abs_diff(expected) <= 1,
                    "{from}->{to}, impulse {position}: peak={peak}, expected={expected}"
                );
                assert!(
                    output[peak].abs() > 1e-5,
                    "finite-clip tail impulse vanished: {from}->{to}, {position}"
                );
            }
        }
    }

    fn tone(from: u32, to: u32, frequency: f64) -> Vec<f64> {
        let input: Vec<_> = (0..from)
            .map(|n| (std::f64::consts::TAU * frequency * n as f64 / from as f64).sin())
            .collect();
        render(&input, from, to, 193)
    }

    #[test]
    fn sinc_spectral_quality_is_preserved_including_extreme_downsampling() {
        for (from, to) in [
            (48000, 16000),
            (44100, 8000),
            (192000, 8000),
            (96000, 24000),
        ] {
            // Ignore finite-clip boundary transients when measuring the filter.
            let trim = to as usize / 10;
            let pass = tone(from, to, to as f64 * 0.4);
            let pass_rms = (pass[trim..pass.len() - trim]
                .iter()
                .map(|x| x * x)
                .sum::<f64>()
                / (pass.len() - 2 * trim) as f64)
                .sqrt();
            let gain_db = 20.0 * (pass_rms * 2.0_f64.sqrt()).log10();
            assert!(gain_db.abs() <= 0.1, "{from}->{to} pass gain={gain_db} dB");
            for factor in [0.525, 0.65, 0.9] {
                let stop = tone(from, to, to as f64 * factor);
                let stop_rms = (stop[trim..stop.len() - trim]
                    .iter()
                    .map(|x| x * x)
                    .sum::<f64>()
                    / (stop.len() - 2 * trim) as f64)
                    .sqrt();
                let rejection_db = -20.0 * (stop_rms * 2.0_f64.sqrt()).log10();
                assert!(
                    rejection_db >= 80.0,
                    "{from}->{to}, factor={factor}: rejection={rejection_db} dB"
                );
            }
        }
    }

    #[test]
    fn end_is_single_use_and_unity_ratio_is_bit_exact() {
        let mut r = PcmResampler::new(16000, 16000).unwrap();
        assert_eq!(
            r.push(&[-32768.0, 0.0, 32767.0]).unwrap(),
            [-32768.0, 0.0, 32767.0]
        );
        assert!(r.finish().unwrap().is_empty());
        assert!(r.finish().is_err());
        assert!(r.push(&[1.0]).is_err());
    }
}
