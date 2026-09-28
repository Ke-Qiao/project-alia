//! Real OpenAI adapter and HTTP health aggregation against a stalled local peer.
#![cfg(feature = "llm-openai")]

use super::*;
use alia_server::config::LlmProviderProfileSection;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

#[tokio::test]
async fn live_openai_probe_timeout_preserves_other_http_health_results() {
    with_fixture(|mut f| async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let provider_stop = CancellationToken::new();
        let _provider_drop = provider_stop.clone().drop_guard();
        let peer_stop = provider_stop.clone();
        let (observed, probe_entered) = oneshot::channel();
        let mut peer = tokio::spawn(async move {
            tokio::select! {
                _ = peer_stop.cancelled() => {}
                _ = async {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        let mut byte = [0; 1];
                        socket.read_exact(&mut byte).await.unwrap();
                        request.push(byte[0]);
                        assert!(request.len() <= 8192);
                    }
                    let request = String::from_utf8(request).unwrap();
                    assert!(request.starts_with("GET /v1/models/health-test-model HTTP/1.1\r\n"));
                    assert!(request.to_ascii_lowercase().contains(
                        "\r\nauthorization: bearer local-runtime-health-secret\r\n"
                    ));
                    observed.send(()).unwrap();
                    // Keep the accepted connection open without any response
                    // headers. Only the adapter deadline may finish the probe.
                    peer_stop.cancelled().await;
                    let _ = socket.shutdown().await;
                } => {}
            }
        });
        let result = AssertUnwindSafe(async {
            f.config.llm.roles.major = "major".into();
            f.config.policies.routing.major_llm_enabled = true;
            let profile = f.config.profiles.llm.get_mut("major").unwrap();
            profile.provider = "openai".into();
            profile.model = "health-test-model".into();
            let LlmProviderProfileSection::OpenAi(provider) =
                f.config.providers.llm.get_mut("openai").unwrap()
            else {
                panic!("OpenAI fixture provider is required");
            };
            provider.api_key = "local-runtime-health-secret".into();
            provider.base_url = endpoint.clone();
            provider.health_probe_enabled = true;

            // The real factory initializes OpenAiLlm from this explicit profile.
            let runtime = f.builder().build().await.unwrap();
            let state = runtime.state().clone();
            assert_eq!(state.modules.major_llm.name(), "openai");
            let stop = state.cancel_token.clone();
            let _runtime_drop = stop.clone().drop_guard();
            let url = format!("http://{}/health", runtime.http_addr());
            let mut serving = tokio::spawn(runtime.run());
            let assertions = AssertUnwindSafe(async {
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(7))
                    .build()
                    .unwrap();
                let started = Instant::now();
                let request = client.get(&url).send();
                tokio::pin!(request);
                tokio::select! {
                    entered = probe_entered => entered.expect("the actual model probe must arrive"),
                    _ = &mut request => panic!("HTTP health finished before a provider probe arrived"),
                }

                // A live stalled request must not pin the provider settings lock.
                let applied = tokio::time::timeout(
                    Duration::from_millis(500),
                    state.modules.major_llm.apply_settings(&std::collections::BTreeMap::from([
                        ("core.profiles.llm.major.temperature".into(), json!(0.25)),
                    ])),
                )
                .await
                .expect("provider settings must remain available during a probe")
                .unwrap();
                assert_eq!(applied, vec!["core.profiles.llm.major.temperature"]);
                let response = request.await.expect("the server must finish before the client deadline");
                assert_eq!(response.status(), reqwest::StatusCode::OK);
                let body: Value = response.json().await.unwrap();
                assert!(started.elapsed() >= Duration::from_millis(4500));
                assert!(started.elapsed() < Duration::from_secs(7));
                assert_eq!(body["modules"]["llm.major"]["status"], "degraded");
                assert_eq!(
                    body["modules"]["llm.major"]["message"],
                    "OpenAI profile 'major' probe timed out after 5s"
                );
                assert_eq!(body["modules"]["stt"]["status"], "healthy");
                assert_eq!(body["modules"]["tts"]["status"], "healthy");
                assert!(body["modules"]["memory"]["status"].is_string());
                assert!(body["modules"]["conversation"]["status"].is_string());
                assert_ne!(body["status"], "ok");
                let serialized = body.to_string();
                assert!(!serialized.contains("local-runtime-health-secret"));
                assert!(!serialized.contains(&endpoint));
                assert!(!serialized.contains("Authorization"));
            })
            .catch_unwind()
            .await;
            stop.cancel();
            let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
            if joined.is_err() {
                serving.abort();
                let _ = serving.await;
            }
            joined.expect("HTTP runtime must join").unwrap().unwrap();
            if let Err(panic) = assertions {
                std::panic::resume_unwind(panic);
            }
        })
        .catch_unwind()
        .await;
        provider_stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(2), &mut peer).await;
        if joined.is_err() {
            peer.abort();
            let _ = peer.await;
        }
        joined.expect("stalled peer must release its connection").unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    })
    .await;
}
