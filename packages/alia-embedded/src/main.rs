use std::path::PathBuf;

use alia_embedded::module::EmbeddedBuilder;
use alia_embedded::{config, identity};
use anyhow::{Context, Result};
use base64::Engine;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[cfg(feature = "sherpa-onnx")]
use alia_embedded::module;
#[cfg(feature = "sherpa-onnx")]
use alia_embedded::module::traits::WakeWordEngine;

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("creating embedded runtime")?;
    let result = runtime.block_on(async_main());
    // Normal node shutdown already joined workers. If native cleanup exceeded its
    // deadline, preserve that error and bound this final runtime reclamation step.
    // This timeout does not prove the remaining hardware workers stopped safely.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    result
}

async fn async_main() -> Result<()> {
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
        expected_device_public_key = %base64::engine::general_purpose::STANDARD.encode(keypair.public_key_bytes()),
        "device keypair ready"
    );

    // The executable owns process signals; library hosts supply their own token.
    // Keep polling startup and shutdown to completion so worker cleanup is not
    // lost by dropping the node future when a signal arrives.
    let shutdown = CancellationToken::new();
    let node_shutdown = shutdown.clone();
    let run = async move {
        let node = EmbeddedBuilder::from_config(config)
            .await?
            .with_cancel_token(node_shutdown)
            .build()
            .await?;
        node.run().await
    };
    tokio::pin!(run);
    let result = tokio::select! {
        result = &mut run => result,
        signal = shutdown_signal() => {
            info!("shutdown requested");
            shutdown.cancel();
            let result = run.await;
            signal.and(result)
        }
    };
    shutdown.cancel();
    result
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("installing SIGTERM listener")?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.context("Ctrl+C signal listener failed"),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .context("Ctrl+C signal listener failed")
}

fn default_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("sqlx::query=warn".parse().expect("valid sqlx query filter"))
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
