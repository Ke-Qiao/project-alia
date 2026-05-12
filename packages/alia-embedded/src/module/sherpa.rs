use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use sherpa_onnx::{
    KeywordSpotter, KeywordSpotterConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineSenseVoiceModelConfig, OnlineStream,
};

use crate::config::{EdgeSttConfig, WakewordConfig};

use super::context::ModuleContext;
use super::lifecycle::ModuleLifecycle;
use super::traits::{EdgeSttEngine, WakeWordEngine, WakeWordEvent};

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
        kws_config.keywords_score = 1.0 + config.sensitivity.clamp(0.0, 1.0);
        kws_config.keywords_threshold = config.sensitivity.clamp(0.0, 1.0);

        let spotter = KeywordSpotter::create(&kws_config)
            .ok_or_else(|| anyhow::anyhow!("failed to create sherpa-onnx keyword spotter"))?;
        let stream = spotter.create_stream();

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
        if duration_ms > max_duration_ms {
            bail!("utterance is {duration_ms}ms, over local STT limit of {max_duration_ms}ms");
        }

        let recognizer = Arc::clone(&self.recognizer);
        let samples = pcm16_to_f32(pcm);
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

fn pcm16_to_f32(pcm: &[i16]) -> Vec<f32> {
    pcm.iter().map(|sample| *sample as f32 / 32768.0).collect()
}

fn timestamp_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
