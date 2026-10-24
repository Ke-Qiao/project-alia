use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Result, bail};
use sherpa_onnx::{KeywordSpotter, KeywordSpotterConfig, OnlineStream};

use crate::config::WakewordConfig;

use super::context::ModuleContext;
use super::lifecycle::ModuleLifecycle;
use super::traits::{WakeWordEngine, WakeWordEvent, WakeWordSource};

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
                        timestamp_us: timestamp_us(),
                        source: WakeWordSource::Local,
                    });
                }
            }
        }

        None
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
