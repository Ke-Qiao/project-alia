//! Real HTTP administration, PostgreSQL Execution and the Rust PDK artifact.
use super::*;
use base64::Engine;
use sea_orm::{DatabaseBackend, Statement};
use serde_json::{Value, json};

async fn execute(client: &reqwest::Client, base: &str, token: &str, request: Uuid) -> Value {
    let effect = skills::response(
        client
            .post(format!("{base}/tools/execute"))
            .bearer_auth(token)
            .json(&json!({
                "request_id":request,"tool_name":"wasm_fixture_echo","arguments":{"value":42}
            })),
        200,
    )
    .await;
    let id = effect["effect_id"].as_str().unwrap();
    if effect["status"] == "proposed" {
        skills::response(
            client
                .post(format!("{base}/effects/{id}/confirm"))
                .bearer_auth(token)
                .json(&json!({"intent_digest":effect["intent_digest"]})),
            200,
        )
        .await;
    }
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let current = skills::response(
                client
                    .get(format!("{base}/effects/{id}"))
                    .bearer_auth(token),
                200,
            )
            .await;
            if !matches!(
                current["status"].as_str(),
                Some("proposed" | "approved" | "dispatched")
            ) {
                assert_eq!(current["status"], "succeeded", "{current}");
                assert_eq!(
                    current["completion"]["receipt"]["output"],
                    json!({"value":42})
                );
                break current;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn wasm_admin_lifecycle_reaches_unified_execution_and_invalidates_old_grants() {
    with_fixture(|mut f| async move {
        f.config.plugins.wasm_dir = Some(f.dir.join("wasm").to_string_lossy().into_owned());
        f.config.policies.expression.external_plugin_timeout_ms = 10_000;
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let base = format!("http://{}/api/v1",runtime.http_addr());
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let assertions = AssertUnwindSafe(async {
            let (_, admin) = delivery::account(&state,"admin").await;
            let (owner, user) = delivery::account(&state,"user").await;
            let client = skills::client();
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/wasm-plugins/target/wasm32-unknown-unknown/release/alia_wasm_demo.wasm");
            let module = std::fs::read(path).expect("build the Rust PDK example before server runtime tests");
            let module = base64::engine::general_purpose::STANDARD.encode(module);
            let mut manifest = json!({"name":"fixture","version":"1","abi_version":1,
                "limits":{"memory_pages":256,"fuel":20000000,"initialization_fuel":2000000,"timeout_ms":10000,"max_input_bytes":65536,"max_output_bytes":65536},
                "expression":{"abi_version":3,"capabilities":{"appraisal_engine":true}},
                "tools":[{"name":"echo","description":"Fixture echo","export":"alia_tool","parameters":{"type":"object","required":["value"],"properties":{"value":{"type":"integer"}},"additionalProperties":false},"require_confirmation":false,"required_role":null}]
            });
            let upload = json!({"manifest":manifest,"module_base64":module});
            skills::response(client.post(format!("{base}/plugins/wasm")).bearer_auth(&user).json(&upload),403).await;
            let mut invalid_upload = upload.clone();
            invalid_upload["manifest"]["tools"][0]["export"] = json!("missing_export");
            skills::response(client.post(format!("{base}/plugins/wasm")).bearer_auth(&admin).json(&invalid_upload),400).await;
            assert_eq!(skills::response(client.get(format!("{base}/plugins/wasm")).bearer_auth(&admin),200).await,json!([]));
            let installed = skills::response(client.post(format!("{base}/plugins/wasm")).bearer_auth(&admin).json(&upload),200).await;
            assert_eq!(installed["enabled"],false);
            let pin = installed["implementation_pin"].as_str().unwrap().to_owned();
            skills::response(client.get(format!("{base}/tools/wasm_fixture_echo")).bearer_auth(&admin),404).await;
            skills::response(client.post(format!("{base}/plugins/wasm/fixture/enable")).bearer_auth(&admin).json(&json!({"expected_pin":"wrong"})),400).await;
            skills::response(client.post(format!("{base}/plugins/wasm/fixture/enable")).bearer_auth(&admin).json(&json!({"expected_pin":pin})),200).await;
            // Enabling execution does not grant access to private context.
            let unapproved = relationship_api::generation(&state,owner,"thanks").await;
            assert!(state.modules.expression.observe_context_user(&state.modules.context,&unapproved).await.is_err());
            let settings = client.get(format!("{base}/settings")).bearer_auth(&admin).send().await.unwrap();
            assert_eq!(settings.status(),200);
            let tag = settings.headers()["etag"].to_str().unwrap().to_owned();
            skills::response(client.patch(format!("{base}/settings")).bearer_auth(&admin).header("if-match",tag)
                .json(&json!({"core.policies.appraisal.approved_interpreters":{"fixture":pin}})),200).await;
            let approved = relationship_api::generation(&state,owner,"thanks").await;
            let receipt = state.modules.expression.observe_context_user(&state.modules.context,&approved).await.unwrap();
            let row = state.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "SELECT events FROM appraisal_records WHERE appraisal_id=$1 AND owner_id=$2",
                [receipt.appraisal_id.into(),owner.into()])).await.unwrap().unwrap();
            let events: Value = row.try_get("","events").unwrap();
            assert!(events.as_array().unwrap().iter().any(|event|event["kind"]=="external_positive_feedback"));
            let request = Uuid::new_v4();
            let first = execute(&client,&base,&admin,request).await;
            let repeated = execute(&client,&base,&admin,request).await;
            assert_eq!(first["effect_id"], repeated["effect_id"]);
            manifest["version"] = json!("2");
            let upgraded = skills::response(client.put(format!("{base}/plugins/wasm/fixture")).bearer_auth(&admin).json(&json!({"manifest":manifest,"module_base64":module,"expected_pin":pin})),200).await;
            let next_pin = upgraded["implementation_pin"].as_str().unwrap();
            assert_ne!(next_pin,pin);
            assert_eq!(upgraded["enabled"],false);
            skills::response(client.get(format!("{base}/tools/wasm_fixture_echo")).bearer_auth(&admin),404).await;
            skills::response(client.post(format!("{base}/plugins/wasm/fixture/enable")).bearer_auth(&admin).json(&json!({"expected_pin":pin})),400).await;
            skills::response(client.post(format!("{base}/plugins/wasm/fixture/enable")).bearer_auth(&admin).json(&json!({"expected_pin":next_pin})),200).await;
            // The durable private-input approval identifies the old generation.
            let stale_approval = relationship_api::generation(&state,owner,"thanks").await;
            assert!(state.modules.expression.observe_context_user(&state.modules.context,&stale_approval).await.is_err());
            let second = execute(&client,&base,&admin,Uuid::new_v4()).await;
            assert_ne!(first["tool_digest"],second["tool_digest"]);
            // Force grant deletion to fail after the runtime has closed. The
            // manager must rebuild a fresh catalog on enable, not claim that
            // the stale active name is still running.
            let grant = f.dir.join("wasm/fixture/grant");
            std::fs::remove_file(&grant).unwrap();
            std::fs::create_dir(&grant).unwrap();
            skills::response(client.post(format!("{base}/plugins/wasm/fixture/revoke")).bearer_auth(&admin).json(&json!({"expected_pin":next_pin})),400).await;
            assert_eq!(skills::response(client.get(format!("{base}/plugins/wasm")).bearer_auth(&admin),200).await[0]["enabled"],false);
            std::fs::remove_dir(&grant).unwrap();
            skills::response(client.post(format!("{base}/plugins/wasm/fixture/enable")).bearer_auth(&admin).json(&json!({"expected_pin":next_pin})),200).await;
            execute(&client,&base,&admin,Uuid::new_v4()).await;
            skills::response(client.post(format!("{base}/plugins/wasm/fixture/revoke")).bearer_auth(&admin).json(&json!({"expected_pin":next_pin})),200).await;
            skills::response(client.get(format!("{base}/tools/wasm_fixture_echo")).bearer_auth(&admin),404).await;
            skills::response(client.delete(format!("{base}/plugins/wasm/fixture")).bearer_auth(&admin).json(&json!({"expected_pin":next_pin})),200).await;
            let list = skills::response(client.get(format!("{base}/plugins/wasm")).bearer_auth(&admin),200).await;
            assert_eq!(list,json!([]));
        }).catch_unwind().await;
        stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(10), &mut serving).await;
        if joined.is_err() { serving.abort(); let _ = serving.await; }
        joined.expect("WASM server runtime must join").unwrap().unwrap();
        if let Err(error) = assertions { std::panic::resume_unwind(error); }
    }).await;
}
