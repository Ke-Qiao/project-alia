use super::*;
use crate::modules::companion::repository::tests::with_fixture;
use futures::poll;
use sea_orm::{ConnectOptions, Database, DatabaseBackend, Statement};
use std::time::Duration;

fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}

async fn registry(db: DatabaseConnection) -> Arc<SkillRegistry> {
    db.execute_unprepared(
        "INSERT INTO skills(name,display_name,category,intent_patterns,prompt_template) \
         VALUES('commit_fixture','Commit fixture','test','[\"commitfixture\"]','before')",
    )
    .await
    .unwrap();
    let registry = Arc::new(SkillRegistry::with_runtime(
        db.clone(),
        Arc::new(ArtifactRegistry::new(db)),
        None,
    ));
    registry.reload().await.unwrap();
    registry
}

async fn update(registry: &Arc<SkillRegistry>) -> (SkillChange, i32) {
    let change = registry.begin_change().await.unwrap();
    let pid = change
        .transaction()
        .query_one(sql("SELECT pg_backend_pid() AS pid", vec![]))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();
    change
        .transaction()
        .execute_unprepared("UPDATE skills SET prompt_template='after' WHERE name='commit_fixture'")
        .await
        .unwrap();
    (change, pid)
}

async fn prompt(registry: &SkillRegistry) -> String {
    registry
        .resolve(&SkillMatch {
            name: "commit_fixture".into(),
            confidence: 1.0,
        })
        .await
        .unwrap()
        .system_prompt
        .unwrap()
}

async fn install_commit_trigger(db: &DatabaseConnection, key: i64, reject: bool) {
    let reject = if reject {
        "RAISE EXCEPTION 'injected deferred commit rejection';"
    } else {
        ""
    };
    db.execute_unprepared(&format!(
        "CREATE FUNCTION block_skill_commit() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN PERFORM pg_advisory_xact_lock({key}); {reject} RETURN NEW; END $$; \
         CREATE CONSTRAINT TRIGGER block_skill_commit AFTER UPDATE ON skills \
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION block_skill_commit()"
    ))
    .await
    .unwrap();
}

async fn wait_for_commit(db: &DatabaseConnection, pid: i32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = db
                .query_one(sql(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 \
                     AND query='COMMIT' AND wait_event='advisory') AS waiting",
                    vec![pid.into()],
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "waiting")
                .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("owned COMMIT never reached the deferred trigger");
}

#[tokio::test]
async fn cancelled_commit_waiter_keeps_publication_owned_and_stop_waits_for_it() {
    with_fixture(|f| async move {
        let registry = registry(f.db.clone()).await;
        let key = Uuid::new_v4().as_u128() as i64;
        install_commit_trigger(&f.db, key, false).await;
        let blocker = f.db.begin().await.unwrap();
        blocker
            .execute(sql("SELECT pg_advisory_xact_lock($1)", vec![key.into()]))
            .await
            .unwrap();
        let (change, pid) = update(&registry).await;
        let waiter = tokio::spawn(change.commit());
        wait_for_commit(&f.db, pid).await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());

        let stopping = registry.stop();
        tokio::pin!(stopping);
        assert!(poll!(stopping.as_mut()).is_pending());
        blocker.rollback().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), stopping)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(prompt(&registry).await, "after");
        let stored = skill::Entity::find()
            .filter(skill::Column::Name.eq("commit_fixture"))
            .one(&f.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.prompt_template.as_deref(), Some("after"));
        assert_eq!(registry.commits.active(), 0);
        assert!(registry.begin_change().await.is_err());
        assert!(registry.reload().await.is_err());
    })
    .await;
}

#[tokio::test]
async fn rejected_commit_rebuilds_authoritative_index_and_keeps_original_database_error() {
    with_fixture(|f| async move {
        let registry = registry(f.db.clone()).await;
        install_commit_trigger(&f.db, Uuid::new_v4().as_u128() as i64, true).await;
        let (change, _) = update(&registry).await;
        let error = change.commit().await.unwrap_err();
        assert!(error.downcast_ref::<sea_orm::DbErr>().is_some());
        assert_eq!(prompt(&registry).await, "before");
        assert!(matches!(
            registry.health_check().await,
            ModuleHealth::Healthy
        ));
        registry.stop().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn failed_commit_and_failed_reconciliation_disable_index_until_successful_reload() {
    with_fixture(|f| async move {
        let schema: String =
            f.db.query_one(sql("SELECT current_schema() AS schema", vec![]))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "schema")
                .unwrap();
        let mut options = ConnectOptions::new(std::env::var("ALIA_TEST_DATABASE_URL").unwrap());
        options
            .max_connections(2)
            .sqlx_logging(false)
            .set_schema_search_path(format!("{schema},public"));
        let database = Database::connect(options).await.unwrap();
        let mut registry = registry(database.clone()).await;
        let key = Uuid::new_v4().as_u128() as i64;
        install_commit_trigger(&f.db, key, true).await;
        let blocker = f.db.begin().await.unwrap();
        blocker
            .execute(sql("SELECT pg_advisory_xact_lock($1)", vec![key.into()]))
            .await
            .unwrap();
        let (change, pid) = update(&registry).await;
        let waiter = tokio::spawn(change.commit());
        wait_for_commit(&f.db, pid).await;
        let pool = database.get_postgres_connection_pool().clone();
        let closing = database.close();
        tokio::pin!(closing);
        let closing_pending = match poll!(closing.as_mut()) {
            std::task::Poll::Ready(result) => {
                result.unwrap();
                false
            }
            std::task::Poll::Pending => true,
        };
        assert!(
            pool.is_closed(),
            "reconciliation must not acquire a new connection"
        );
        blocker.rollback().await.unwrap();
        let error = waiter.await.unwrap().unwrap_err();
        assert!(error.downcast_ref::<sea_orm::DbErr>().is_some());
        if closing_pending {
            closing.await.unwrap();
        }
        assert!(registry.match_skill("commitfixture").await.is_err());
        assert!(registry.list_skills().await.is_err());
        assert!(
            registry
                .resolve(&SkillMatch {
                    name: "commit_fixture".into(),
                    confidence: 1.0
                })
                .await
                .is_err()
        );
        assert!(matches!(
            registry.health_check().await,
            ModuleHealth::Degraded(_)
        ));

        tokio::time::timeout(Duration::from_secs(5), async {
            while registry.commits.active() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("commit owner did not finish after delivering its result");
        // Repair only the test's failed pool; production recovery uses the same reload path.
        let registry_mut = Arc::get_mut(&mut registry).expect("commit owner released its registry");
        registry_mut.db = Some(f.db.clone());
        registry_mut.artifacts = Some(Arc::new(ArtifactRegistry::new(f.db.clone())));
        registry.reload().await.unwrap();
        assert_eq!(prompt(&registry).await, "before");
        assert!(matches!(
            registry.health_check().await,
            ModuleHealth::Healthy
        ));
        registry.stop().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn stop_waits_for_prepared_change_and_closes_future_mutation_admission() {
    with_fixture(|f| async move {
        let registry = registry(f.db.clone()).await;
        let (change, _) = update(&registry).await;
        let stopping = registry.stop();
        tokio::pin!(stopping);
        assert!(poll!(stopping.as_mut()).is_pending());
        drop(change);
        stopping.await.unwrap();
        assert_eq!(prompt(&registry).await, "before");
        assert!(registry.begin_change().await.is_err());
        assert!(registry.reload().await.is_err());
    })
    .await;
}
