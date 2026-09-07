//! Opt-in test of the production Rust adapter against real prepared model weights.
#![cfg(feature = "turn-easy")]

use alia_core::voice::{DetectionId, PcmWindow, TurnState};
use alia_server::adapters::easy_turn::{EasyTurnConfig, EasyTurnDetector};
use alia_server::modules::interaction::turn_detector::TurnDetector;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "requires a real Easy-Turn service and ALIA_EASY_TURN_PCM pointing to official backchannel_real_1 PCM16"]
async fn official_audio_through_production_rust_adapter() {
    let path = std::env::var("ALIA_EASY_TURN_PCM")
        .expect("set ALIA_EASY_TURN_PCM to raw 16 kHz mono PCM16");
    let audio = PcmWindow::new(std::fs::read(path).unwrap(), 16_000, 1).unwrap();
    let detector = EasyTurnDetector::new(&EasyTurnConfig {
        enabled: true,
        endpoint: std::env::var("ALIA_EASY_TURN_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8091".into()),
        timeout_ms: 30_000,
        max_concurrency: 1,
    })
    .unwrap();
    let id = DetectionId::new(17);
    let observation = detector
        .detect(id, &audio, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(observation.id, id);
    assert_eq!(observation.state, TurnState::Backchannel);
    assert_eq!(observation.model.source, "ASLP-lab/Easy-Turn");
    assert_eq!(
        observation.model.revision,
        "7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98"
    );
    assert!(!observation.transcript.trim().is_empty());
    eprintln!(
        "real Easy-Turn Rust adapter: {:?}, {} ms on {}",
        observation.state, observation.inference_ms, observation.model.device
    );
}
