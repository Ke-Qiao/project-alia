//! Isolated admission tests own the real global budget without allocating audio.
use super::*;
use crate::modules::delivery::audio::{AudioReservation, MAX_GLOBAL_ENCODED_BYTES};
use futures::poll;
use std::{future::Future, task::Poll};

fn isolated_budget_case(name: &str, case: impl Future<Output = ()>) {
    const CHILD: &str = "ALIA_SOVITS_SETTINGS_BUDGET_TEST";
    let name = name.strip_prefix("alia_server::").unwrap();
    if std::env::var(CHILD).as_deref() == Ok(name) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(case);
        return;
    }

    // A separate process prevents quota exhaustion from blocking parallel tests.
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env(CHILD, name)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                assert!(status.success(), "isolated SoVITS budget case failed");
                return;
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            outcome => {
                let _ = child.kill();
                let joined = child.wait();
                panic!("SoVITS budget child did not finish: {outcome:?}; joined: {joined:?}");
            }
        }
    }
}

async fn reserve_global_capacity() -> Vec<AudioReservation> {
    let mut held = Vec::new();
    for _ in 0..MAX_GLOBAL_ENCODED_BYTES / MAX_RESPONSE_ENCODED_BYTES {
        let budget = ResponseAudioBudget::for_standalone_request(uuid::Uuid::new_v4()).unwrap();
        for _ in 0..MAX_RESPONSE_ENCODED_BYTES / MAX_ENCODED_PAYLOAD_BYTES {
            held.push(budget.reserve(MAX_ENCODED_PAYLOAD_BYTES).await.unwrap());
        }
    }
    held
}

#[test]
fn weight_setup_deadline_includes_audio_capacity_admission() {
    isolated_budget_case(
        concat!(
            module_path!(),
            "::weight_setup_deadline_includes_audio_capacity_admission"
        ),
        async {
            let held = reserve_global_capacity().await;
            let client = Client::new();
            let mut operation = Box::pin(tokio::task::unconstrained(set_remote_weights(
                &client,
                "http://127.0.0.1:9/tts",
                "set_gpt_weights",
                "weights_path",
                "test.ckpt",
                Duration::from_millis(100),
            )));
            assert!(poll!(operation.as_mut()).is_pending());
            tokio::time::advance(Duration::from_millis(101)).await;
            let Poll::Ready(result) = poll!(operation.as_mut()) else {
                panic!("weight setup deadline must include its audio capacity wait");
            };
            assert!(
                result.is_err(),
                "quota exhaustion must not report remote success"
            );
            drop((operation, held));
            let reclaimed = tokio::time::timeout(Duration::from_secs(1), reserve_global_capacity())
                .await
                .expect("expired weight setup retained audio capacity");
            assert_eq!(
                reclaimed.len(),
                MAX_GLOBAL_ENCODED_BYTES / MAX_ENCODED_PAYLOAD_BYTES
            );
        },
    );
}

#[test]
fn cancelling_weight_setup_keeps_local_settings_and_releases_queued_capacity() {
    isolated_budget_case(
        concat!(
            module_path!(),
            "::cancelling_weight_setup_keeps_local_settings_and_releases_queued_capacity"
        ),
        async {
            let held = reserve_global_capacity().await;
            let tts = GptSovitsTts::new(
                &GptSovitsProviderSection::default(),
                "default",
                &BTreeMap::new(),
                &BTreeMap::new(),
            );
            let updates = BTreeMap::from([
                (
                    "core.providers.gpt_sovits.endpoint".to_owned(),
                    serde_json::json!("http://127.0.0.1:9/tts"),
                ),
                (
                    "core.profiles.voice.default.provider_specific.gpt_weights_path".to_owned(),
                    serde_json::json!("new-local.ckpt"),
                ),
            ]);
            let mut operation = Box::pin(tokio::task::unconstrained(tts.apply_settings(&updates)));
            assert!(poll!(operation.as_mut()).is_pending());
            assert_eq!(
                tts.config
                    .read()
                    .await
                    .default_profile
                    .gpt_weights_path
                    .as_deref(),
                Some("new-local.ckpt")
            );
            drop(operation);
            assert_eq!(
                tts.config.read().await.endpoint,
                "http://127.0.0.1:9/tts",
                "cancelling remote setup must not roll back local configuration"
            );
            drop(held);
            let reclaimed = tokio::time::timeout(Duration::from_secs(1), reserve_global_capacity())
                .await
                .expect("cancelled settings apply retained audio capacity");
            assert_eq!(
                reclaimed.len(),
                MAX_GLOBAL_ENCODED_BYTES / MAX_ENCODED_PAYLOAD_BYTES
            );
        },
    );
}
