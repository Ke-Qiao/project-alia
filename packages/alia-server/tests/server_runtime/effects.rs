//! Lost HTTP replies can be reconciled without a live executor or new approval.
use super::*;
use alia_server::providers::adapter::ToolEntry;
use sea_orm::{DatabaseBackend, Statement};
use serde_json::{Value, json};

fn entry(revision: &str, calls: Arc<AtomicUsize>) -> Arc<ToolEntry> {
    let tool = rig_core::tool::DynamicTool::new_with_context(
        rig_core::message::ToolName::new("recovery_echo").unwrap(),
        "Receipt recovery fixture",
        json!({"type":"object"}),
        move |_, arguments| {
            let calls = calls.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(rig_core::tool::ToolOutput::json(arguments))
            })
        },
    );
    ToolEntry::dynamic(tool, "mcp:recovery", revision, true, None, Value::Null).unwrap()
}

#[tokio::test]
async fn request_recovery_reads_original_effect_after_tool_removal_without_reapproving() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let base = format!("http://{}/api/v1", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let (owner, token) = delivery::account(&state, "user").await;
            let (_, stranger) = delivery::account(&state, "user").await;
            let calls = Arc::new(AtomicUsize::new(0));
            state.modules.tools.replace_source_tools("mcp:recovery", vec![entry("v1", calls.clone())]).await.unwrap();
            let client = skills::client();
            let request = Uuid::new_v4();
            let payload = json!({"request_id":request,"tool_name":"recovery_echo","arguments":{"value":42}});
            let original = skills::response(client.post(format!("{base}/tools/execute")).bearer_auth(&token).json(&payload), 200).await;
            let id = original["effect_id"].as_str().unwrap();
            skills::response(client.post(format!("{base}/effects/{id}/confirm")).bearer_auth(&token).json(&json!({"intent_digest":original["intent_digest"]})), 200).await;
            let completed = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let current = skills::response(client.get(format!("{base}/effects/{id}")).bearer_auth(&token), 200).await;
                    if current["status"] == "succeeded" { break current; }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.unwrap();
            let pending_request = Uuid::new_v4();
            let pending = skills::response(client.post(format!("{base}/tools/execute")).bearer_auth(&token)
                .json(&json!({"request_id":pending_request,"tool_name":"recovery_echo","arguments":{"value":43}})), 200).await;
            state.modules.tools.replace_source_tools("mcp:recovery", vec![]).await.unwrap();
            skills::response(client.post(format!("{base}/tools/execute")).bearer_auth(&token).json(&payload), 400).await;
            let lookup = format!("{base}/effects?request_id={request}");
            assert_eq!(skills::response(client.get(&lookup).bearer_auth(&token), 200).await, json!([completed]));
            assert_eq!(skills::response(client.get(&lookup).bearer_auth(&stranger), 200).await, json!([]));
            skills::response(client.get(format!("{base}/effects/{id}")).bearer_auth(&stranger), 404).await;
            skills::response(client.get(format!("{base}/effects?request_id=invalid")).bearer_auth(&token), 400).await;
            assert_eq!(skills::response(client.get(format!("{lookup}&before={id}&limit=1")).bearer_auth(&token), 200).await, json!([]));

            // Reconnection changes implementation identity; reading cannot approve or dispatch the old proposal.
            state.modules.tools.replace_source_tools("mcp:recovery", vec![entry("v2", calls.clone())]).await.unwrap();
            assert_eq!(skills::response(client.get(format!("{base}/effects?request_id={pending_request}")).bearer_auth(&token), 200).await, json!([pending]));
            let pending_id = pending["effect_id"].as_str().unwrap();
            skills::response(client.post(format!("{base}/effects/{pending_id}/confirm")).bearer_auth(&token).json(&json!({"intent_digest":pending["intent_digest"]})), 200).await;
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let rejected = skills::response(client.get(format!("{base}/effects/{pending_id}")).bearer_auth(&token), 200).await;
                    if rejected["status"] == "failed" {
                        assert_eq!(rejected["completion"]["evidence"]["execution_started"], false);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "UPDATE users SET status='disabled' WHERE id=$1", [owner.into()])).await.unwrap();
            skills::response(client.get(&lookup).bearer_auth(&token), 401).await;
        }).catch_unwind().await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(10), serving).await.unwrap().unwrap().unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }).await;
}
