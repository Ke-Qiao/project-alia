//! Actual HTTP, lifecycle and PostgreSQL shadow work; no fabricated authority.
use super::*;
use alia_server::modules::companion::types::{CompanionMode, CompanionPolicy};
use alia_server::providers::adapter::{ChatMessage, LlmAdapter};
use sea_orm::{DatabaseBackend, Statement};
use serde_json::{Value, json};

#[derive(Clone)]
struct ForbiddenInference(Arc<AtomicUsize>);
#[async_trait]
impl ModuleLifecycle for ForbiddenInference {
    fn name(&self) -> &'static str {
        "companion-no-inference"
    }
}
#[async_trait]
impl LlmAdapter for ForbiddenInference {
    async fn chat(&self, _: &[ChatMessage]) -> anyhow::Result<String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("shadow must never call inference")
    }
    async fn chat_stream(
        &self,
        _: &[ChatMessage],
    ) -> anyhow::Result<std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<String>> + Send>>>
    {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("shadow must never call streaming inference")
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test]
async fn companion_http_policy_is_owner_scoped_cas_and_survives_restart() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let (other, other_token) = delivery::account(&state, "user").await;
        let url = format!("http://{}/api/v1/companion", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            assert_eq!(
                client
                    .get(format!("{url}/policy"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                401
            );
            let initial: Value = client
                .get(format!("{url}/policy"))
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(initial["revision"], 1);
            assert_eq!(initial["policy"]["mode"], "shadow");
            for mode in ["enabled", "unknown"] {
                let mut policy = initial["policy"].clone();
                policy["mode"] = json!(mode);
                assert_eq!(
                    client
                        .put(format!("{url}/policy"))
                        .bearer_auth(&token)
                        .json(&json!({"expected_revision":1,"policy":policy}))
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    400
                );
            }
            assert_eq!(
                client
                    .put(format!("{url}/policy"))
                    .bearer_auth(&token)
                    .json(
                        &json!({"expected_revision":1,"policy":initial["policy"],"owner_id":other})
                    )
                    .send()
                    .await
                    .unwrap()
                    .status(),
                400
            );
            let mut disabled = initial["policy"].clone();
            disabled["mode"] = json!("disabled");
            let body = json!({"expected_revision":1,"policy":disabled});
            let updated: Value = client
                .put(format!("{url}/policy"))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(updated["revision"], 2);
            assert_eq!(
                client
                    .put(format!("{url}/policy"))
                    .bearer_auth(&token)
                    .json(&body)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                409
            );
            let untouched: Value = client
                .get(format!("{url}/policy"))
                .bearer_auth(&other_token)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(untouched["policy"]["mode"], "shadow");
            assert_eq!(untouched["revision"], 1);
        })
        .catch_unwind()
        .await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
        let restarted = f.builder().build().await.unwrap();
        let restored = restarted
            .state()
            .modules
            .companion
            .policy(owner)
            .await
            .unwrap();
        assert_eq!(restored.revision, 2);
        assert_eq!(restored.policy.mode, CompanionMode::Disabled);
        assert_eq!(
            restarted
                .state()
                .modules
                .companion
                .policy(other)
                .await
                .unwrap()
                .policy
                .mode,
            CompanionMode::Shadow
        );
        let companion = restarted.state().modules.companion.clone();
        restarted.shutdown().await.unwrap();
        assert!(companion.policy(owner).await.is_err());
    })
    .await;
}

#[tokio::test]
async fn companion_shadow_http_has_durable_retry_and_erasure_without_side_effects() {
    with_fixture(|mut f| async move {
        f.config.policies.memory.enabled = true;
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = f.builder().with_llm(ForbiddenInference(calls.clone())).build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let (_, other_token) = delivery::account(&state, "user").await;
        // The scanner may race this HTTP request; persisted source dedup and
        // operation identity must still hold. Quiet remains a hard suppression.
        let policy = state.modules.companion.policy(owner).await.unwrap();
        state.modules.companion.replace_policy(owner, policy.revision, CompanionPolicy { quiet: true, ..policy.policy }).await.unwrap();
        let original = relationship_api::generation(&state, owner, "thank you").await;
        let appraisal = state.modules.expression.observe_context_user(&state.modules.context, &original).await.unwrap();
        let url = format!("http://{}/api/v1/companion", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            let operation = Uuid::new_v4();
            let body = json!({"operation_id":operation,"appraisal_ids":[appraisal.appraisal_id]});
            for invalid in [
                json!({"operation_id":operation,"appraisal_ids":[]}),
                json!({"operation_id":operation,"appraisal_ids":[Uuid::nil()]}),
                json!({"operation_id":operation,"appraisal_ids":[appraisal.appraisal_id,appraisal.appraisal_id]}),
                json!({"operation_id":operation,"appraisal_ids":[appraisal.appraisal_id],"opportunity":{"activity":"idle"}}),
                json!({"operation_id":operation,"appraisal_ids":[appraisal.appraisal_id],"owner_id":owner}),
            ] {
                assert_eq!(client.post(format!("{url}/shadow")).bearer_auth(&token).json(&invalid).send().await.unwrap().status(), 400);
            }
            let (left, right) = tokio::join!(
                client.post(format!("{url}/shadow")).bearer_auth(&token).json(&body).send(),
                client.post(format!("{url}/shadow")).bearer_auth(&token).json(&body).send(),
            );
            let first: Value = left.unwrap().error_for_status().unwrap().json().await.unwrap();
            let duplicate: Value = right.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(first, duplicate);
            let replayed: Value = client.get(format!("{url}/replay/{operation}")).bearer_auth(&token).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(first, replayed);
            assert_eq!(client.get(format!("{url}/replay/{operation}")).bearer_auth(&other_token).send().await.unwrap().status(), 403);
            assert_eq!(client.post(format!("{url}/shadow")).bearer_auth(&token).json(&json!({"operation_id":operation,"appraisal_ids":[Uuid::new_v4()]})).send().await.unwrap().status(), 409);
            state.modules.context.clear_relationship(owner, Uuid::new_v4(), 0).await.unwrap();
            let erased = client.get(format!("{url}/replay/{operation}")).bearer_auth(&token).send().await.unwrap();
            assert_eq!(erased.status(), 410);
            let erased: Value = erased.json().await.unwrap();
            assert_eq!(erased["error"]["code"], "COMPANION_EVIDENCE_ERASED");
            for table in ["delivery_responses", "deliveries", "execution_effects"] {
                let row = state.db.query_one(Statement::from_string(DatabaseBackend::Postgres, format!("SELECT count(*) AS n FROM {table}"))).await.unwrap().unwrap();
                assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
            }
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres, "UPDATE users SET status='disabled' WHERE id=$1", [owner.into()])).await.unwrap();
            assert_eq!(client.get(format!("{url}/policy")).bearer_auth(&token).send().await.unwrap().status(), 401);
        }).catch_unwind().await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving).await.unwrap().unwrap().unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }).await;
}

async fn wait_for_scan(
    state: &alia_server::state::AppState,
    owner: Uuid,
    expected: usize,
) -> Vec<(Uuid, Value)> {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let rows = state.db.query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT operation_id,private_snapshot FROM companion_operations WHERE owner_id=$1 ORDER BY operation_id",
                [owner.into()],
            )).await.unwrap();
            if rows.len() == expected {
                return rows.into_iter().map(|row| (
                    row.try_get("", "operation_id").unwrap(),
                    row.try_get("", "private_snapshot").unwrap(),
                )).collect();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await;
    result.unwrap_or_else(|_| {
        panic!(
            "scanner did not produce {expected} receipts: {:?}",
            state.modules.companion.scan_progress()
        )
    })
}

#[tokio::test]
async fn companion_runtime_discovers_canonical_sources_and_joins_worker() {
    with_fixture(|f| async move {
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = f.builder().with_llm(ForbiddenInference(calls.clone())).build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, _) = delivery::account(&state, "user").await;
        let original = relationship_api::generation(&state, owner, "thank you").await;
        let appraisal = state.modules.expression.observe_context_user(&state.modules.context, &original).await.unwrap();
        let result = AssertUnwindSafe(wait_for_scan(&state, owner, 1)).catch_unwind().await;
        runtime.shutdown().await.unwrap();
        let first = match result { Ok(value) => value, Err(error) => std::panic::resume_unwind(error) };
        assert!(state.modules.companion.policy(owner).await.is_err());
        assert!(state.modules.companion.start().await.is_err());
        state.modules.companion.stop().await.unwrap();

        let restarted = f.builder().with_llm(ForbiddenInference(calls.clone())).build().await.unwrap();
        let state = restarted.state().clone();
        let result = AssertUnwindSafe(async {
            let new_source = relationship_api::generation(&state, owner, "thank you again").await;
            state.modules.expression.observe_context_user(&state.modules.context, &new_source).await.unwrap();
            // A second receipt is a progress witness for a real post-restart
            // scan, so the assertion does not rely on an arbitrary quiet sleep.
            let after = wait_for_scan(&state, owner, 2).await;
            assert!(after.contains(&first[0]), "restart changed the original immutable receipt");
            let row = state.db.query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT count(*)::bigint AS n FROM companion_processed_sources WHERE owner_id=$1 AND appraisal_id=$2",
                [owner.into(), appraisal.appraisal_id.into()],
            )).await.unwrap().unwrap();
            assert_eq!(row.try_get::<i64>("", "n").unwrap(), 1);
            for table in ["delivery_responses", "deliveries", "execution_effects"] {
                let row = state.db.query_one(Statement::from_string(DatabaseBackend::Postgres, format!("SELECT count(*)::bigint AS n FROM {table}"))).await.unwrap().unwrap();
                assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
            }
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }).catch_unwind().await;
        restarted.shutdown().await.unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }).await;
}

// Observe actual admission before racing shutdown with a blocked SQL request.
#[tokio::test]
async fn companion_stop_retains_inflight_public_request_until_its_caller_is_cancelled() {
    use sea_orm::TransactionTrait;

    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, _) = delivery::account(&state, "user").await;
        let companion = state.modules.companion.clone();

        // Disabled excludes automatic discovery, while an explicit shadow
        // operation remains a bounded, suppressed decision. The scanner cannot
        // race the one receipt whose absence this test is proving.
        let initial = companion.policy(owner).await.unwrap();
        companion
            .replace_policy(
                owner,
                initial.revision,
                CompanionPolicy {
                    mode: CompanionMode::Disabled,
                    ..initial.policy
                },
            )
            .await
            .unwrap();
        let original = relationship_api::generation(&state, owner, "thank you").await;
        let appraisal = state
            .modules
            .expression
            .observe_context_user(&state.modules.context, &original)
            .await
            .unwrap();
        let operation = Uuid::new_v4();
        let lock = state.db.begin().await.unwrap();
        lock.execute_unprepared("SELECT version FROM context_policy FOR UPDATE")
            .await
            .unwrap();

        let target = companion.clone();
        let request = tokio::spawn(async move {
            target
                .evaluate(owner, operation, vec![appraisal.appraisal_id])
                .await
        });

        let result = AssertUnwindSafe(async {
            tokio::time::timeout(Duration::from_secs(1), async {
                while companion.active_requests() != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("public request did not enter Companion admission");

            // The real SQL lock keeps the original request outstanding. A
            // bounded stop observer must expire without detaching its ownership.
            assert!(
                tokio::time::timeout(Duration::from_millis(50), companion.stop())
                    .await
                    .is_err(),
                "stop returned while a public request still owned admission"
            );
            assert_eq!(companion.active_requests(), 1);
            assert!(
                companion.policy(owner).await.is_err(),
                "stop must close new admission"
            );

            // Calling stop again must still observe and drain that same request.
            let mut stopping = Box::pin(companion.stop());
            assert!(
                tokio::time::timeout(Duration::from_millis(50), &mut stopping)
                    .await
                    .is_err(),
                "dropping the first stop observer lost its drain obligation"
            );
            request.abort();
            tokio::time::timeout(Duration::from_secs(1), async {
                while companion.active_requests() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("cancelled caller retained its admission token");
            tokio::time::timeout(Duration::from_secs(1), &mut stopping)
                .await
                .expect("stop did not finish after the original request was dropped")
                .unwrap();

            let row = state
                .db
                .query_one(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT count(*)::bigint AS n FROM companion_operations WHERE operation_id=$1",
                    [operation.into()],
                ))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
        })
        .catch_unwind()
        .await;

        // Release all fixture-owned blockers even on a behavioral red; retaining
        // the JoinHandle also proves no public-request task is left detached.
        request.abort();
        let request_result = tokio::time::timeout(Duration::from_secs(1), request)
            .await
            .expect("cancelled request task did not join");
        lock.rollback().await.unwrap();
        assert!(request_result.unwrap_err().is_cancelled());
        companion.stop().await.unwrap();
        // The service stop did not close its shared database dependency.
        state.db.execute_unprepared("SELECT 1").await.unwrap();
        let row = state
            .db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT count(*)::bigint AS n FROM companion_operations WHERE operation_id=$1",
                [operation.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
        runtime.shutdown().await.unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}

#[tokio::test]
async fn companion_public_request_capacity_rejects_ninth_and_cancel_releases_all_tokens() {
    use sea_orm::TransactionTrait;

    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, _) = delivery::account(&state, "user").await;
        let companion = state.modules.companion.clone();
        let initial = companion.policy(owner).await.unwrap();
        companion
            .replace_policy(
                owner,
                initial.revision,
                CompanionPolicy {
                    mode: CompanionMode::Disabled,
                    ..initial.policy
                },
            )
            .await
            .unwrap();

        // Requests that acquire pool connections wait on this actual authority
        // lock; the remaining requests wait on the pool while still admitted.
        let lock = state.db.begin().await.unwrap();
        lock.execute_unprepared("SELECT version FROM context_policy FOR UPDATE")
            .await
            .unwrap();
        let requests = (0..8)
            .map(|_| {
                let companion = companion.clone();
                tokio::spawn(async move { companion.policy(owner).await })
            })
            .collect::<Vec<_>>();
        let result = AssertUnwindSafe(async {
            tokio::time::timeout(Duration::from_secs(1), async {
                while companion.active_requests() != 8 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("eight requests did not enter Companion admission");

            let rejected =
                tokio::time::timeout(Duration::from_millis(100), companion.policy(owner))
                    .await
                    .expect("ninth request queued instead of rejecting capacity");
            assert!(matches!(
                rejected,
                Err(alia_server::modules::companion::repository::RepoError::Unavailable(message))
                    if message.contains("capacity")
            ));
            assert_eq!(companion.active_requests(), 8);
            for request in &requests {
                request.abort();
            }
            tokio::time::timeout(Duration::from_secs(1), async {
                while companion.active_requests() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("caller cancellation retained admission tokens");
        })
        .catch_unwind()
        .await;

        // Always abort/join all callers and release the SQL blocker, including
        // when an assertion detects a behavioral failure. Defer assertions on
        // JoinHandle results until the fixture's SQL lock has been released.
        for request in &requests {
            request.abort();
        }
        let joined =
            tokio::time::timeout(Duration::from_secs(2), futures::future::join_all(requests)).await;
        lock.rollback().await.unwrap();
        let post_cancel = companion.policy(owner).await;
        companion.stop().await.unwrap();
        state.db.execute_unprepared("SELECT 1").await.unwrap();
        runtime.shutdown().await.unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
        assert!(
            post_cancel.is_ok(),
            "released slots did not admit a fresh request"
        );
        for request in joined.expect("cancelled request tasks did not join") {
            assert!(request.unwrap_err().is_cancelled());
        }
        assert_eq!(companion.active_requests(), 0);
    })
    .await;
}

#[tokio::test]
async fn companion_http_prewrite_timeout_preserves_original_operation_for_retry() {
    use sea_orm::TransactionTrait;

    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let initial = state.modules.companion.policy(owner).await.unwrap();
        state
            .modules
            .companion
            .replace_policy(
                owner,
                initial.revision,
                CompanionPolicy {
                    mode: CompanionMode::Disabled,
                    ..initial.policy
                },
            )
            .await
            .unwrap();
        let original = relationship_api::generation(&state, owner, "thank you").await;
        let appraisal = state
            .modules
            .expression
            .observe_context_user(&state.modules.context, &original)
            .await
            .unwrap();
        let operation = Uuid::new_v4();
        let body = json!({"operation_id":operation,"appraisal_ids":[appraisal.appraisal_id]});
        let url = format!("http://{}/api/v1/companion", runtime.http_addr());
        let lock = state.db.begin().await.unwrap();
        lock.execute_unprepared("SELECT version FROM context_policy FOR UPDATE")
            .await
            .unwrap();
        let mut policy_lock = Some(lock);
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            // The authority row remains locked throughout the whole request.
            // Either the two-second HTTP budget or the two-second PostgreSQL
            // statement budget can expire first; neither reaches any write.
            let timed_out = client
                .post(format!("{url}/shadow"))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(timed_out.status(), 503);
            let unconfirmed: Value = timed_out.json().await.unwrap();
            assert_eq!(unconfirmed["error"]["code"], "COMPANION_SHADOW_UNCONFIRMED");
            assert_eq!(unconfirmed["error"]["operation_id"], json!(operation));
            let before = state
                .db
                .query_one(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT count(*)::bigint AS n FROM companion_operations WHERE owner_id=$1",
                    [owner.into()],
                ))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(before.try_get::<i64>("", "n").unwrap(), 0);
            policy_lock.take().unwrap().rollback().await.unwrap();

            let accepted: Value = client
                .post(format!("{url}/shadow"))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            let retried: Value = client
                .post(format!("{url}/shadow"))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            let replayed: Value = client
                .get(format!("{url}/replay/{operation}"))
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(accepted["operation_id"], json!(operation));
            assert_eq!(accepted, retried);
            assert_eq!(accepted, replayed);
            for table in [
                "companion_operations",
                "companion_candidates",
                "companion_processed_sources",
            ] {
                let row = state
                    .db
                    .query_one(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres,
                        format!("SELECT count(*)::bigint AS n FROM {table} WHERE owner_id=$1"),
                        [owner.into()],
                    ))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    row.try_get::<i64>("", "n").unwrap(),
                    1,
                    "duplicate {table} row"
                );
            }
            for table in ["delivery_responses", "deliveries", "execution_effects"] {
                let row = state
                    .db
                    .query_one(Statement::from_string(
                        DatabaseBackend::Postgres,
                        format!("SELECT count(*)::bigint AS n FROM {table}"),
                    ))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
            }
        })
        .catch_unwind()
        .await;

        // This executes even if the 503 response or retry assertions fail.
        if let Some(lock) = policy_lock.take() {
            lock.rollback().await.unwrap();
        }
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .expect("HTTP runtime did not join after removing the SQL blocker")
            .unwrap()
            .unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}
