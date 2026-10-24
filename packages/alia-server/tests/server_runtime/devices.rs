//! REST authorization and status mapping, with a controlled channel adapter.
//! The fixture is not a real device or a claim about gRPC/physical hardware.
use super::*;
use alia_server::channels::adapter::{ChannelAction, ChannelAdapter, ChannelCapabilities};
use reqwest::Method;
use sea_orm::{DatabaseBackend, Statement};
use serde_json::{Value, json};

struct DeviceChannel {
    id: &'static str,
    owner: Option<Uuid>,
    calls: Arc<Mutex<Vec<(String, String)>>>,
    status_mode: Arc<AtomicUsize>,
}
#[async_trait]
impl ModuleLifecycle for DeviceChannel {
    fn name(&self) -> &'static str {
        "device-rest-fixture"
    }
}
#[async_trait]
impl alia_server::modules::delivery::transport::DeliveryTransport for DeviceChannel {
    fn channel_id(&self) -> &str {
        self.id
    }
    fn owner_user_id(&self) -> Option<Uuid> {
        self.owner
    }
}

#[async_trait::async_trait]
impl ChannelAdapter for DeviceChannel {
    fn channel_type(&self) -> &'static str {
        "embedded"
    }
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities::default()
    }

    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<Value> {
        self.calls
            .lock()
            .unwrap()
            .push((self.id.into(), action.name.clone()));
        match action.name.as_str() {
            "binding_status" => match self.status_mode.load(Ordering::SeqCst) {
                0 => Ok(json!({"device_id":format!("live-{}", self.id),"is_bound":true})),
                1 => anyhow::bail!("private device transport detail"),
                2 => Ok(json!({"device_id":"  ","is_bound":true})),
                3 => std::future::pending().await,
                _ => unreachable!(),
            },
            "discovery_status" => Ok(json!({"candidate":{
                "device_id":format!("discovered-{}",self.id),
                "device_name":"discovery fixture", "status":"unbound"
            }})),
            _ => Ok(json!({"success":true,"device_id":format!("live-{}",self.id)})),
        }
    }
}

fn operations() -> [(Method, &'static str, &'static str); 9] {
    [
        (Method::GET, "", "discovery_status"),
        (Method::GET, "/status", "binding_status"),
        (Method::POST, "/bind", "bind_device"),
        (Method::POST, "/unbind", "unbind_device"),
        (Method::GET, "/settings", "get_settings"),
        (Method::PATCH, "/settings", "update_settings"),
        (
            Method::GET,
            "/settings/direct-responses",
            "list_direct_responses",
        ),
        (
            Method::POST,
            "/settings/direct-responses",
            "upsert_direct_response",
        ),
        (
            Method::DELETE,
            "/settings/direct-responses/1",
            "delete_direct_response",
        ),
    ]
}

fn request(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    channel: &str,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    let body = if matches!(path, "/bind" | "/unbind") {
        json!({"channel_id":channel,"require_confirmation":false})
    } else {
        json!({})
    };
    let needs_body = method == Method::POST || method == Method::PATCH;
    let request = client
        .request(method, format!("{base}{path}?channel_id={channel}"))
        .bearer_auth(token);
    if needs_body {
        request.json(&body)
    } else {
        request
    }
}

#[tokio::test]
async fn device_routes_enforce_current_owner_or_admin_before_any_action() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, owner_token) = delivery::account(&state, "user").await;
        let (other, other_token) = delivery::account(&state, "user").await;
        let (_, no_device_token) = delivery::account(&state, "user").await;
        let (admin, admin_token) = delivery::account(&state, "admin").await;
        let calls = Arc::new(Mutex::new(Vec::new()));
        for (id, owner) in [
            ("device-a", Some(owner)),
            ("device-b", Some(other)),
            ("unlinked", None),
        ] {
            state
                .channels
                .register(Arc::new(DeviceChannel {
                    id,
                    owner,
                    calls: calls.clone(),
                    status_mode: Arc::default(),
                }))
                .await
                .unwrap();
        }
        let stop = state.cancel_token.clone();
        let _runtime_drop = stop.clone().drop_guard();
        let base = format!("http://{}/api/v1/device", runtime.http_addr());
        let mut serving = tokio::spawn(runtime.run());
        let assertions = AssertUnwindSafe(async {
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            assert_eq!(
                client
                    .get(format!("{base}/status"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                401
            );
            for (method, path, action) in operations() {
                for (token, channel) in [(&other_token, "device-a"), (&owner_token, "unlinked")] {
                    let before = calls.lock().unwrap().len();
                    let response = request(&client, &base, token, channel, method.clone(), path)
                        .send()
                        .await
                        .unwrap();
                    assert_eq!(response.status(), 403, "denied {method} {path}");
                    assert_eq!(
                        calls.lock().unwrap().len(),
                        before,
                        "denied action reached adapter"
                    );
                }
                for token in [&owner_token, &admin_token] {
                    let before = calls.lock().unwrap().len();
                    let response = request(&client, &base, token, "device-a", method.clone(), path)
                        .send()
                        .await
                        .unwrap();
                    assert_eq!(response.status(), 200, "authorized {method} {path}");
                    let observed = calls.lock().unwrap();
                    assert_eq!(observed.len(), before + 1);
                    assert_eq!(
                        observed.last().unwrap(),
                        &("device-a".into(), action.into())
                    );
                }
            }

            // Selection is over this account's accessible channels, not the
            // registry's arbitrary first embedded entry.
            let response: Value = client
                .get(format!("{base}/status"))
                .bearer_auth(&owner_token)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(response["device_id"], "live-device-a");
            let before = calls.lock().unwrap().len();
            assert_eq!(
                client
                    .get(format!("{base}/status"))
                    .bearer_auth(&no_device_token)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                404
            );
            assert_eq!(
                client
                    .get(format!("{base}/status"))
                    .bearer_auth(&admin_token)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                400
            );
            assert_eq!(calls.lock().unwrap().len(), before);
            assert_eq!(
                request(
                    &client,
                    &base,
                    &admin_token,
                    "unlinked",
                    Method::POST,
                    "/unbind"
                )
                .send()
                .await
                .unwrap()
                .status(),
                200
            );
            state
                .channels
                .register(Arc::new(DeviceChannel {
                    id: "device-a-second",
                    owner: Some(owner),
                    calls: calls.clone(),
                    status_mode: Arc::default(),
                }))
                .await
                .unwrap();
            let before = calls.lock().unwrap().len();
            assert_eq!(
                client
                    .get(format!("{base}/status"))
                    .bearer_auth(&owner_token)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                400
            );
            assert_eq!(calls.lock().unwrap().len(), before);
            assert_eq!(
                request(
                    &client,
                    &base,
                    &owner_token,
                    "device-a-second",
                    Method::GET,
                    "/status"
                )
                .send()
                .await
                .unwrap()
                .status(),
                200
            );

            // JWT still says admin, but current database role is authoritative.
            state
                .db
                .execute(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "UPDATE users SET role='user' WHERE id=$1",
                    [admin.into()],
                ))
                .await
                .unwrap();
            let before = calls.lock().unwrap().len();
            for (method, path, _) in operations() {
                assert_eq!(
                    request(&client, &base, &admin_token, "device-a", method, path)
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    403
                );
            }
            assert_eq!(calls.lock().unwrap().len(), before);
            state
                .db
                .execute(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "UPDATE users SET status='disabled' WHERE id=$1",
                    [owner.into()],
                ))
                .await
                .unwrap();
            for (method, path, _) in operations() {
                assert_eq!(
                    request(&client, &base, &owner_token, "device-a", method, path)
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    401
                );
            }
            assert_eq!(calls.lock().unwrap().len(), before);
        })
        .catch_unwind()
        .await;
        stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        if joined.is_err() {
            serving.abort();
            let _ = serving.await;
        }
        joined
            .expect("device REST runtime must join")
            .unwrap()
            .unwrap();
        if let Err(panic) = assertions {
            std::panic::resume_unwind(panic);
        }
    })
    .await;
}

#[tokio::test]
async fn device_status_uses_live_identity_and_reports_unavailable_without_fake_offline() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let status_mode = Arc::new(AtomicUsize::new(0));
        state
            .channels
            .register(Arc::new(DeviceChannel {
                id: "device-live",
                owner: Some(owner),
                calls: calls.clone(),
                status_mode: status_mode.clone(),
            }))
            .await
            .unwrap();
        let stop = state.cancel_token.clone();
        let _runtime_drop = stop.clone().drop_guard();
        let url = format!(
            "http://{}/api/v1/device/status?channel_id=device-live",
            runtime.http_addr()
        );
        let mut serving = tokio::spawn(runtime.run());
        let assertions = AssertUnwindSafe(async {
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(7))
                .build()
                .unwrap();
            let actual: Value = client
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(
                actual,
                json!({"device_id":"live-device-live","online":true,
                "cpu_percent":null,"memory_percent":null,"temperature_c":null})
            );
            for (mode, message) in [
                (1, "device status unavailable"),
                (2, "device status has no device identity"),
                (3, "device status query timed out"),
            ] {
                status_mode.store(mode, Ordering::SeqCst);
                let started = Instant::now();
                let response = client.get(&url).bearer_auth(&token).send().await.unwrap();
                assert_eq!(response.status(), 503);
                let body: Value = response.json().await.unwrap();
                assert_eq!(body["error"]["code"], "SERVICE_UNAVAILABLE");
                assert_eq!(body["error"]["message"], message);
                assert!(body.get("online").is_none());
                assert!(body.get("device_id").is_none());
                assert!(!body.to_string().contains("private device transport detail"));
                if mode == 3 {
                    assert!(started.elapsed() >= Duration::from_millis(4500));
                    assert!(started.elapsed() < Duration::from_secs(7));
                }
            }
            let observed = calls.lock().unwrap();
            assert_eq!(observed.len(), 4);
            assert!(
                observed
                    .iter()
                    .all(|(id, action)| id == "device-live" && action == "binding_status")
            );
        })
        .catch_unwind()
        .await;
        stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        if joined.is_err() {
            serving.abort();
            let _ = serving.await;
        }
        joined
            .expect("device status runtime must join")
            .unwrap()
            .unwrap();
        if let Err(panic) = assertions {
            std::panic::resume_unwind(panic);
        }
    })
    .await;
}
