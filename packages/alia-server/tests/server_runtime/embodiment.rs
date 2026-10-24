//! The foundation boots without physical adapters and exposes evidence only.
use super::*;
use alia_server::modules::embodiment::operations::{
    AdapterDescriptor, OperationFeedback, OperationIdentity, OperationRequest, OperationStatus,
    PhysicalAdapter, StoppedEvidence,
};
use alia_server::state::AppState;
use sea_orm::{DatabaseBackend, Statement};
use serde_json::{Value, json};

#[tokio::test]
async fn empty_embodiment_has_no_fake_capabilities_and_requires_current_account() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let entries = state.embodiment.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].definition().name.as_str(),
            "embodiment_operation_status"
        );
        let base = format!(
            "http://{}/api/v1/embodiment/operations",
            runtime.http_addr()
        );
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let (owner, token) = delivery::account(&state, "user").await;
            let client = skills::client();
            skills::response(client.get(&base), 401).await;
            assert_eq!(
                skills::response(client.get(&base).bearer_auth(&token), 200).await,
                json!([])
            );
            for query in ["limit=0", "limit=101", "limit=invalid", "unknown=1"] {
                skills::response(
                    client.get(format!("{base}?{query}")).bearer_auth(&token),
                    400,
                )
                .await;
            }
            skills::response(
                client.get(format!("{base}/invalid")).bearer_auth(&token),
                400,
            )
            .await;
            skills::response(
                client
                    .get(format!("{base}/{}", Uuid::new_v4()))
                    .bearer_auth(&token),
                404,
            )
            .await;
            assert_eq!(
                client
                    .post(&base)
                    .bearer_auth(&token)
                    .json(&json!({}))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                405
            );
            state
                .db
                .execute(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "UPDATE users SET status='disabled' WHERE id=$1",
                    [owner.into()],
                ))
                .await
                .unwrap();
            skills::response(client.get(&base).bearer_auth(&token), 401).await;
        })
        .catch_unwind()
        .await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(10), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}

#[derive(Default)]
struct PhysicalState {
    identity: Option<OperationIdentity>,
    sequence: u64,
    cancellation_requested: bool,
    stop_proven: bool,
}

/// A protocol fixture, not a simulated device driver: acceptance, cancellation
/// acknowledgement and positive stop evidence are deliberately separate.
struct PhysicalFixture {
    descriptor: AdapterDescriptor,
    state: Mutex<PhysicalState>,
    starts: AtomicUsize,
    cancels: AtomicUsize,
}

impl PhysicalFixture {
    fn new(owner_id: Uuid) -> Self {
        Self {
            descriptor: AdapterDescriptor {
                owner_id,
                device_id: Uuid::new_v4(),
                boot_id: Uuid::new_v4(),
                adapter_id: "runtime_fixture".into(),
                revision: "fixture-v1".into(),
                capability: "navigate".into(),
                capability_pin: "fixture-navigation-v1".into(),
                goal_schema: json!({"type":"object","properties":{"destination":{"const":"dock"}},"required":["destination"],"additionalProperties":false}),
                resources: vec!["base_motion".into()],
            },
            state: Mutex::new(PhysicalState::default()),
            starts: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
        }
    }

    fn feedback(state: &mut PhysicalState, identity: OperationIdentity) -> OperationFeedback {
        assert_eq!(state.identity.as_ref(), Some(&identity));
        state.sequence += 1;
        let observed_at_ms = chrono::Utc::now().timestamp_millis();
        let stopped = state.cancellation_requested && state.stop_proven;
        OperationFeedback {
            identity,
            sequence: state.sequence,
            observed_at_ms,
            status: if stopped {
                OperationStatus::Cancelled
            } else if state.cancellation_requested {
                OperationStatus::Cancelling
            } else {
                OperationStatus::Accepted
            },
            payload: json!({"cancellation_acknowledged":state.cancellation_requested}),
            stopped: stopped.then(|| StoppedEvidence {
                observed_at_ms,
                evidence: json!({"operation_controller":"stopped","source":"fixture_observation"}),
            }),
        }
    }
}

#[async_trait]
impl PhysicalAdapter for PhysicalFixture {
    fn descriptor(&self) -> AdapterDescriptor {
        self.descriptor.clone()
    }

    async fn start(&self, request: OperationRequest) -> anyhow::Result<OperationFeedback> {
        assert_eq!(request.goal, json!({"destination":"dock"}));
        self.starts.fetch_add(1, Ordering::SeqCst);
        let mut state = self.state.lock().unwrap();
        assert!(
            state.identity.is_none(),
            "the host must submit an operation only once"
        );
        state.identity = Some(request.identity.clone());
        Ok(Self::feedback(&mut state, request.identity))
    }

    async fn observe(&self, identity: OperationIdentity) -> anyhow::Result<OperationFeedback> {
        Ok(Self::feedback(&mut self.state.lock().unwrap(), identity))
    }

    async fn cancel(&self, identity: OperationIdentity) -> anyhow::Result<OperationFeedback> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        let mut state = self.state.lock().unwrap();
        state.cancellation_requested = true;
        Ok(Self::feedback(&mut state, identity))
    }
}

async fn fixed_account(state: &AppState, id: Uuid) -> String {
    state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Embodiment test','not-a-login-hash','user','active')",
        [id.into(), format!("{id}@embodiment.test").into()],
    )).await.unwrap();
    state
        .jwt_keys
        .encode(id, "test@embodiment.test", "Embodiment test", "user", 1)
        .unwrap()
}

async fn completed_effect(client: &reqwest::Client, base: &str, token: &str, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let effect = skills::response(
                client
                    .get(format!("{base}/effects/{id}"))
                    .bearer_auth(token),
                200,
            )
            .await;
            if !matches!(
                effect["status"].as_str(),
                Some("proposed" | "approved" | "dispatched")
            ) {
                assert_eq!(effect["status"], "succeeded", "{effect}");
                return effect;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Execution must finish independently of physical completion")
}

async fn observed_operation(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    id: &str,
    status: &str,
) -> Value {
    // Real PostgreSQL and the service's real observation cadence; no virtual
    // clock can expire database deadlines before their I/O has completed.
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let operation = skills::response(
                client
                    .get(format!("{base}/embodiment/operations/{id}"))
                    .bearer_auth(token),
                200,
            )
            .await;
            if operation["status"] == status && operation["last_confirmed_status"] == status {
                return operation;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("adapter evidence must reach the durable operation")
}

async fn resource_count(state: &AppState, id: &str) -> i64 {
    state
        .db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS count FROM embodiment_operation_resources WHERE operation_id=$1",
            [Uuid::parse_str(id).unwrap().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "count")
        .unwrap()
}

#[tokio::test]
async fn physical_operation_http_separates_execution_cancel_and_positive_stop_evidence() {
    with_fixture(|f| async move {
        let owner = Uuid::new_v4();
        let adapter = Arc::new(PhysicalFixture::new(owner));
        let start_name = adapter.descriptor.start_tool_name();
        let cancel_name = adapter.descriptor.cancel_tool_name();
        let runtime = f.builder().with_embodiment_adapter(adapter.clone()).build().await.unwrap();
        let state = runtime.state().clone();
        let base = format!("http://{}/api/v1", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let token = fixed_account(&state, owner).await;
            let (_, other) = delivery::account(&state, "user").await;
            let client = skills::client();
            for name in [&start_name, &cancel_name] {
                let definition = skills::response(client.get(format!("{base}/tools/{name}")).bearer_auth(&token), 200).await;
                assert_eq!(definition["require_confirmation"], true);
                skills::response(client.get(format!("{base}/tools/{name}")).bearer_auth(&other), 404).await;
            }
            let request = json!({"request_id":Uuid::new_v4(),"tool_name":start_name,"arguments":{"destination":"dock"}});
            skills::response(client.post(format!("{base}/tools/execute")).bearer_auth(&other).json(&request), 403).await;
            let proposed = skills::response(client.post(format!("{base}/tools/execute")).bearer_auth(&token).json(&request), 200).await;
            assert_eq!(proposed["status"], "proposed");
            assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);
            assert_eq!(skills::response(client.get(format!("{base}/embodiment/operations")).bearer_auth(&token), 200).await, json!([]));
            let effect_id = proposed["effect_id"].as_str().unwrap();
            skills::response(client.post(format!("{base}/effects/{effect_id}/confirm")).bearer_auth(&token).json(&json!({"intent_digest":proposed["intent_digest"]})), 200).await;
            let started = completed_effect(&client, &base, &token, effect_id).await;
            let receipt = &started["completion"]["receipt"]["output"];
            assert_eq!(receipt["status"], "submitting");
            assert!(receipt["stopped"].is_null());
            let operation_id = receipt["identity"]["operation_id"].as_str().unwrap();
            assert_ne!(operation_id, effect_id);
            let accepted = observed_operation(&client, &base, &token, operation_id, "accepted").await;
            assert_eq!(accepted["start_effect_id"], effect_id);
            assert_eq!(accepted["identity"]["owner_id"], owner.to_string());
            assert!(accepted["stopped"].is_null());
            assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
            assert_eq!(resource_count(&state, operation_id).await, 1);
            let own_list = skills::response(client.get(format!("{base}/embodiment/operations")).bearer_auth(&token), 200).await;
            assert_eq!(own_list.as_array().unwrap().len(), 1);
            assert_eq!(own_list[0]["identity"]["operation_id"], operation_id);
            assert_eq!(skills::response(client.get(format!("{base}/embodiment/operations")).bearer_auth(&other), 200).await, json!([]));
            skills::response(client.get(format!("{base}/embodiment/operations/{operation_id}")).bearer_auth(&other), 404).await;
            let replay = skills::response(client.post(format!("{base}/tools/execute")).bearer_auth(&token).json(&request), 200).await;
            assert_eq!(replay["effect_id"], started["effect_id"]);
            assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);

            let cancellation = json!({"request_id":Uuid::new_v4(),"tool_name":cancel_name,"arguments":{"operation_id":operation_id}});
            skills::response(client.post(format!("{base}/tools/execute")).bearer_auth(&other).json(&cancellation), 403).await;
            let proposed_cancel = skills::response(client.post(format!("{base}/tools/execute")).bearer_auth(&token).json(&cancellation), 200).await;
            assert_eq!(proposed_cancel["status"], "proposed");
            assert_eq!(adapter.cancels.load(Ordering::SeqCst), 0);
            assert_ne!(proposed_cancel["effect_id"], started["effect_id"]);
            let cancel_effect_id = proposed_cancel["effect_id"].as_str().unwrap();
            skills::response(client.post(format!("{base}/effects/{cancel_effect_id}/confirm")).bearer_auth(&token).json(&json!({"intent_digest":proposed_cancel["intent_digest"]})), 200).await;
            let cancelled_effect = completed_effect(&client, &base, &token, cancel_effect_id).await;
            assert_eq!(cancelled_effect["completion"]["receipt"]["output"]["status"], "cancelling");
            assert!(cancelled_effect["completion"]["receipt"]["output"]["stopped"].is_null());
            let cancelling = observed_operation(&client, &base, &token, operation_id, "cancelling").await;
            assert!(cancelling["stopped"].is_null());
            assert_eq!(adapter.cancels.load(Ordering::SeqCst), 1);
            assert_eq!(resource_count(&state, operation_id).await, 1, "cancel acknowledgement cannot release a physical resource");

            adapter.state.lock().unwrap().stop_proven = true;
            let stopped = observed_operation(&client, &base, &token, operation_id, "cancelled").await;
            assert_eq!(stopped["stopped"]["evidence"]["operation_controller"], "stopped");
            assert_eq!(resource_count(&state, operation_id).await, 0);
            let original = completed_effect(&client, &base, &token, effect_id).await;
            assert_eq!(original["completion"], started["completion"], "physical progress cannot rewrite a completed Execution receipt");
            let original_cancel = completed_effect(&client, &base, &token, cancel_effect_id).await;
            assert_eq!(original_cancel["completion"], cancelled_effect["completion"]);
            assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
            assert_eq!(adapter.cancels.load(Ordering::SeqCst), 1);
        }).catch_unwind().await;
        stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(10), &mut serving).await;
        if joined.is_err() { serving.abort(); let _ = serving.await; }
        joined.expect("physical operation runtime must join").unwrap().unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }).await;
}
