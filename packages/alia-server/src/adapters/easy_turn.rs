use std::sync::Arc;
use std::time::Duration;

use alia_core::voice::{DetectionId, PcmWindow, SAMPLE_RATE_HZ, TurnObservation};
use async_trait::async_trait;
use base64::Engine;
use reqwest::{Client, StatusCode, Url};
use serde::Serialize;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::modules::interaction::turn_detector::{DetectionError, TurnDetector};

const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub use crate::config::EasyTurnSection as EasyTurnConfig;

pub struct EasyTurnDetector {
    client: Client,
    endpoint: Url,
    timeout: Duration,
    permits: Arc<Semaphore>,
}

impl EasyTurnDetector {
    pub fn new(config: &EasyTurnConfig) -> anyhow::Result<Self> {
        anyhow::ensure!(
            config.timeout_ms > 0 && config.timeout_ms <= 120_000,
            "Easy-Turn timeout_ms must be in 1..=120000"
        );
        anyhow::ensure!(
            (1..=16).contains(&config.max_concurrency),
            "Easy-Turn max_concurrency must be in 1..=16"
        );
        let mut endpoint = Url::parse(&config.endpoint)?;
        anyhow::ensure!(
            matches!(endpoint.scheme(), "http" | "https") && endpoint.host_str().is_some(),
            "Easy-Turn endpoint must use HTTP(S)"
        );
        anyhow::ensure!(
            endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none(),
            "Easy-Turn endpoint cannot include credentials, a query or a fragment"
        );
        let path = format!("{}/v1/detect", endpoint.path().trim_end_matches('/'));
        endpoint.set_path(&path);
        Ok(Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()?,
            endpoint,
            timeout: Duration::from_millis(config.timeout_ms),
            permits: Arc::new(Semaphore::new(config.max_concurrency)),
        })
    }

    async fn request(
        &self,
        id: DetectionId,
        audio: &PcmWindow,
    ) -> Result<TurnObservation, DetectionError> {
        let payload = DetectRequest {
            id,
            audio: AudioRequest {
                encoding: "pcm_s16le",
                sample_rate_hz: SAMPLE_RATE_HZ,
                channels: 1,
                data_base64: base64::engine::general_purpose::STANDARD.encode(audio.as_bytes()),
            },
        };
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .json(&payload)
            .send()
            .await
            .map_err(|_| DetectionError::Unavailable("HTTP request failed".into()))?;
        match response.status() {
            StatusCode::GATEWAY_TIMEOUT => return Err(DetectionError::Timeout),
            StatusCode::TOO_MANY_REQUESTS => return Err(DetectionError::Busy),
            StatusCode::SERVICE_UNAVAILABLE => {
                return Err(DetectionError::Unavailable("model is not ready".into()));
            }
            status if !status.is_success() => {
                return Err(DetectionError::Unavailable(format!("HTTP {status}")));
            }
            _ => {}
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(DetectionError::InvalidResponse(
                "response exceeds size limit".into(),
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| DetectionError::InvalidResponse("incomplete response body".into()))?
        {
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err(DetectionError::InvalidResponse(
                    "response exceeds size limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let observation: TurnObservation = serde_json::from_slice(&bytes).map_err(|_| {
            DetectionError::InvalidResponse(
                "response does not match the turn detection contract".into(),
            )
        })?;
        if observation.id != id {
            return Err(DetectionError::InvalidResponse(
                "request ID or generation mismatch".into(),
            ));
        }
        if observation.model.source.is_empty()
            || observation.model.revision.is_empty()
            || observation.model.device.is_empty()
            || !observation.inference_ms.is_finite()
            || observation.inference_ms < 0.0
        {
            return Err(DetectionError::InvalidResponse(
                "model identity or timing is missing or invalid".into(),
            ));
        }
        Ok(observation)
    }
}

#[async_trait]
impl TurnDetector for EasyTurnDetector {
    async fn detect(
        &self,
        id: DetectionId,
        audio: &PcmWindow,
        cancel: &CancellationToken,
    ) -> Result<TurnObservation, DetectionError> {
        if cancel.is_cancelled() {
            return Err(DetectionError::Cancelled);
        }
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| DetectionError::Busy)?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(DetectionError::Cancelled),
            result = tokio::time::timeout(self.timeout, self.request(id, audio)) => {
                result.unwrap_or(Err(DetectionError::Timeout))
            }
        }
    }
}

#[derive(Serialize)]
struct DetectRequest {
    #[serde(flatten)]
    id: DetectionId,
    audio: AudioRequest,
}

#[derive(Serialize)]
struct AudioRequest {
    encoding: &'static str,
    sample_rate_hz: u32,
    channels: u32,
    data_base64: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use axum::{Json, Router, routing::post};
    use serde_json::{Value, json};
    use tokio::sync::Notify;

    struct TestServer {
        endpoint: String,
        task: tokio::task::JoinHandle<()>,
    }

    impl TestServer {
        async fn start(router: Router) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            Self { endpoint, task }
        }

        fn detector(&self, timeout_ms: u64) -> EasyTurnDetector {
            EasyTurnDetector::new(&EasyTurnConfig {
                endpoint: self.endpoint.clone(),
                timeout_ms,
                ..Default::default()
            })
            .unwrap()
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn observation(request: &Value) -> Value {
        json!({
            "request_id": request["request_id"],
            "generation_epoch": request["generation_epoch"],
            "transcript": "等一下",
            "state": "wait",
            "model": {"source": "ASLP-lab/Easy-Turn", "revision": "test-model", "device": "test"},
            "inference_ms": 2.5
        })
    }

    fn audio() -> PcmWindow {
        PcmWindow::new(vec![0; 640], SAMPLE_RATE_HZ, 1).unwrap()
    }

    #[tokio::test]
    async fn sends_valid_pcm_and_checks_correlated_model_observation() {
        let server = TestServer::start(Router::new().route(
            "/v1/detect",
            post(|Json(request): Json<Value>| async move {
                assert_eq!(request["audio"]["encoding"], "pcm_s16le");
                assert_eq!(request["audio"]["sample_rate_hz"], SAMPLE_RATE_HZ);
                assert_eq!(request["audio"]["channels"], 1);
                assert_eq!(
                    base64::engine::general_purpose::STANDARD
                        .decode(request["audio"]["data_base64"].as_str().unwrap())
                        .unwrap(),
                    vec![0; 640]
                );
                Json(observation(&request))
            }),
        ))
        .await;
        let id = DetectionId::new(42);
        let result = server
            .detector(1_000)
            .detect(id, &audio(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.id, id);
        assert_eq!(result.state, alia_core::voice::TurnState::Wait);
        assert_eq!(result.transcript, "等一下");
    }

    #[tokio::test]
    async fn rejects_late_generation_and_unknown_state() {
        for invalid_state in [false, true] {
            let server = TestServer::start(Router::new().route(
                "/v1/detect",
                post(move |Json(request): Json<Value>| async move {
                    let mut response = observation(&request);
                    if invalid_state {
                        response["state"] = json!("maybe");
                    } else {
                        response["generation_epoch"] = json!(0);
                    }
                    Json(response)
                }),
            ))
            .await;
            let error = server
                .detector(1_000)
                .detect(DetectionId::new(1), &audio(), &CancellationToken::new())
                .await
                .unwrap_err();
            assert!(matches!(error, DetectionError::InvalidResponse(_)));
        }
    }

    #[tokio::test]
    async fn cancels_in_flight_request_and_releases_local_capacity() {
        let started = Arc::new(Notify::new());
        let signal = started.clone();
        let server = TestServer::start(Router::new().route(
            "/v1/detect",
            post(move |Json(request): Json<Value>| {
                let signal = signal.clone();
                async move {
                    if request["generation_epoch"] == 1 {
                        signal.notify_one();
                        std::future::pending::<()>().await;
                    }
                    Json(observation(&request))
                }
            }),
        ))
        .await;
        let detector = Arc::new(server.detector(1_000));
        let cancel = CancellationToken::new();
        let task = {
            let detector = detector.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                detector
                    .detect(DetectionId::new(1), &audio(), &cancel)
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .unwrap();
        assert!(matches!(
            detector
                .detect(DetectionId::new(2), &audio(), &CancellationToken::new())
                .await,
            Err(DetectionError::Busy)
        ));
        cancel.cancel();
        assert!(matches!(
            task.await.unwrap(),
            Err(DetectionError::Cancelled)
        ));
        assert!(
            detector
                .detect(DetectionId::new(2), &audio(), &CancellationToken::new())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn bounds_deadline_response_size_and_upstream_errors() {
        for (status, expected) in [
            (429, "busy"),
            (503, "unavailable"),
            (504, "timeout"),
            (307, "unavailable"),
        ] {
            let server = TestServer::start(Router::new().route(
                "/v1/detect",
                post(move || async move {
                    (
                        StatusCode::from_u16(status).unwrap(),
                        "private provider details",
                    )
                }),
            ))
            .await;
            let error = server
                .detector(1_000)
                .detect(DetectionId::new(1), &audio(), &CancellationToken::new())
                .await
                .unwrap_err();
            assert!(!error.to_string().contains("private provider details"));
            match expected {
                "busy" => assert!(matches!(error, DetectionError::Busy)),
                "timeout" => assert!(matches!(error, DetectionError::Timeout)),
                _ => assert!(matches!(error, DetectionError::Unavailable(_))),
            }
        }
        let server = TestServer::start(Router::new().route(
            "/v1/detect",
            post(|| async { "x".repeat(MAX_RESPONSE_BYTES + 1) }),
        ))
        .await;
        assert!(matches!(
            server
                .detector(1_000)
                .detect(DetectionId::new(1), &audio(), &CancellationToken::new())
                .await,
            Err(DetectionError::InvalidResponse(_))
        ));

        let server = TestServer::start(Router::new().route(
            "/v1/detect",
            post(|| async {
                std::future::pending::<()>().await;
                StatusCode::OK.into_response()
            }),
        ))
        .await;
        assert!(matches!(
            server
                .detector(30)
                .detect(DetectionId::new(1), &audio(), &CancellationToken::new())
                .await,
            Err(DetectionError::Timeout)
        ));
    }

    #[test]
    fn rejects_invalid_config_before_starting_work() {
        for endpoint in [
            "file:///tmp/model",
            "http://user:secret@localhost",
            "http://localhost?token=secret",
            "http://localhost/#secret",
        ] {
            assert!(
                EasyTurnDetector::new(&EasyTurnConfig {
                    endpoint: endpoint.into(),
                    ..Default::default()
                })
                .is_err()
            );
        }
        assert!(
            EasyTurnDetector::new(&EasyTurnConfig {
                timeout_ms: 0,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            EasyTurnDetector::new(&EasyTurnConfig {
                max_concurrency: 0,
                ..Default::default()
            })
            .is_err()
        );
    }
}
