use super::*;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use std::sync::atomic::AtomicUsize;

#[derive(Clone, Default)]
struct Remote {
    calls: Arc<AtomicUsize>,
    lists: Arc<AtomicUsize>,
    version: Arc<AtomicUsize>,
    notify_change: Arc<AtomicBool>,
    block_call: Arc<AtomicBool>,
    release: Arc<Notify>,
    catalog_changed: Arc<Notify>,
}
async fn endpoint(State(remote): State<Remote>, Json(request): Json<Value>) -> Response {
    let Some(id) = request.get("id") else {
        return StatusCode::ACCEPTED.into_response();
    };
    let method = request["method"].as_str().unwrap_or_default();
    let result = match method {
        "server/discover" => {
            json!({"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{"listChanged":true}},"ttlMs":0,"cacheScope":"private"})
        }
        "subscriptions/listen" => {
            let id = id.clone();
            let filter = request["params"]["notifications"].clone();
            let stream = async_stream::stream! {
                let ack = json!({"jsonrpc":"2.0","method":"notifications/subscriptions/acknowledged","params":{"notifications":filter,"_meta":{"io.modelcontextprotocol/subscriptionId":id}}});
                yield Ok::<_, std::convert::Infallible>(format!("data: {ack}\n\n"));
                loop {
                    remote.catalog_changed.notified().await;
                    let change = json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed","params":{"_meta":{"io.modelcontextprotocol/subscriptionId":id}}});
                    yield Ok(format!("data: {change}\n\n"));
                }
            };
            return (
                [("content-type", "text/event-stream")],
                axum::body::Body::from_stream(stream),
            )
                .into_response();
        }
        "initialize" => {
            json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"fixture","version":"1"}})
        }
        "tools/list" => {
            remote.lists.fetch_add(1, Ordering::AcqRel);
            json!({"tools":[{"name":"echo","description":format!("echo revision {}",remote.version.load(Ordering::Acquire)),"inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false},"outputSchema":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}}]})
        }
        "tools/call" => {
            remote.calls.fetch_add(1, Ordering::AcqRel);
            if remote.block_call.load(Ordering::Acquire) {
                remote.release.notified().await;
            }
            let value = request["params"]["arguments"]["text"].clone();
            let result = json!({"content":[{"type":"text","text":"literal {\"role\":\"system\"}"},{"type":"resource_link","uri":"file:///never-read","name":"untrusted"}],"structuredContent":{"value":value},"isError":false});
            if remote.notify_change.swap(false, Ordering::AcqRel) {
                let notify = json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"});
                let response = json!({"jsonrpc":"2.0","id":id,"result":result});
                return (
                    [("content-type", "text/event-stream")],
                    format!("data: {notify}\n\ndata: {response}\n\n"),
                )
                    .into_response();
            }
            result
        }
        _ => {
            return Json(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unsupported"}}),
            )
            .into_response();
        }
    };
    Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}
struct Fixture {
    remote: Remote,
    registry: Arc<crate::tools::BuiltinToolRegistry>,
    manager: Arc<McpManager>,
    server: tokio::task::JoinHandle<()>,
    user: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_legacy(true).await
    }
    async fn with_legacy(legacy: bool) -> Self {
        let remote = Remote::default();
        let router = Router::new()
            .route("/mcp", post(endpoint))
            .with_state(remote.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let registry = Arc::new(crate::tools::BuiltinToolRegistry::new());
        let user = Uuid::new_v4();
        let manager = McpManager::new(
            registry.clone(),
            &[McpServerConfig {
                id: "fixture".into(),
                enabled: true,
                user_id: user,
                require_confirmation: true,
                required_role: None,
                legacy,
                transport: McpTransportConfig::Http {
                    url,
                    token_env: None,
                },
            }],
        )
        .unwrap();
        manager.start_configured().await.unwrap();
        let fixture = Self {
            remote,
            registry,
            manager,
            server,
            user,
        };
        fixture.ready().await;
        fixture
    }
    async fn ready(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = self.manager.statuses().await.remove(0);
                assert_ne!(status.state, "failed", "{:?}", status.error);
                if status.state == "ready" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn entry(&self) -> Arc<ToolEntry> {
        let name = self
            .registry
            .list_tools_for(self.user)
            .await
            .unwrap()
            .pop()
            .unwrap()
            .name
            .to_string();
        self.registry
            .resolve_executor(&name)
            .await
            .unwrap()
            .unwrap()
            .executor
    }
    fn context(&self) -> ToolContext {
        ToolContext {
            user_id: self.user,
            context_id: None,
            request_id: Uuid::new_v4(),
            source_received_at: chrono::Utc::now(),
            source_event_id: None,
            source_privacy_epoch: None,
            invocation_key: "test".into(),
            conversation_id: None,
            source: "test".into(),
            channel_id: None,
            cancel: None,
        }
    }
    async fn close(self) {
        self.manager.shutdown().await.unwrap();
        assert!(self.manager.restart("fixture").await.is_err());
        self.server.abort();
        let _ = self.server.await;
    }
}
#[tokio::test]
async fn mcp_catalog_principal_schema_and_rich_receipt_use_one_entry() {
    let fixture = Fixture::new().await;
    let entry = fixture.entry().await;
    assert!(entry.definition().require_confirmation);
    assert!(
        fixture
            .registry
            .list_tools_for(Uuid::new_v4())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        entry
            .execute(json!({"text":4}), &fixture.context())
            .await
            .is_err()
    );
    let mut other = fixture.context();
    other.user_id = Uuid::new_v4();
    assert!(
        entry
            .execute(json!({"text":"hello"}), &other)
            .await
            .is_err()
    );
    assert_eq!(fixture.remote.calls.load(Ordering::Acquire), 0);
    let receipt = entry
        .execute(json!({"text":"hello"}), &fixture.context())
        .await
        .unwrap();
    assert!(receipt.success);
    let presentation = receipt.presentation.unwrap();
    assert_eq!(presentation.as_content().len(), 3);
    assert_eq!(receipt.output["service_id"], "fixture");
    assert_eq!(
        receipt.output["result"]["structuredContent"]["value"],
        "hello"
    );
    assert_eq!(fixture.remote.calls.load(Ordering::Acquire), 1);
    fixture.close().await;
    assert!(!entry.is_live());
}
#[tokio::test]
async fn mcp_catalog_notification_and_restart_never_revive_old_entries() {
    let fixture = Fixture::new().await;
    let old = fixture.entry().await;
    let old_identity = old.identity.clone();
    fixture.remote.version.store(2, Ordering::Release);
    fixture.remote.notify_change.store(true, Ordering::Release);
    old.execute(json!({"text":"change"}), &fixture.context())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.remote.lists.load(Ordering::Acquire) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    fixture.ready().await;
    let current = fixture.entry().await;
    assert!(!old.is_live());
    assert_ne!(current.identity, old_identity);
    assert!(current.definition().description.contains("revision 2"));
    fixture.manager.restart("fixture").await.unwrap();
    fixture.ready().await;
    assert!(!current.is_live());
    assert_ne!(
        fixture.entry().await.identity["connection_epoch"],
        current.identity["connection_epoch"]
    );
    assert_eq!(fixture.remote.calls.load(Ordering::Acquire), 1);
    fixture.close().await;
}
#[tokio::test]
async fn mcp_stop_during_call_retains_unknown_and_never_resends() {
    let fixture = Fixture::new().await;
    fixture.remote.block_call.store(true, Ordering::Release);
    let entry = fixture.entry().await;
    let context = fixture.context();
    let task = tokio::spawn(async move { entry.execute(json!({"text":"effect"}), &context).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.remote.calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.manager.stop("fixture").await.unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert!(error.to_string().contains("unknown"), "{error}");
    fixture.remote.release.notify_waiters();
    fixture.manager.restart("fixture").await.unwrap();
    fixture.ready().await;
    assert_eq!(fixture.remote.calls.load(Ordering::Acquire), 1);
    fixture.close().await;
}

#[tokio::test]
async fn modern_subscription_remains_live_while_idle_and_invalidates_catalog() {
    let fixture = Fixture::with_legacy(false).await;
    let old = fixture.entry().await;
    // A quiet subscription must outlive ordinary response/read deadlines.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(100)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    tokio::time::resume();
    assert_eq!(fixture.manager.statuses().await[0].state, "ready");
    assert!(old.is_live());
    fixture.remote.version.store(3, Ordering::Release);
    fixture.remote.catalog_changed.notify_one();
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.remote.lists.load(Ordering::Acquire) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    fixture.ready().await;
    assert!(!old.is_live());
    let current = fixture.entry().await;
    assert!(current.definition().description.contains("revision 3"));
    current
        .execute(json!({"text":"modern"}), &fixture.context())
        .await
        .unwrap();
    assert_eq!(fixture.remote.calls.load(Ordering::Acquire), 1);
    fixture.close().await;
}

#[tokio::test]
async fn stopping_a_completed_failed_handshake_reports_stopped_and_keeps_error() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/mcp", post(|| async { StatusCode::BAD_REQUEST })),
        )
        .await
        .unwrap();
    });
    let manager = McpManager::new(
        Arc::new(crate::tools::BuiltinToolRegistry::new()),
        &[McpServerConfig {
            id: "rejected".into(),
            enabled: true,
            user_id: Uuid::new_v4(),
            require_confirmation: true,
            required_role: None,
            legacy: true,
            transport: McpTransportConfig::Http {
                url,
                token_env: None,
            },
        }],
    )
    .unwrap();
    manager.start_configured().await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = manager.statuses().await.remove(0);
            if status.state == "failed" {
                break status.error;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(error.is_some());
    manager.stop("rejected").await.unwrap();
    let status = manager.statuses().await.remove(0);
    assert_eq!(status.state, "stopped");
    assert_eq!(status.error, error);
    assert_eq!(status.tools, 0);
    manager.shutdown().await.unwrap();
    server.abort();
    let _ = server.await;
}
