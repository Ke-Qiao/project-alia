// TODO(sensor): Implement sensor reporting system:
//   - Add a `sensor` module that collects readings from hardware sensors (temperature, humidity, motion, etc.)
//   - Report sensor data to alia-server via gRPC `SensorData` channel events
//   - Support plugin-defined sensors (plugins can register custom sensor types)
//   - Periodic reporting interval configurable per sensor
//   - Batch multiple readings in a single gRPC call for efficiency

use std::path::PathBuf;

use alia_embedded::module::EmbeddedBuilder;
use alia_embedded::{config, identity, module};
use anyhow::{Context, Result};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[cfg(feature = "sherpa-onnx")]
use alia_embedded::module::traits::{EdgeSttEngine, WakeWordEngine};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(default_env_filter())
        .init();

    info!("alia-embedded starting");

    // Load configuration
    let config_path = std::env::var("ALIA_EMBEDDED_CONFIG_PATH")
        .map(PathBuf::from)
        .map(config::normalize_config_path)
        .unwrap_or_else(|_| config::default_config_path());
    let mut config = config::load_config(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;

    #[cfg(feature = "sherpa-onnx")]
    if let Some(path) = std::env::var_os("ALIA_EMBEDDED_CHECK_STT_WAV") {
        return check_sherpa_stt(config.edge_stt.clone(), PathBuf::from(path)).await;
    }

    #[cfg(feature = "sherpa-onnx")]
    if let Some(path) = std::env::var_os("ALIA_EMBEDDED_CHECK_KWS_WAV") {
        return check_sherpa_kws(config.wakeword.clone(), PathBuf::from(path));
    }

    let keypair =
        identity::ensure_device_keypair(&config.identity.keypair_path).with_context(|| {
            format!(
                "ensuring device keypair at {}",
                config.identity.keypair_path.display()
            )
        })?;

    let generated_device_id = if config.identity.device_id.is_empty() {
        config.identity.device_id = derived_device_id(&keypair.public_key_bytes());
        true
    } else {
        false
    };

    if generated_device_id || !config_path.exists() {
        config::save_runtime_config(&config).with_context(|| {
            format!(
                "persisting embedded runtime settings to {}",
                config.config_path.display()
            )
        })?;
    }

    info!(device_id = %config.identity.device_id, device_name = %config.identity.device_name, "identity loaded");
    info!(
        keypair_path = %config.identity.keypair_path.display(),
        public_key_len = keypair.public_key_bytes().len(),
        "device keypair ready"
    );

    // Build and run the embedded node using the module builder
    let node = EmbeddedBuilder::from_config(config).await?.build().await?;

    node.run().await?;

    Ok(())
}

fn default_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("sqlx::query=warn".parse().expect("valid sqlx query filter"))
}

#[cfg(feature = "sherpa-onnx")]
async fn check_sherpa_stt(config: config::EdgeSttConfig, wav_path: PathBuf) -> Result<()> {
    let wav_path_display = wav_path.display().to_string();
    let wave = sherpa_onnx::Wave::read(&wav_path_display)
        .ok_or_else(|| anyhow::anyhow!("failed to read STT check WAV {wav_path_display}"))?;
    let samples = wave
        .samples()
        .iter()
        .map(|sample| {
            let scaled = (*sample * 32768.0).round();
            scaled.clamp(i16::MIN as f32, i16::MAX as f32) as i16
        })
        .collect::<Vec<_>>();
    let max_duration_ms = config
        .max_duration_ms
        .max(((samples.len() as u64 * 1_000) / wave.sample_rate().max(1) as u64) as u32 + 1_000);
    let mut config = config;
    config.max_duration_ms = max_duration_ms;
    if let Ok(language) = std::env::var("ALIA_EMBEDDED_CHECK_STT_LANGUAGE")
        && !language.trim().is_empty()
    {
        config.language = language;
    }
    let stt = module::sherpa::SherpaOnnxStt::new(&config).context("creating sherpa edge STT")?;
    let transcript = stt
        .transcribe_short(&samples, max_duration_ms)
        .await
        .context("running sherpa edge STT")?
        .unwrap_or_default();

    println!("{transcript}");
    Ok(())
}

#[cfg(feature = "sherpa-onnx")]
fn check_sherpa_kws(config: config::WakewordConfig, wav_path: PathBuf) -> Result<()> {
    let wav_path_display = wav_path.display().to_string();
    let wave = sherpa_onnx::Wave::read(&wav_path_display)
        .ok_or_else(|| anyhow::anyhow!("failed to read KWS check WAV {wav_path_display}"))?;
    let kws =
        module::sherpa::SherpaOnnxKws::new(&config).context("creating sherpa wakeword KWS")?;
    let pcm = wave
        .samples()
        .iter()
        .map(|sample| {
            let scaled = (*sample * 32768.0).round();
            scaled.clamp(i16::MIN as f32, i16::MAX as f32) as i16
        })
        .collect::<Vec<_>>();

    let chunk_samples = (wave.sample_rate().max(1) as usize / 10).max(1);
    for chunk in pcm.chunks(chunk_samples) {
        if let Some(event) = kws.detect(chunk) {
            println!("{}", event.keyword);
            return Ok(());
        }
    }

    println!("NO_WAKEWORD");
    Ok(())
}

fn derived_device_id(public_key: &[u8; 32]) -> String {
    let suffix = public_key[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("alia-embedded-{suffix}")
}
