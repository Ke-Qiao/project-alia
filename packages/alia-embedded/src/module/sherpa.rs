use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use sherpa_onnx::{
    KeywordSpotter, KeywordSpotterConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineSenseVoiceModelConfig, OnlineStream, SileroVadModelConfig, VadModelConfig,
    VoiceActivityDetector,
};

use crate::config::{EdgeSttConfig, VadConfig, WakewordConfig};

use super::context::ModuleContext;
use super::lifecycle::ModuleLifecycle;
use super::traits::{
    EdgeSttEngine, VoiceActivityEngine, VoiceActivityState, WakeWordEngine, WakeWordEvent,
};

pub struct SherpaOnnxKws {
    state: Mutex<KwsState>,
    sample_rate: u32,
}

struct KwsState {
    spotter: KeywordSpotter,
    stream: OnlineStream,
}

impl SherpaOnnxKws {
    pub fn new(config: &WakewordConfig) -> Result<Self> {
        validate_file(&config.encoder_path, "wakeword encoder")?;
        validate_file(&config.decoder_path, "wakeword decoder")?;
        validate_file(&config.joiner_path, "wakeword joiner")?;
        validate_file(&config.tokens_path, "wakeword tokens")?;
        validate_file(&config.keywords_file, "wakeword keywords")?;
        warn_if_keywords_look_unconverted(&config.keywords_file, &config.tokens_path);

        let mut kws_config = KeywordSpotterConfig::default();
        kws_config.model_config.transducer.encoder =
            Some(config.encoder_path.display().to_string());
        kws_config.model_config.transducer.decoder =
            Some(config.decoder_path.display().to_string());
        kws_config.model_config.transducer.joiner = Some(config.joiner_path.display().to_string());
        kws_config.model_config.tokens = Some(config.tokens_path.display().to_string());
        kws_config.model_config.provider = Some(config.provider.clone());
        kws_config.model_config.num_threads = config.num_threads;
        kws_config.model_config.debug = config.debug;
        kws_config.keywords_file = Some(config.keywords_file.display().to_string());
        kws_config.keywords_score = config.keywords_score.max(0.0);
        kws_config.keywords_threshold = config.keywords_threshold.clamp(0.0, 1.0);

        let spotter = KeywordSpotter::create(&kws_config)
            .ok_or_else(|| anyhow::anyhow!("failed to create sherpa-onnx keyword spotter"))?;
        let stream = spotter.create_stream();
        tracing::info!("created sherpa-onnx keyword spotter");
        tracing::debug!(
            keywords_file = %config.keywords_file.display(),
            threshold = kws_config.keywords_threshold,
            score = kws_config.keywords_score,
            provider = %config.provider,
            "sherpa-onnx keyword spotter details"
        );

        Ok(Self {
            state: Mutex::new(KwsState { spotter, stream }),
            sample_rate: 16_000,
        })
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for SherpaOnnxKws {
    fn name(&self) -> &'static str {
        "sherpa_onnx_kws"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> Result<()> {
        self.sample_rate = ctx.config.audio.capture_sample_rate;
        Ok(())
    }
}

impl WakeWordEngine for SherpaOnnxKws {
    fn reset(&self) -> Result<()> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("wakeword state lock poisoned"))?;
        state.spotter.reset(&state.stream);
        Ok(())
    }

    fn detect(&self, pcm: &[i16]) -> Option<WakeWordEvent> {
        if pcm.is_empty() {
            return None;
        }

        let samples = pcm16_to_f32(pcm);
        let state = self.state.lock().ok()?;
        state
            .stream
            .accept_waveform(self.sample_rate as i32, &samples);

        while state.spotter.is_ready(&state.stream) {
            state.spotter.decode(&state.stream);
            if let Some(result) = state.spotter.get_result(&state.stream) {
                if !result.keyword.is_empty() {
                    state.spotter.reset(&state.stream);
                    return Some(WakeWordEvent {
                        keyword: result.keyword,
                        confidence: 1.0,
                        timestamp_us: timestamp_us(),
                    });
                }
            }
        }

        None
    }
}

pub struct SherpaOnnxStt {
    recognizer: Arc<Mutex<OfflineRecognizer>>,
    sample_rate: u32,
    max_duration_ms: u32,
}

impl SherpaOnnxStt {
    pub fn new(config: &EdgeSttConfig) -> Result<Self> {
        validate_file(&config.model_path, "edge STT model")?;
        validate_file(&config.tokens_path, "edge STT tokens")?;

        let mut recognizer_config = OfflineRecognizerConfig::default();
        recognizer_config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(config.model_path.display().to_string()),
            language: Some(config.language.clone()),
            use_itn: config.use_itn,
        };
        recognizer_config.model_config.tokens = Some(config.tokens_path.display().to_string());
        recognizer_config.model_config.provider = Some(config.provider.clone());
        recognizer_config.model_config.num_threads = config.num_threads;
        recognizer_config.model_config.debug = config.debug;

        let recognizer = OfflineRecognizer::create(&recognizer_config)
            .ok_or_else(|| anyhow::anyhow!("failed to create sherpa-onnx offline recognizer"))?;
        tracing::info!("created sherpa-onnx edge STT");
        tracing::debug!(
            model = %config.model_path.display(),
            tokens = %config.tokens_path.display(),
            language = %config.language,
            provider = %config.provider,
            "sherpa-onnx edge STT details"
        );

        Ok(Self {
            recognizer: Arc::new(Mutex::new(recognizer)),
            sample_rate: 16_000,
            max_duration_ms: config.max_duration_ms,
        })
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for SherpaOnnxStt {
    fn name(&self) -> &'static str {
        "sherpa_onnx_stt"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> Result<()> {
        self.sample_rate = ctx.config.audio.capture_sample_rate;
        Ok(())
    }
}

#[tonic::async_trait]
impl EdgeSttEngine for SherpaOnnxStt {
    async fn transcribe_short(&self, pcm: &[i16], max_duration_ms: u32) -> Result<Option<String>> {
        if pcm.is_empty() {
            return Ok(None);
        }

        let duration_ms = (pcm.len() as u64 * 1_000) / self.sample_rate.max(1) as u64;
        let max_duration_ms = max_duration_ms.min(self.max_duration_ms) as u64;
        let max_samples = ((self.sample_rate.max(1) as u64 * max_duration_ms) / 1_000) as usize;
        let pcm = if duration_ms > max_duration_ms {
            tracing::warn!(
                duration_ms,
                max_duration_ms,
                "trimming utterance to local STT limit"
            );
            &pcm[..pcm.len().min(max_samples)]
        } else {
            pcm
        };
        if pcm.is_empty() {
            return Ok(None);
        }

        let recognizer = Arc::clone(&self.recognizer);
        let samples = with_leading_silence(pcm16_to_f32(pcm), self.sample_rate);
        let sample_rate = self.sample_rate as i32;

        tokio::task::spawn_blocking(move || {
            let recognizer = recognizer
                .lock()
                .map_err(|_| anyhow::anyhow!("sherpa recognizer mutex poisoned"))?;
            let stream = recognizer.create_stream();
            stream.accept_waveform(sample_rate, &samples);
            recognizer.decode(&stream);
            let result = stream
                .get_result()
                .context("sherpa offline recognizer returned no result")?;
            let text = result.text.trim().to_string();
            Ok((!text.is_empty()).then_some(text))
        })
        .await
        .context("joining sherpa STT task")?
    }
}

pub struct SherpaOnnxVad {
    state: Mutex<VadState>,
    sample_rate: u32,
    window_size: usize,
}

struct VadState {
    detector: VoiceActivityDetector,
    pending_samples: Vec<f32>,
}

impl SherpaOnnxVad {
    pub fn new(config: &VadConfig) -> Result<Self> {
        validate_file(&config.model_path, "VAD model")?;

        let mut vad_config = VadModelConfig::default();
        vad_config.silero_vad = SileroVadModelConfig {
            model: Some(config.model_path.display().to_string()),
            threshold: config.threshold,
            min_silence_duration: millis_to_seconds(config.min_silence_duration_ms),
            min_speech_duration: millis_to_seconds(config.min_speech_duration_ms),
            window_size: config.window_size,
            max_speech_duration: millis_to_seconds(config.max_speech_duration_ms),
        };
        vad_config.sample_rate = config.sample_rate as i32;
        vad_config.num_threads = config.num_threads;
        vad_config.provider = Some(config.provider.clone());
        vad_config.debug = config.debug;

        let detector = VoiceActivityDetector::create(&vad_config, config.buffer_size_sec)
            .ok_or_else(|| anyhow::anyhow!("failed to create sherpa-onnx VAD"))?;
        tracing::info!("created sherpa-onnx VAD");
        tracing::debug!(
            model = %config.model_path.display(),
            sample_rate = config.sample_rate,
            threshold = config.threshold,
            min_silence_ms = config.min_silence_duration_ms,
            provider = %config.provider,
            "sherpa-onnx VAD details"
        );

        Ok(Self {
            state: Mutex::new(VadState {
                detector,
                pending_samples: Vec::with_capacity(config.window_size.max(1) as usize),
            }),
            sample_rate: config.sample_rate,
            window_size: config.window_size.max(1) as usize,
        })
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for SherpaOnnxVad {
    fn name(&self) -> &'static str {
        "sherpa_onnx_vad"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> Result<()> {
        if ctx.config.audio.capture_sample_rate != self.sample_rate {
            tracing::warn!(
                expected = self.sample_rate,
                actual = ctx.config.audio.capture_sample_rate,
                "audio.capture_sample_rate differs from VAD sample_rate; VAD endpointing may be inaccurate"
            );
        }
        Ok(())
    }
}

impl VoiceActivityEngine for SherpaOnnxVad {
    fn reset(&self) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("sherpa VAD mutex poisoned"))?;
        state.detector.reset();
        state.detector.clear();
        state.pending_samples.clear();
        Ok(())
    }

    fn accept(&self, pcm: &[i16], sample_rate: u32) -> Result<VoiceActivityState> {
        if pcm.is_empty() {
            return Ok(VoiceActivityState::Silence);
        }
        if sample_rate != self.sample_rate {
            bail!(
                "VAD expected {expected}Hz audio, got {actual}Hz",
                expected = self.sample_rate,
                actual = sample_rate
            );
        }

        let samples = pcm16_to_f32(pcm);
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("sherpa VAD mutex poisoned"))?;
        state.pending_samples.extend(samples);

        while state.pending_samples.len() >= self.window_size {
            let window = state
                .pending_samples
                .drain(..self.window_size)
                .collect::<Vec<_>>();
            state.detector.accept_waveform(&window);
        }

        if let Some(segment) = state.detector.front() {
            tracing::debug!(
                start = segment.start(),
                samples = segment.n(),
                "sherpa VAD finished speech segment"
            );
            drop(segment);
            state.detector.pop();
            return Ok(VoiceActivityState::Finished);
        }
        if state.detector.detected() {
            return Ok(VoiceActivityState::Speech);
        }
        Ok(VoiceActivityState::Silence)
    }
}

fn millis_to_seconds(value: u32) -> f32 {
    value as f32 / 1_000.0
}

fn pcm16_to_f32(pcm: &[i16]) -> Vec<f32> {
    pcm.iter().map(|sample| *sample as f32 / 32768.0).collect()
}

fn with_leading_silence(mut samples: Vec<f32>, sample_rate: u32) -> Vec<f32> {
    if samples.is_empty() || sample_rate == 0 {
        return samples;
    }

    let silence_samples = (sample_rate as usize * 200) / 1_000;
    let mut padded = Vec::with_capacity(silence_samples + samples.len());
    padded.resize(silence_samples, 0.0);
    padded.append(&mut samples);
    padded
}

fn timestamp_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn validate_file(path: &Path, label: &str) -> Result<()> {
    if path.is_file() {
        return Ok(());
    }

    bail!(
        "{label} file does not exist: {}. Run scripts/download-sherpa-models.sh from the project root, or update the embedded model paths in config/embedded.toml.",
        path.display()
    )
}

fn warn_if_keywords_look_unconverted(keywords_file: &Path, tokens_file: &Path) {
    let tokens = match std::fs::read_to_string(tokens_file) {
        Ok(content) => content
            .lines()
            .filter_map(|line| line.split_once(' ').map(|(token, _)| token.to_string()))
            .collect::<HashSet<_>>(),
        Err(error) => {
            tracing::warn!(
                error = ?error,
                tokens_file = %tokens_file.display(),
                "failed to validate sherpa keyword tokens"
            );
            return;
        }
    };

    let content = match std::fs::read_to_string(keywords_file) {
        Ok(content) => content,
        Err(error) => {
            tracing::warn!(
                error = ?error,
                keywords_file = %keywords_file.display(),
                "failed to validate sherpa keyword file"
            );
            return;
        }
    };

    for (line_index, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((token_part, _label)) = line.rsplit_once('@') else {
            tracing::warn!(
                keywords_file = %keywords_file.display(),
                line = line_index + 1,
                "sherpa keyword line has no @label"
            );
            continue;
        };

        let unknown = token_part
            .split_whitespace()
            .find(|token| !tokens.contains(*token));
        if let Some(token) = unknown {
            tracing::warn!(
                keywords_file = %keywords_file.display(),
                line = line_index + 1,
                token,
                "sherpa keyword line contains a token not present in tokens.txt; regenerate keywords.txt"
            );
        }
    }
}
