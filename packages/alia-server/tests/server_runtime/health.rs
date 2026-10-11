//! Real HTTP health budgets, shared profiles and owned probe lifetimes.
#![cfg(feature = "llm-openai")]
use super::*;
use alia_server::config::LlmProviderProfileSection;
use alia_server::modules::platform::lifecycle::ModuleHealth;
use alia_server::providers::adapter::{ChatMessage, LlmAdapter};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const ROLES: [&str; 4] = [
    "llm.minor",
    "llm.major",
    "llm.memory_capture",
    "llm.sentence_expression",
];
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .unwrap()
}
async fn response(client: &reqwest::Client, url: &str) -> Value {
    let response = client.get(url).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert!(body["status"].is_string());
    assert!(body["version"].is_string());
    assert!(body["modules"].is_object());
    body
}
async fn stop_runtime(
    stop: &CancellationToken,
    mut serving: tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    stop.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
    if joined.is_err() {
        serving.abort();
        let _ = serving.await;
    }
    joined.expect("HTTP runtime must join").unwrap().unwrap();
}
async fn local_provider(
    listener: tokio::net::TcpListener,
    calls: Arc<AtomicUsize>,
    entered: Arc<Semaphore>,
    healthy: Arc<AtomicBool>,
    stop: CancellationToken,
) {
    let mut requests = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            result = requests.join_next(), if !requests.is_empty() => {
                result.unwrap().unwrap();
            },
            accepted = listener.accept() => {
                let (mut socket, _) = accepted.unwrap();
                let calls = calls.clone();
                let entered = entered.clone();
                let healthy = healthy.clone();
                let stop = stop.clone();
                requests.spawn(async move {
                    tokio::select! {
                        _ = stop.cancelled() => {},
                        _ = async {
                            let mut request = Vec::new();
                            while !request.ends_with(b"\r\n\r\n") {
                                let mut byte = [0; 1];
                                socket.read_exact(&mut byte).await.unwrap();
                                request.push(byte[0]);
                                assert!(request.len() <= 8192);
                            }
                            let request = String::from_utf8(request).unwrap();
                            assert!(request.starts_with("GET /v1/models/health-test-model HTTP/1.1\r\n"));
                            assert!(request.to_ascii_lowercase().contains("\r\nauthorization: bearer local-runtime-health-secret\r\n"));
                            calls.fetch_add(1, Ordering::SeqCst);
                            entered.add_permits(1);
                            if healthy.load(Ordering::SeqCst) {
                                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                            } else {
                                // No response headers: the real adapter must reach its own deadline.
                                let _ = socket.read(&mut [0; 1]).await;
                            }
                        } => {},
                    }
                    let _ = socket.shutdown().await;
                });
            }
        }
    }
    while let Some(result) = requests.join_next().await {
        result.unwrap();
    }
}

#[tokio::test]
async fn shared_openai_health_is_opt_in_single_probe_and_busy_requests_do_not_queue() {
    for probe_enabled in [false, true] {
        with_fixture(move |mut f| async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap();
            let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
            f.config.llm.roles.default = "major".into();
            f.config.llm.roles.minor = "major".into();
            f.config.llm.roles.major = "major".into();
            f.config.llm.roles.memory_capture = Some("major".into());
            f.config.llm.roles.sentence_expression = Some("major".into());
            f.config.policies.routing.minor_llm_enabled = true;
            f.config.policies.routing.major_llm_enabled = true;
            let profile = f.config.profiles.llm.get_mut("major").unwrap();
            profile.provider = "openai".into();
            profile.model = "health-test-model".into();
            let LlmProviderProfileSection::OpenAi(provider) =
                f.config.providers.llm.get_mut("openai").unwrap()
            else {
                panic!("OpenAI fixture provider is required");
            };
            assert!(
                !provider.health_probe_enabled,
                "example must keep probes opt-in"
            );
            provider.api_key = "local-runtime-health-secret".into();
            provider.base_url = endpoint.clone();
            provider.health_probe_enabled = probe_enabled;
            let calls = Arc::new(AtomicUsize::new(0));
            let entered = Arc::new(Semaphore::new(0));
            let healthy = Arc::new(AtomicBool::new(false));
            let provider_stop = CancellationToken::new();
            let _provider_drop = provider_stop.clone().drop_guard();
            let mut listener = Some(listener);
            let peer = probe_enabled.then(|| {
                tokio::spawn(local_provider(
                    listener.take().unwrap(),
                    calls.clone(),
                    entered.clone(),
                    healthy.clone(),
                    provider_stop.clone(),
                ))
            });
            let result = AssertUnwindSafe(async {
                let runtime = f.builder().build().await.unwrap();
                let state = runtime.state().clone();
                let stop = state.cancel_token.clone();
                let _runtime_drop = stop.clone().drop_guard();
                let url = format!("http://{}/health", runtime.http_addr());
                let serving = tokio::spawn(runtime.run());
                let assertions = AssertUnwindSafe(async {
                    assert!(Arc::ptr_eq(
                        &state.modules.minor_llm,
                        &state.modules.major_llm
                    ));
                    assert!(Arc::ptr_eq(
                        &state.modules.major_llm,
                        state.modules.memory_capture_llm.as_ref().unwrap()
                    ));
                    assert!(Arc::ptr_eq(
                        &state.modules.major_llm,
                        state.modules.sentence_expression_llm.as_ref().unwrap()
                    ));
                    let client = client();
                    if !probe_enabled {
                        let body = response(&client, &url).await;
                        for role in ROLES {
                            assert_eq!(body["modules"][role]["status"], "degraded");
                            assert!(
                                body["modules"][role]["message"]
                                    .as_str()
                                    .unwrap()
                                    .contains("probe disabled")
                            );
                        }
                        assert_eq!(body["modules"]["router"]["status"], "healthy");
                        let listener = listener.take().unwrap().into_std().unwrap();
                        assert_eq!(
                            listener.accept().unwrap_err().kind(),
                            std::io::ErrorKind::WouldBlock
                        );
                        return;
                    }
                    let started = Instant::now();
                    let request = response(&client, &url);
                    tokio::pin!(request);
                    tokio::select! {
                        _ = signalled(&entered) => {},
                        _ = &mut request => panic!("health finished before its actual probe arrived"),
                    }
                    let updates = std::collections::BTreeMap::from([
                        ("core.profiles.llm.major.temperature".into(), json!(0.25)),
                        ("core.policies.routing.timeout_sec".into(), json!(3)),
                    ]);
                    let reports = tokio::time::timeout(
                        Duration::from_millis(500),
                        state.modules.apply_settings(&updates),
                    )
                    .await
                    .expect("provider and router settings must remain writable during health");
                    assert_eq!(
                        reports.iter().filter(|report| report.applied.iter().any(|key|
                            key == "core.profiles.llm.major.temperature"
                        )).count(),
                        1
                    );
                    assert!(reports.iter().any(|report| report.applied.iter().any(|key|
                        key == "core.policies.routing.timeout_sec"
                    )));
                    assert!(reports.iter().all(|report| report.error.is_none()));
                    let busy = tokio::time::timeout(
                        Duration::from_secs(2),
                        futures::future::join_all((0..4).map(|_| response(&client, &url))),
                    )
                    .await
                    .expect("overlapping health requests must not queue behind the live probe");
                    for body in &busy {
                        assert_eq!(body["status"], "degraded");
                        assert!(body["modules"].as_object().unwrap().values().all(|value|
                            value == &json!({
                                "status":"degraded", "message":"health check already in progress"
                            })
                        ));
                    }
                    assert_eq!(calls.load(Ordering::SeqCst), 1);
                    let body = request.await;
                    assert!(started.elapsed() >= Duration::from_millis(4500));
                    assert!(started.elapsed() < Duration::from_secs(7));
                    for role in ROLES {
                        assert_eq!(body["modules"][role], json!({
                            "status":"degraded", "message":"OpenAI profile 'major' probe timed out after 5s"
                        }));
                    }
                    assert_eq!(body["modules"]["router"]["status"], "healthy");
                    for key in ["stt", "tts"] {
                        assert_eq!(body["modules"][key]["status"], "healthy");
                    }
                    for key in ["memory", "conversation"] {
                        assert!(body["modules"][key]["status"].is_string());
                    }
                    for value in busy {
                        assert_eq!(
                            value["modules"].as_object().unwrap().keys().collect::<Vec<_>>(),
                            body["modules"].as_object().unwrap().keys().collect::<Vec<_>>()
                        );
                    }
                    let serialized = body.to_string();
                    for secret in ["local-runtime-health-secret", &endpoint, "Authorization"] {
                        assert!(!serialized.contains(secret));
                    }
                    assert_eq!(
                        calls.load(Ordering::SeqCst),
                        1,
                        "router or another role repeated the model probe"
                    );
                    healthy.store(true, Ordering::SeqCst);
                    let fresh = response(&client, &url).await;
                    for role in ROLES {
                        assert_eq!(fresh["modules"][role]["status"], "healthy");
                    }
                    assert_eq!(
                        calls.load(Ordering::SeqCst),
                        2,
                        "a later request must probe afresh"
                    );
                })
                .catch_unwind()
                .await;
                stop_runtime(&stop, serving).await;
                if let Err(panic) = assertions {
                    std::panic::resume_unwind(panic);
                }
            })
            .catch_unwind()
            .await;
            provider_stop.cancel();
            if let Some(mut peer) = peer {
                let joined = tokio::time::timeout(Duration::from_secs(2), &mut peer).await;
                if joined.is_err() {
                    peer.abort();
                    let _ = peer.await;
                }
                joined.expect("local provider must join").unwrap();
            }
            if let Err(panic) = result {
                std::panic::resume_unwind(panic);
            }
        })
        .await;
    }
}

struct HealthProbeState {
    initialized: AtomicUsize,
    started: AtomicUsize,
    stopped: AtomicUsize,
    settings: AtomicUsize,
    calls: AtomicUsize,
    active: AtomicUsize,
    completed: AtomicBool,
    entered: Semaphore,
}

impl Default for HealthProbeState {
    fn default() -> Self {
        Self {
            initialized: AtomicUsize::new(0),
            started: AtomicUsize::new(0),
            stopped: AtomicUsize::new(0),
            settings: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            completed: AtomicBool::new(false),
            entered: Semaphore::new(0),
        }
    }
}

struct PendingProbe(Arc<HealthProbeState>);
impl Drop for PendingProbe {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

struct GatedLlm(Arc<HealthProbeState>);
#[async_trait]
impl ModuleLifecycle for GatedLlm {
    fn name(&self) -> &'static str {
        "independent-health-fixture"
    }
    async fn init(&mut self) -> anyhow::Result<()> {
        self.0.initialized.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn start(&self) -> anyhow::Result<()> {
        self.0.started.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn stop(&self) -> anyhow::Result<()> {
        self.0.stopped.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn health_check(&self) -> ModuleHealth {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        self.0.active.fetch_add(1, Ordering::SeqCst);
        let _pending = PendingProbe(self.0.clone());
        self.0.entered.add_permits(1);
        if !self.0.completed.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        ModuleHealth::Healthy
    }
    async fn apply_settings(
        &self,
        settings: &std::collections::BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        if settings.contains_key("health.fixture.value") {
            self.0.settings.fetch_add(1, Ordering::SeqCst);
            Ok(vec!["health.fixture.value".into()])
        } else {
            Ok(Vec::new())
        }
    }
}
#[async_trait]
impl LlmAdapter for GatedLlm {
    async fn chat(&self, _: &[ChatMessage]) -> anyhow::Result<String> {
        anyhow::bail!("health must never generate text")
    }
    async fn chat_stream(
        &self,
        _: &[ChatMessage],
    ) -> anyhow::Result<std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<String>> + Send>>>
    {
        anyhow::bail!("health must never generate text")
    }
}

#[tokio::test]
async fn http_health_has_one_total_budget_and_preserves_independent_same_name_instances() {
    with_fixture(|f| async move {
        let minor = Arc::new(HealthProbeState::default());
        let major = Arc::new(HealthProbeState::default());
        let runtime = f
            .builder()
            .with_minor_llm(GatedLlm(minor.clone()))
            .with_major_llm(GatedLlm(major.clone()))
            .build()
            .await
            .unwrap();
        let state = runtime.state().clone();
        let stop = state.cancel_token.clone();
        let _runtime_drop = stop.clone().drop_guard();
        let url = format!("http://{}/health", runtime.http_addr());
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            assert!(!Arc::ptr_eq(
                &state.modules.minor_llm,
                &state.modules.major_llm
            ));
            let client = client();
            let started = Instant::now();
            let request = response(&client, &url);
            tokio::pin!(request);
            let entered = async {
                tokio::join!(signalled(&minor.entered), signalled(&major.entered));
            };
            tokio::select! {
                _ = entered => {},
                _ = &mut request => panic!("both independent probes must start"),
            }
            for probe in [&minor, &major] {
                assert_eq!(probe.active.load(Ordering::SeqCst), 1);
                assert_eq!(probe.initialized.load(Ordering::SeqCst), 1);
                assert_eq!(probe.started.load(Ordering::SeqCst), 1);
            }
            let reports = state
                .modules
                .apply_settings(&std::collections::BTreeMap::from([(
                    "health.fixture.value".into(),
                    json!(1),
                )]))
                .await;
            assert_eq!(
                reports
                    .iter()
                    .filter(|report| report.applied == ["health.fixture.value"])
                    .count(),
                2
            );
            let body = request.await;
            assert!(started.elapsed() >= Duration::from_millis(5500));
            assert!(started.elapsed() < Duration::from_secs(8));
            for role in ["llm.minor", "llm.major"] {
                assert_eq!(
                    body["modules"][role],
                    json!({
                        "status":"degraded", "message":"health check timed out after 6s"
                    })
                );
            }
            for key in ["stt", "tts", "router"] {
                assert_eq!(body["modules"][key]["status"], "healthy");
            }
            for probe in [&minor, &major] {
                assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
                assert_eq!(
                    probe.active.load(Ordering::SeqCst),
                    0,
                    "deadline retained a probe future"
                );
                assert_eq!(probe.settings.load(Ordering::SeqCst), 1);
                probe.completed.store(true, Ordering::SeqCst);
            }
            let fresh = response(&client, &url).await;
            for role in ["llm.minor", "llm.major"] {
                assert_eq!(fresh["modules"][role]["status"], "healthy");
            }
            for probe in [&minor, &major] {
                assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
            }
        })
        .catch_unwind()
        .await;
        stop_runtime(&stop, serving).await;
        for probe in [&minor, &major] {
            assert_eq!(probe.stopped.load(Ordering::SeqCst), 1);
            assert_eq!(probe.active.load(Ordering::SeqCst), 0);
        }
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    })
    .await;
}

#[tokio::test]
async fn stopping_http_health_drops_active_probes_before_runtime_join() {
    with_fixture(|f| async move {
        let probe = Arc::new(HealthProbeState::default());
        let runtime = f
            .builder()
            .with_major_llm(GatedLlm(probe.clone()))
            .build()
            .await
            .unwrap();
        let state = runtime.state().clone();
        let stop = state.cancel_token.clone();
        let _runtime_drop = stop.clone().drop_guard();
        let url = format!("http://{}/health", runtime.http_addr());
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            let request = response(&client, &url);
            tokio::pin!(request);
            tokio::select! {
                _ = signalled(&probe.entered) => {},
                _ = &mut request => panic!("probe must enter before cancellation"),
            }
            assert_eq!(probe.active.load(Ordering::SeqCst), 1);
            stop.cancel();
            let body = tokio::time::timeout(Duration::from_secs(2), request)
                .await
                .expect("stopping must not wait for the health budget");
            assert_eq!(
                body["modules"]["llm.major"],
                json!({
                    "status":"degraded", "message":"server is stopping"
                })
            );
            assert_eq!(probe.active.load(Ordering::SeqCst), 0);
            let stopped = state.modules.health().await;
            assert!(stopped.values().all(|health| matches!(
                health,
                ModuleHealth::Degraded(message) if message == "server is stopping"
            )));
            assert_eq!(
                probe.calls.load(Ordering::SeqCst),
                1,
                "stopped registry admitted another probe"
            );
        })
        .catch_unwind()
        .await;
        stop_runtime(&stop, serving).await;
        assert_eq!(probe.stopped.load(Ordering::SeqCst), 1);
        assert_eq!(probe.active.load(Ordering::SeqCst), 0);
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    })
    .await;
}
