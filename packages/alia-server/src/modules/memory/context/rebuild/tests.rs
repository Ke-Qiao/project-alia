//! Real PostgreSQL claim fencing and atomic canonical/receipt mutations.
use super::*;
use crate::{
    config::ServerConfig,
    expression::{AppraisalEvent, ExpressionRuntime, rule_based_user_appraisals},
    modules::{memory::source::capture_source_epochs, platform::migrations},
    providers::embedding::NoopEmbedding,
};
use futures::FutureExt;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use std::{future::Future, panic::AssertUnwindSafe, path::Path, sync::Arc, time::Duration};

#[derive(Clone)]
struct Fixture {
    db: DatabaseConnection,
    admin: DatabaseConnection,
    schema: String,
    owner: Uuid,
    session: Uuid,
    runtime: Arc<ExpressionRuntime>,
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("rebuild tests require the disposable real PostgreSQL database");
        let mut options = ConnectOptions::new(url.clone());
        options.max_connections(1).sqlx_logging(false);
        let admin = Database::connect(options).await.unwrap();
        let schema = format!("appraisal_rebuild_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(6)
            .sqlx_logging(false)
            .set_schema_search_path(format!("{schema},public"));
        let db = match Database::connect(options).await {
            Ok(db) => db,
            Err(error) => {
                admin
                    .execute_unprepared(&format!("DROP SCHEMA {schema} CASCADE"))
                    .await
                    .unwrap();
                admin.close().await.unwrap();
                panic!("connecting isolated rebuild schema: {error}");
            }
        };
        let config = ServerConfig::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/server.example.toml"),
        )
        .unwrap();
        let fixture = Self {
            db,
            admin,
            schema,
            owner: Uuid::new_v4(),
            session: Uuid::new_v4(),
            runtime: Arc::new(ExpressionRuntime::from_config(&config)),
        };
        let setup = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(45), async {
            migrations::run(&fixture.db).await.unwrap();
            fixture.db.execute(stmt(
                "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Rebuild test','not-a-login','user','active')",
                vec![fixture.owner.into(), format!("{}@rebuild.test", fixture.owner).into()],
            )).await.unwrap();
            fixture.db.execute(stmt(
                "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'rebuild-device',$2)",
                vec![fixture.session.into(), fixture.owner.to_string().into()],
            )).await.unwrap();
            fixture.service().initialize_policy(ContextPolicy::from_server_config(&config).unwrap()).await.unwrap();
            fixture.runtime.attach_db(fixture.db.clone()).await.unwrap();
        })).catch_unwind().await;
        match setup {
            Ok(Ok(())) => fixture,
            other => {
                tokio::time::timeout(Duration::from_secs(5), fixture.runtime.shutdown())
                    .await
                    .unwrap()
                    .unwrap();
                fixture.cleanup().await;
                match other {
                    Err(error) => std::panic::resume_unwind(error),
                    _ => panic!("rebuild fixture setup exceeded 45 seconds"),
                }
            }
        }
    }

    fn service(&self) -> ContextService {
        ContextService::new(self.db.clone(), Arc::new(NoopEmbedding))
    }

    async fn seed(&self) -> AppraisalReceipt {
        let text = "thank you";
        let epochs = capture_source_epochs(&self.db, self.owner).await.unwrap();
        let source = Uuid::new_v4();
        let row = self.db.query_one(stmt(
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,created_at,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,clock_timestamp(),clock_timestamp(),$5,$6) RETURNING source_received_at",
            vec![source.into(), self.session.into(), self.owner.to_string().into(), text.into(), epochs.privacy_epoch.into(), epochs.relationship_epoch.into()],
        )).await.unwrap().unwrap();
        let generation = self
            .service()
            .materialize(
                ContextSource {
                    owner_id: self.owner,
                    request_id: Uuid::new_v4(),
                    channel_id: "rebuild-device".into(),
                    conversation_id: Some(self.session),
                    source_event_id: Some(source),
                    source_received_at: row.try_get("", "source_received_at").unwrap(),
                    source_privacy_epoch: epochs.privacy_epoch,
                },
                text,
            )
            .await
            .unwrap();
        let input = self
            .service()
            .prepare_user_appraisal(&generation)
            .await
            .unwrap();
        self.runtime
            .submit_appraisal(input.clone(), drafts(&input))
            .await
            .unwrap()
    }

    async fn appraisal(&self, id: Uuid) -> Option<Value> {
        self.db
            .query_one(stmt(
                "SELECT to_jsonb(a) AS document FROM appraisal_records a WHERE appraisal_id=$1",
                vec![id.into()],
            ))
            .await
            .unwrap()
            .map(|row| row.try_get("", "document").unwrap())
    }

    async fn item(&self, operation: Uuid) -> Value {
        self.db
            .query_one(stmt(
                "SELECT to_jsonb(i) AS document FROM appraisal_rebuild_items i WHERE operation_id=$1",
                vec![operation.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "document")
            .unwrap()
    }

    async fn operation(&self, operation: Uuid) -> Value {
        self.db
            .query_one(stmt(
                "SELECT to_jsonb(o) AS document FROM appraisal_rebuild_operations o WHERE operation_id=$1",
                vec![operation.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "document")
            .unwrap()
    }

    async fn cleanup(self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            self.db.close().await.unwrap();
            self.admin
                .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .await
                .unwrap();
            self.admin.close().await.unwrap();
        })
        .await
        .expect("isolated rebuild schema cleanup exceeded 10 seconds");
    }
}

fn drafts(input: &AppraisalInput) -> Vec<AppraisalEvent> {
    let drafts =
        rule_based_user_appraisals(input.text(), input.effective_at(), &input.policy().affect);
    assert!(
        !drafts.is_empty(),
        "the fixture must interpret an actual user fact"
    );
    drafts
}

fn assert_revoked(error: &anyhow::Error) {
    assert!(
        matches!(
            error.downcast_ref::<ContextError>(),
            Some(ContextError::Revoked(_))
        ),
        "expected authority revocation, got {error:#}"
    );
}

async fn with_fixture<F, Fut>(shutdown_error: Option<&str>, test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let fixture = Fixture::new().await;
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(30),
        test(fixture.clone()),
    ))
    .catch_unwind()
    .await;
    let shutdown = tokio::time::timeout(Duration::from_secs(8), fixture.runtime.shutdown()).await;
    fixture.cleanup().await;
    match result {
        Err(error) => std::panic::resume_unwind(error),
        Ok(Err(_)) => panic!("rebuild test exceeded 30 seconds"),
        Ok(Ok(())) => {}
    }
    let shutdown = shutdown.expect("appraisal writer did not join within 8 seconds");
    match shutdown_error {
        Some(expected) => {
            let error =
                shutdown.expect_err("writer must retain the injected SQL failure after join");
            assert!(
                error.to_string().contains(expected),
                "unexpected shutdown failure: {error:#}"
            );
        }
        None => shutdown.unwrap(),
    }
}

#[tokio::test]
async fn expired_claim_takeover_rejects_late_token_and_commits_only_the_new_receipt() {
    with_fixture(None, |f| async move {
        let original = f.seed().await;
        let service = f.service();
        let operation = Uuid::new_v4();
        let begun = service.begin_rebuild(f.owner, operation, None, 1).await.unwrap();
        assert_eq!((begun.examined, begun.pending, begun.committed), (1, 1, 0));
        let old = service.claim_rebuild(f.owner, operation).await.unwrap().unwrap();
        let old_drafts = drafts(&old);
        let old_claim = old.rebuild.clone().unwrap();
        assert!(service.claim_rebuild(f.owner, operation).await.unwrap().is_none());

        assert_eq!(f.db.execute(stmt(
            "UPDATE appraisal_rebuild_items SET claim_until=clock_timestamp()-INTERVAL '1 second' WHERE operation_id=$1 AND source_event_id=$2 AND claim_token=$3",
            vec![operation.into(), old_claim.source.into(), old_claim.token.into()],
        )).await.unwrap().rows_affected(), 1);
        let new = service.claim_rebuild(f.owner, operation).await.unwrap().unwrap();
        let new_claim = new.rebuild.clone().unwrap();
        assert_ne!(old_claim.token, new_claim.token);
        assert_eq!(old_claim.source, new_claim.source);
        assert_eq!(old.id(), new.id());
        assert_eq!(old.lease.id, new.lease.id);
        assert_eq!(old.lease.valid_until, new.lease.valid_until);
        let claimed = f.item(operation).await;
        let error = f.runtime.submit_appraisal(old, old_drafts).await.unwrap_err();
        assert_revoked(&error);
        assert!(error.to_string().contains("rebuild claim"));
        assert_eq!(f.item(operation).await, claimed, "late owner must not retire the new claim");
        assert!(f.appraisal(new.id()).await.is_none());
        assert_eq!(f.appraisal(original.appraisal_id).await.unwrap()["status"], "active");

        let receipt = f.runtime.submit_appraisal(new.clone(), drafts(&new)).await.unwrap();
        assert_eq!(receipt.appraisal_id, new.id());
        let committed = f.item(operation).await;
        assert_eq!(committed["status"], "committed");
        assert_eq!(committed["receipt"], json!(receipt));
        assert!(committed["claim_token"].is_null() && committed["claim_until"].is_null());
        let old_record = f.appraisal(original.appraisal_id).await.unwrap();
        assert_eq!(old_record["status"], "superseded");
        assert_eq!(old_record["revision"], original.revision + 1);
        let new_record = f.appraisal(receipt.appraisal_id).await.unwrap();
        assert_eq!(new_record["status"], "active");
        assert_eq!(new_record["result_digest"], receipt.result_digest);
        assert!(service.claim_rebuild(f.owner, operation).await.unwrap().is_none());
        let page = service.begin_rebuild(f.owner, operation, None, 1).await.unwrap();
        assert_eq!((page.committed, page.pending), (1, 0));
        assert_eq!(json!(page.receipts), json!([receipt]));
    }).await;
}

#[tokio::test]
async fn receipt_failure_rolls_back_supersede_and_insert_and_cannot_refresh_revoked_intent() {
    with_fixture(Some("injected rebuild receipt failure"), |f| async move {
        let original = f.seed().await;
        let service = f.service();
        let operation = Uuid::new_v4();
        let begun = service.begin_rebuild(f.owner, operation, None, 1).await.unwrap();
        assert_eq!(begun.pending, 1);
        let input = service.claim_rebuild(f.owner, operation).await.unwrap().unwrap();
        let original_row = f.appraisal(original.appraisal_id).await.unwrap();
        let item = f.item(operation).await;
        let intent = f.operation(operation).await;
        f.db.execute_unprepared(
            "CREATE FUNCTION fail_rebuild_receipt() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN
               IF NEW.status='committed' THEN
                 IF NOT EXISTS(SELECT 1 FROM appraisal_records WHERE appraisal_id=(NEW.receipt->>'appraisal_id')::uuid AND status='active' AND context_id=NEW.context_id)
                    OR NOT EXISTS(SELECT 1 FROM appraisal_records WHERE source_event_id=NEW.source_event_id AND appraisal_id<>(NEW.receipt->>'appraisal_id')::uuid AND status='superseded' AND revision=2) THEN
                   RAISE EXCEPTION 'receipt fault did not reach the canonical insert and supersede';
                 END IF;
                 RAISE EXCEPTION 'injected rebuild receipt failure';
               END IF;
               RETURN NEW;
             END $$;
             CREATE TRIGGER fail_rebuild_receipt BEFORE UPDATE ON appraisal_rebuild_items
             FOR EACH ROW EXECUTE FUNCTION fail_rebuild_receipt();"
        ).await.unwrap();
        let error = f.runtime.submit_appraisal(input.clone(), drafts(&input)).await.unwrap_err();
        assert!(error.to_string().contains("injected rebuild receipt failure"), "{error:#}");
        assert!(error.downcast_ref::<sea_orm::DbErr>().is_some());
        assert_eq!(f.appraisal(original.appraisal_id).await.unwrap(), original_row);
        assert!(f.appraisal(input.id()).await.is_none());
        assert_eq!(f.item(operation).await, item);
        assert_eq!(f.operation(operation).await, intent);
        f.db.execute_unprepared(
            "DROP TRIGGER fail_rebuild_receipt ON appraisal_rebuild_items; DROP FUNCTION fail_rebuild_receipt();"
        ).await.unwrap();

        let current = service.policy_store().current().await.unwrap();
        let mut policy = current.policy;
        policy.appraisal.affect.baseline_trust = 0.375;
        let changed = service.policy_store().replace(current.version, policy).await.unwrap();
        assert_eq!(changed.version, begun.policy_version + 1);
        for error in [
            service.begin_rebuild(f.owner, operation, None, 1).await.unwrap_err(),
            service.claim_rebuild(f.owner, operation).await.unwrap_err(),
        ] {
            assert!(matches!(error.downcast_ref::<ContextError>(), Some(ContextError::PolicyChanged)), "{error:#}");
        }
        assert_eq!(f.operation(operation).await, intent, "retry cannot sign the old operation with the new policy");

        let before_clear = Uuid::new_v4();
        let current_page = service.begin_rebuild(f.owner, before_clear, None, 1).await.unwrap();
        assert_eq!(current_page.pending, 1);
        let pending = service.claim_rebuild(f.owner, before_clear).await.unwrap().unwrap();
        let cleared = service.clear_relationship(f.owner, Uuid::new_v4(), current_page.relationship_epoch).await.unwrap();
        assert_eq!(cleared.relationship_epoch, current_page.relationship_epoch + 1);
        assert_revoked(&service.begin_rebuild(f.owner, before_clear, None, 1).await.unwrap_err());
        assert_revoked(&service.claim_rebuild(f.owner, before_clear).await.unwrap_err());
        assert_revoked(&f.runtime.submit_appraisal(pending.clone(), drafts(&pending)).await.unwrap_err());
        let revoked = f.operation(before_clear).await;
        assert_eq!(revoked["revoked"], true);
        assert_eq!(revoked["policy_version"], changed.version);
        assert_eq!(revoked["relationship_epoch"], current_page.relationship_epoch);
        assert!(f.appraisal(original.appraisal_id).await.is_none());
        assert!(f.appraisal(pending.id()).await.is_none());
        assert!(f.item(before_clear).await["receipt"].is_null());
    }).await;
}
