use super::relationship_api::generation;
use super::*;
use alia_server::modules::memory::context::{ContextBinding, ContextUse};
use sea_orm::{DatabaseBackend, Statement};
use serde_json::{Value, json};

#[tokio::test]
async fn explicit_rebuild_pins_page_policy_expiry_and_receipts_through_retries() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let (_, stranger) = delivery::account(&state, "user").await;
        let original = generation(&state, owner, "thank you").await;
        let receipt = state.modules.expression.observe_context_user(&state.modules.context, &original).await.unwrap();
        let until: chrono::DateTime<chrono::Utc> = state.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT valid_until FROM appraisal_records WHERE appraisal_id=$1", [receipt.appraisal_id.into()],
        )).await.unwrap().unwrap().try_get("", "valid_until").unwrap();
        let old = state.modules.context.policy_store().current().await.unwrap();
        let mut policy = old.policy;
        policy.appraisal.affect.relationship_appraisal_ttl_sec *= 2;
        policy.appraisal.affect.session_appraisal_ttl_sec *= 2;
        policy.appraisal.affect.turn_appraisal_ttl_sec *= 2;
        let current = state.modules.context.policy_store().replace(old.version, policy).await.unwrap();
        let url = format!("http://{}/api/v1/expression/relationship/rebuild", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = skills::client();
            for body in [json!({}),json!({"operation_id":Uuid::nil(),"limit":1}),json!({"operation_id":Uuid::new_v4(),"limit":33}),json!({"operation_id":Uuid::new_v4(),"limit":1,"owner_id":owner})] {
                assert_eq!(client.post(&url).bearer_auth(&token).json(&body).send().await.unwrap().status(), 400);
            }
            let operation = Uuid::new_v4();
            let body = json!({"operation_id":operation,"after":null,"limit":1});
            let response = client.post(&url).bearer_auth(&token).json(&body).send().await.unwrap();
            assert_eq!(response.status(), 200);
            let report: Value = response.json().await.unwrap();
            assert_eq!(report["policy_version"], current.version);
            assert_eq!(report["examined"], 1);
            assert_eq!(report["committed"], 1);
            assert_eq!(report["pending"], 0);
            let id = Uuid::parse_str(report["receipts"][0]["appraisal_id"].as_str().unwrap()).unwrap();
            let row = state.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "SELECT a.context_id,a.request_id,a.source_event_id,a.source_privacy_epoch,a.effective_at,a.valid_until,c.purpose FROM appraisal_records a JOIN context_leases c ON c.context_id=a.context_id WHERE a.appraisal_id=$1", [id.into()],
            )).await.unwrap().unwrap();
            assert_eq!(row.try_get::<String>("", "purpose").unwrap(), "appraisal_rebuild");
            assert!(row.try_get::<chrono::DateTime<chrono::Utc>>("", "valid_until").unwrap() <= until, "policy changes cannot extend the old judgment's lifetime");
            assert_eq!(row.try_get::<chrono::DateTime<chrono::Utc>>("", "effective_at").unwrap().timestamp_micros(), original.source().source_received_at.timestamp_micros());
            let binding = ContextBinding { context_id: row.try_get("", "context_id").unwrap(), owner_id:owner,
                request_id: row.try_get("", "request_id").unwrap(), source_event_id: row.try_get("", "source_event_id").unwrap(), source_privacy_epoch: row.try_get("", "source_privacy_epoch").unwrap() };
            assert!(state.modules.context.validate(&binding, ContextUse::OutputAdmission).await.is_err());
            assert!(state.modules.context.validate(&binding, ContextUse::AiMemoryWrite).await.is_err());
            state.modules.context.validate(&binding, ContextUse::AppraisalWrite).await.unwrap();
            let next = generation(&state, owner, "thank you again").await;
            state.modules.expression.observe_context_user(&state.modules.context, &next).await.unwrap();
            let projected = state.modules.context.extend_affect(&next).await.unwrap();
            assert!(projected.affect().unwrap().appraisals().iter().any(|r|r.appraisal_id==id), "a valid rebuilt judgment participates in the next projection");
            let retried: Value = client.post(&url).bearer_auth(&token).json(&body).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(retried, report, "new facts never enter an already fixed page");
            assert_eq!(client.post(&url).bearer_auth(&token).json(&json!({"operation_id":operation,"limit":2})).send().await.unwrap().status(), 409);
            assert_eq!(client.post(&url).bearer_auth(&stranger).json(&body).send().await.unwrap().status(), 409);
            let mut policy = current.policy.clone(); policy.appraisal.affect.baseline_trust=0.375;
            state.modules.context.policy_store().replace(current.version, policy).await.unwrap();
            assert_eq!(client.post(&url).bearer_auth(&token).json(&body).send().await.unwrap().status(), 409);
        }).catch_unwind().await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving).await.unwrap().unwrap().unwrap();
        if let Err(error)=result {std::panic::resume_unwind(error);}
    }).await;
}
