//! Routing fallback and budgets, independent of PostgreSQL and real providers.
#[path = "support/llm.rs"]
mod llm_fixture;

use alia_server::modules::interaction::routing::{
    JevRouter, RouteDecision, RouterConfig, RouterInput, RouterService,
};
use axum::{Json, Router, http::StatusCode, routing::post};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::Semaphore;

struct Peer {
    endpoint: String,
    calls: Arc<AtomicUsize>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
    task: tokio::task::JoinHandle<()>,
}

impl Peer {
    async fn start(status: StatusCode, body: Value, hang: bool) -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let observed = calls.clone();
        let signal = entered.clone();
        let response_gate = release.clone();
        let app = Router::new().route(
            "/v1/systemone",
            post(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                signal.add_permits(1);
                let body = body.clone();
                let response_gate = response_gate.clone();
                async move {
                    if hang {
                        response_gate.acquire().await.unwrap().forget();
                    }
                    (status, Json(body))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            endpoint,
            calls,
            entered,
            release,
            task,
        }
    }

    fn router(&self, minor_calls: Arc<AtomicUsize>) -> Arc<RouterService> {
        let minor = llm_fixture::test_text("must-not-reclassify", move |_| {
            minor_calls.fetch_add(1, Ordering::SeqCst);
            async { anyhow::bail!("Jev fallback invoked the legacy classifier") }
        });
        Arc::new(
            RouterService::new(
                RouterConfig {
                    timeout_sec: 1,
                    minor_llm_enabled: true,
                    major_llm_enabled: true,
                },
                Some(Arc::new(minor)),
            )
            .unwrap()
            .with_decision_model(Some(Arc::new(
                JevRouter::new("fixture-key", &self.endpoint, "jev-1.14.0", "jev-1.14.0").unwrap(),
            ))),
        )
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn input() -> RouterInput {
    RouterInput {
        text: "打开它。".into(),
        user_id: uuid::Uuid::new_v4(),
        conversation_context: vec![],
        assistant_name: "Alia".into(),
        available_skills: vec![],
    }
}

fn response() -> Value {
    json!({
        "model":"jev-1.14.0",
        "answers":{"route":{"type":"choice","choice":"clarify","confidence":1.0,
            "probabilities":{"minor_llm":0.0,"major_llm":0.0,"clarify":1.0}}},
        "usage":{"input_tokens":100,"output_tokens":8}
    })
}

#[tokio::test]
async fn invalid_or_failed_decisions_use_one_local_fallback_without_legacy_classification() {
    let mut bad_option = response();
    bad_option["answers"]["route"]["choice"] = json!("execute_unoffered_skill");
    let mut bad_probability = response();
    bad_probability["answers"]["route"]["probabilities"]["clarify"] = json!(1.1);
    for (status, body, reason) in [
        (StatusCode::OK, bad_option, "invalid_response"),
        (StatusCode::OK, bad_probability, "invalid_response"),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"private-provider-body"}),
            "transport",
        ),
    ] {
        let peer = Peer::start(status, body, false).await;
        let minor_calls = Arc::new(AtomicUsize::new(0));
        let router = peer.router(minor_calls.clone());
        let result = router.route_observed(&input()).await.unwrap();
        assert!(matches!(result.decision, RouteDecision::MajorLlm { .. }));
        assert!(!result.clarification_only);
        assert_eq!(result.observation.fallback_reason, Some(reason));
        assert_eq!(
            result.observation.configured_model.as_deref(),
            Some("jev-1.14.0")
        );
        assert!(result.observation.choice.is_none());
        assert!(result.observation.probabilities.is_none());
        assert!(
            !serde_json::to_string(&result.observation)
                .unwrap()
                .contains("private-provider-body")
        );
        assert!(matches!(
            router.fallback("Alia").await,
            RouteDecision::MajorLlm { .. }
        ));
        assert_eq!(peer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(minor_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn hung_decision_exhausts_the_original_deadline_without_a_second_budget() {
    let peer = Peer::start(StatusCode::OK, response(), true).await;
    let minor_calls = Arc::new(AtomicUsize::new(0));
    let router = peer.router(minor_calls.clone());
    let task_router = router.clone();
    let task = tokio::spawn(async move { task_router.route_observed(&input()).await });
    tokio::time::timeout(Duration::from_secs(1), peer.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    // Let the real loopback exchange arrive before advancing the virtual deadline.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(1)).await;
    let expired_at = tokio::time::Instant::now();
    let result = task.await.unwrap().unwrap();
    assert_eq!(
        tokio::time::Instant::now(),
        expired_at,
        "fallback obtained another waiting budget"
    );
    assert!(result.observation.elapsed_ms >= 1000.0);
    assert_eq!(
        result.observation.fallback_reason,
        Some("deadline_exceeded")
    );
    assert!(matches!(result.decision, RouteDecision::MajorLlm { .. }));
    assert!(matches!(
        router.fallback("Alia").await,
        RouteDecision::MajorLlm { .. }
    ));
    assert_eq!(tokio::time::Instant::now(), expired_at);
    assert_eq!(peer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(minor_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn valid_clarification_is_a_minor_answer_without_escalation_or_fallback() {
    let peer = Peer::start(StatusCode::OK, response(), false).await;
    let minor_calls = Arc::new(AtomicUsize::new(0));
    let result = peer
        .router(minor_calls.clone())
        .route_observed(&input())
        .await
        .unwrap();
    assert!(result.clarification_only);
    let RouteDecision::MinorLlm {
        system_prompt,
        escalation_prompt,
    } = result.decision
    else {
        panic!("missing object must be clarified by the available minor role");
    };
    assert!(escalation_prompt.is_none());
    assert!(
        system_prompt
            .unwrap()
            .contains("No tools are available for this clarification turn")
    );
    assert_eq!(result.observation.choice.as_deref(), Some("clarify"));
    assert_eq!(result.observation.model.as_deref(), Some("jev-1.14.0"));
    assert_eq!(result.observation.probabilities.unwrap()["clarify"], 1.0);
    assert!(result.observation.fallback_reason.is_none());
    assert_eq!(peer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(minor_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn live_tier_change_cannot_turn_clarification_into_an_action_capable_fallback() {
    use alia_server::modules::platform::lifecycle::ModuleLifecycle;
    use std::collections::BTreeMap;
    let peer = Peer::start(StatusCode::OK, response(), true).await;
    let minor_calls = Arc::new(AtomicUsize::new(0));
    let router = peer.router(minor_calls.clone());
    let task_router = router.clone();
    let task = tokio::spawn(async move { task_router.route_observed(&input()).await });
    tokio::time::timeout(Duration::from_secs(1), peer.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    router
        .apply_settings(&BTreeMap::from([(
            "core.policies.routing.minor_llm_enabled".into(),
            json!(false),
        )]))
        .await
        .unwrap();
    peer.release.add_permits(1);
    let result = task.await.unwrap().unwrap();
    assert!(
        result.clarification_only,
        "missing objects remain missing after a tier change"
    );
    let RouteDecision::MajorLlm { system_prompt } = result.decision else {
        panic!("the newly enabled tier must own this clarification");
    };
    assert!(
        system_prompt
            .unwrap()
            .contains("No tools are available for this clarification turn")
    );
    assert_eq!(result.observation.choice.as_deref(), Some("clarify"));
    assert_eq!(
        result.observation.fallback_reason,
        Some("routing_policy_changed")
    );
    assert_eq!(peer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(minor_calls.load(Ordering::SeqCst), 0);
}
