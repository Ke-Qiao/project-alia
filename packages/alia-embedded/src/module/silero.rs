use std::sync::Mutex;

use anyhow::{Context, Result, ensure};
use ort::session::Session;
use ort::value::{Outlet, TensorElementType, TensorRef};

use crate::config::VadConfig;

use super::context::ModuleContext;
use super::lifecycle::ModuleLifecycle;
use super::traits::{VoiceActivityEngine, VoiceActivityState};

const SAMPLE_RATE: u32 = 16_000;
const WINDOW: usize = 512;
const STATE_SIZE: usize = 2 * 64;

pub struct SileroVad {
    state: Mutex<VadState>,
}

struct VadState {
    session: Session,
    window: [f32; WINDOW],
    pending: usize,
    h: [f32; STATE_SIZE],
    c: [f32; STATE_SIZE],
    endpoint: Endpoint,
    latest: VoiceActivityState,
}

struct Endpoint {
    threshold: f32,
    min_speech_samples: u64,
    min_silence_samples: u64,
    onset: Option<u64>,
    silence: Option<u64>,
    active: bool,
}

impl Endpoint {
    fn new(config: &VadConfig) -> Self {
        Self {
            threshold: config.threshold,
            min_speech_samples: u64::from(SAMPLE_RATE) * u64::from(config.min_speech_duration_ms)
                / 1_000,
            min_silence_samples: u64::from(SAMPLE_RATE) * u64::from(config.min_silence_duration_ms)
                / 1_000,
            onset: None,
            silence: None,
            active: false,
        }
    }

    fn reset(&mut self) {
        self.onset = None;
        self.silence = None;
        self.active = false;
    }

    fn accept(&mut self, probability: f32) -> VoiceActivityState {
        // Relative, saturated counters preserve Silero's hysteresis indefinitely.
        if let Some(onset) = self.onset.as_mut() {
            *onset = onset
                .saturating_add(WINDOW as u64)
                .min(self.min_speech_samples);
        }
        if let Some(silence) = self.silence.as_mut() {
            *silence = silence
                .saturating_add(WINDOW as u64)
                .min(self.min_silence_samples);
        }
        if probability > self.threshold {
            self.silence = None;
            if self.onset.is_none() {
                self.onset = Some(0);
                return VoiceActivityState::Silence;
            }
            if !self.active {
                if self.onset.unwrap_or_default() < self.min_speech_samples {
                    return VoiceActivityState::Silence;
                }
                self.active = true;
                return VoiceActivityState::Speech;
            }
        }
        if probability < self.threshold && !self.active {
            self.onset = None;
            self.silence = None;
            return VoiceActivityState::Silence;
        }
        if self.active
            && (probability >= self.threshold || probability > (self.threshold - 0.15).max(0.01))
        {
            return VoiceActivityState::Speech;
        }
        if self.active && probability < self.threshold {
            let silence = self.silence.get_or_insert(0);
            if *silence < self.min_silence_samples {
                return VoiceActivityState::Speech;
            }
            self.reset();
            return VoiceActivityState::Finished;
        }
        VoiceActivityState::Silence
    }
}

impl VadState {
    fn reset(&mut self) {
        self.window.fill(0.0);
        self.pending = 0;
        self.h.fill(0.0);
        self.c.fill(0.0);
        self.endpoint.reset();
        self.latest = VoiceActivityState::Silence;
    }

    fn infer(&mut self) -> Result<()> {
        let outputs = self.session.run(ort::inputs! {
            "x" => TensorRef::from_array_view(([1, WINDOW], &self.window[..]))?,
            "h" => TensorRef::from_array_view(([2, 1, 64], &self.h[..]))?,
            "c" => TensorRef::from_array_view(([2, 1, 64], &self.c[..]))?,
        })?;
        let (prob_shape, probability) = outputs["prob"].try_extract_tensor::<f32>()?;
        let (h_shape, h) = outputs["new_h"].try_extract_tensor::<f32>()?;
        let (c_shape, c) = outputs["new_c"].try_extract_tensor::<f32>()?;
        ensure!(
            &prob_shape[..] == [1, 1] && &h_shape[..] == [2, 1, 64] && &c_shape[..] == [2, 1, 64],
            "Silero VAD returned incompatible output shapes"
        );
        let probability = probability[0];
        ensure!(
            probability.is_finite()
                && (0.0..=1.0).contains(&probability)
                && h.iter().chain(c).all(|value| value.is_finite()),
            "Silero VAD returned invalid probability or recurrent state"
        );
        self.h.copy_from_slice(h);
        self.c.copy_from_slice(c);
        self.latest = self.endpoint.accept(probability);
        Ok(())
    }
}

impl SileroVad {
    pub fn new(config: &VadConfig) -> Result<Self> {
        config.validate()?;
        ensure!(config.model_path.is_file(), "VAD model is not a file");
        // ort and KWS share the same linked runtime; custom native builds must match.
        let api_base = unsafe { ort_sys::OrtGetApiBase().as_ref() }
            .context("ONNX Runtime API is unavailable")?;
        ensure!(
            !unsafe { (api_base.GetApi)(ort_sys::ORT_API_VERSION) }.is_null(),
            "Silero VAD requires ONNX Runtime API {}",
            ort_sys::ORT_API_VERSION
        );
        let mut builder = Session::builder()?
            .with_intra_threads(config.num_threads as usize)
            .and_then(|builder| builder.with_inter_threads(1))
            .and_then(|builder| builder.with_no_environment_execution_providers())
            .and_then(|builder| {
                builder.with_log_level(if config.debug {
                    ort::logging::LogLevel::Verbose
                } else {
                    ort::logging::LogLevel::Warning
                })
            })
            .map_err(|error| anyhow::anyhow!("configuring Silero VAD: {error}"))?;
        let session = builder
            .commit_from_file(&config.model_path)
            .context("loading Silero VAD model")?;
        let metadata = session.metadata()?;
        ensure!(
            metadata.custom("model_type").as_deref() == Some("silero-vad")
                && metadata.custom("version").as_deref() == Some("1"),
            "VAD requires the k2-fsa Silero v4 16 kHz model"
        );
        drop(metadata);
        validate_signature(
            session.inputs(),
            &[("x", &[1, 512]), ("h", &[2, 1, 64]), ("c", &[2, 1, 64])],
        )?;
        validate_signature(
            session.outputs(),
            &[
                ("prob", &[-1, 1]),
                ("new_h", &[2, -1, 64]),
                ("new_c", &[2, -1, 64]),
            ],
        )?;
        let mut state = VadState {
            session,
            window: [0.0; WINDOW],
            pending: 0,
            h: [0.0; STATE_SIZE],
            c: [0.0; STATE_SIZE],
            endpoint: Endpoint::new(config),
            latest: VoiceActivityState::Silence,
        };
        state.infer().context("validating Silero VAD outputs")?;
        state.reset();
        tracing::info!("created streaming Silero VAD with bounded recurrent state");
        Ok(Self {
            state: Mutex::new(state),
        })
    }
}

#[cfg(test)]
mod tests;

fn validate_signature(outlets: &[Outlet], expected: &[(&str, &[i64])]) -> Result<()> {
    ensure!(
        outlets.len() == expected.len(),
        "incompatible Silero VAD tensor count"
    );
    for (name, shape) in expected {
        let outlet = outlets
            .iter()
            .find(|outlet| outlet.name() == *name)
            .with_context(|| format!("Silero VAD is missing tensor {name}"))?;
        let actual = outlet
            .dtype()
            .tensor_shape()
            .context("Silero VAD requires tensors")?;
        ensure!(
            outlet.dtype().tensor_type() == Some(TensorElementType::Float32)
                && actual.len() == shape.len()
                && actual.iter().zip(*shape).all(|(actual, expected)| {
                    actual == expected || (*expected == -1 && *actual == 1)
                }),
            "Silero VAD tensor {name} has an incompatible type or shape"
        );
    }
    Ok(())
}

#[tonic::async_trait]
impl ModuleLifecycle for SileroVad {
    fn name(&self) -> &'static str {
        "silero_vad"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> Result<()> {
        ensure!(
            ctx.config.audio.capture_sample_rate == SAMPLE_RATE,
            "Silero VAD requires 16 kHz capture"
        );
        Ok(())
    }
}

impl VoiceActivityEngine for SileroVad {
    fn reset(&self) -> Result<()> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("Silero VAD mutex poisoned"))?
            .reset();
        Ok(())
    }

    fn accept(&self, mut pcm: &[i16], sample_rate: u32) -> Result<VoiceActivityState> {
        ensure!(
            sample_rate == SAMPLE_RATE,
            "Silero VAD expected 16000 Hz audio, got {sample_rate} Hz"
        );
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("Silero VAD mutex poisoned"))?;
        while !pcm.is_empty() {
            let n = (WINDOW - state.pending).min(pcm.len());
            let pending = state.pending;
            for (target, sample) in state.window[pending..pending + n].iter_mut().zip(&pcm[..n]) {
                *target = f32::from(*sample) / 32_768.0;
            }
            state.pending += n;
            pcm = &pcm[n..];
            if state.pending == WINDOW {
                if let Err(error) = state.infer() {
                    state.reset();
                    return Err(error.context("Silero VAD inference failed; state reset"));
                }
                state.pending = 0;
            }
        }
        Ok(state.latest)
    }
}
