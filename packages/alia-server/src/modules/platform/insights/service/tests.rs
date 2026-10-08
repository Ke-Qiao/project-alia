use super::*;

#[cfg(all(test, feature = "insights"))]
mod deadline_tests {
    use super::*;
    use crate::modules::interaction::cache::{
        CacheCas, CacheReview, CacheStore, CacheWork, CachedResponse,
    };
    use crate::providers::adapter::{ChatMessage, EmbeddingAdapter};

    struct FixedMemoryLlm {
        response: String,
        calls: AtomicU32,
    }

    #[async_trait]
    impl ModuleLifecycle for FixedMemoryLlm {
        fn name(&self) -> &'static str {
            "fixed_insights_memory_llm"
        }
    }

    #[async_trait]
    impl LlmAdapter for FixedMemoryLlm {
        async fn chat(&self, _messages: &[ChatMessage]) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.response.clone())
        }

        async fn chat_stream(
            &self,
            _messages: &[ChatMessage],
        ) -> anyhow::Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<String>> + Send>>,
        > {
            anyhow::bail!("insights memory tests require the non-streaming call path")
        }
    }
    fn insight_user() -> MemoryMaintenanceUser {
        let user_id = Uuid::new_v4();
        MemoryMaintenanceUser {
            user_id,
            user_id_text: user_id.to_string(),
            channel_id: Some("insights-test".into()),
            session_id: None,
            last_seen_at: Utc::now(),
        }
    }

    struct ReviewCheckpoint {
        row: Mutex<CachedResponse>,
        approvals: AtomicU32,
    }
    #[async_trait]
    impl CacheStore for ReviewCheckpoint {
        async fn get(&self, _: Uuid, _: &str, _: &str) -> anyhow::Result<Option<CachedResponse>> {
            Ok(None)
        }
        async fn put(
            &self,
            _: crate::modules::interaction::cache::CacheEntry,
        ) -> anyhow::Result<CachedResponse> {
            anyhow::bail!("no new cache puts in deadline fixture")
        }
        async fn get_semantic_candidates(
            &self,
            _: Uuid,
            _: &str,
            _: &crate::providers::adapter::EmbeddingProfile,
            _: &crate::providers::adapter::EmbeddingVector,
            _: usize,
        ) -> anyhow::Result<Vec<crate::modules::interaction::cache::SemanticCacheCandidate>>
        {
            Ok(vec![])
        }
        async fn load_pending(
            &self,
            _: Uuid,
            _: Option<&str>,
            _: usize,
        ) -> anyhow::Result<Vec<CachedResponse>> {
            let row = self.row.lock().await;
            Ok(
                if row.status
                    == crate::modules::interaction::cache::CacheStatus::PendingInsightsReview
                {
                    vec![row.clone()]
                } else {
                    vec![]
                },
            )
        }
        async fn apply_review(
            &self,
            expected: &CachedResponse,
            review: CacheReview,
        ) -> anyhow::Result<CacheCas<Option<CachedResponse>>> {
            let mut row = self.row.lock().await;
            if row.version != expected.version {
                return Ok(CacheCas::Stale);
            }
            let CacheReview::Approve {
                response_text,
                metadata,
            } = review
            else {
                anyhow::bail!("fixture expects approval")
            };
            row.status = crate::modules::interaction::cache::CacheStatus::Active;
            row.version.revision += 1;
            row.response_text = response_text;
            row.metadata = metadata;
            self.approvals.fetch_add(1, Ordering::SeqCst);
            Ok(CacheCas::Applied(Some(row.clone())))
        }
        async fn authorize_work(&self, _: &CachedResponse, _: CacheWork) -> anyhow::Result<bool> {
            Ok(true)
        }
        async fn fill_embedding(
            &self,
            _: &CachedResponse,
            _: &crate::providers::adapter::EmbeddingProfile,
            _: &crate::providers::adapter::EmbeddingVector,
        ) -> anyhow::Result<CacheCas<()>> {
            anyhow::bail!("hanging inference must never write a vector")
        }
        async fn cleanup_expired(&self, _: usize) -> anyhow::Result<u64> {
            Ok(0)
        }
    }

    struct HangingEmbedding {
        entered: tokio::sync::Notify,
        dropped: CancellationToken,
    }
    #[async_trait]
    impl ModuleLifecycle for HangingEmbedding {
        fn name(&self) -> &'static str {
            "hanging_embedding"
        }
    }
    #[async_trait]
    impl EmbeddingAdapter for HangingEmbedding {
        async fn embed(&self, _: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
            anyhow::bail!("only profiled embedding is allowed")
        }
        async fn embed_profiled(
            &self,
            _: &[String],
        ) -> anyhow::Result<Option<crate::providers::adapter::ProfiledEmbeddings>> {
            let _owner = self.dropped.clone().drop_guard();
            self.entered.notify_one();
            std::future::pending().await
        }
        fn dimension(&self) -> usize {
            1536
        }
    }

    #[tokio::test(start_paused = true)]
    async fn hanging_phase_releases_its_permit_and_preserves_acknowledged_cache_approval() {
        use crate::modules::interaction::cache::{CacheStatus, CacheVersion};
        let cache = Arc::new(ReviewCheckpoint {
            row: Mutex::new(CachedResponse {
                version: CacheVersion {
                    cache_id: Uuid::new_v4(),
                    revision: 1,
                    context_id: Uuid::new_v4(),
                },
                owner_id: Uuid::new_v4(),
                scope: "default".into(),
                query_digest: "a".repeat(64),
                content_digest: "b".repeat(64),
                origin_response_id: Uuid::new_v4(),
                query_text: "Stable question".into(),
                response_text: "Stable answer".into(),
                metadata: serde_json::json!({}),
                status: CacheStatus::PendingInsightsReview,
                expires_at: Utc::now() + chrono::Duration::hours(1),
            }),
            approvals: AtomicU32::new(0),
        });
        let embedding = Arc::new(HangingEmbedding {
            entered: tokio::sync::Notify::new(),
            dropped: CancellationToken::new(),
        });
        let llm = Arc::new(FixedMemoryLlm {
            response: serde_json::json!({"decision":"approve","safe_direct_return":true,"memory_independent":true,"tool_free":true,"dynamic_free":true}).to_string(),
            calls: AtomicU32::new(0),
        });
        let semaphore = Arc::new(Semaphore::new(1));
        let runtime = CacheReviewRuntime {
            cache: Some(cache.clone()),
            embedding: Some(embedding.clone()),
        };
        let worker_runtime = runtime.clone();
        let worker_llm = llm.clone();
        let worker_semaphore = semaphore.clone();
        let task = tokio::spawn(async move {
            InsightService::bounded_phase(
                &worker_semaphore,
                InsightService::run_phase(
                    &DatabaseConnection::Disconnected,
                    Some(worker_llm),
                    None,
                    worker_runtime,
                    &InsightPhase::Consolidation,
                    &insight_user(),
                    &InsightsSection::default(),
                ),
            )
            .await
        });
        embedding.entered.notified().await;
        assert_eq!(cache.approvals.load(Ordering::SeqCst), 1);
        assert_eq!(semaphore.available_permits(), 0);
        tokio::time::advance(PHASE_TIMEOUT).await;
        let error = match task.await.unwrap() {
            Err(error) => error,
            Ok(_) => panic!("hanging phase cannot complete"),
        };
        assert!(error.to_string().contains("completion is unconfirmed"));
        assert!(embedding.dropped.is_cancelled());
        assert_eq!(semaphore.available_permits(), 1);
        assert_eq!(cache.row.lock().await.status, CacheStatus::Active);
        InsightService::bounded_phase(
            &semaphore,
            InsightService::run_phase(
                &DatabaseConnection::Disconnected,
                Some(llm.clone()),
                None,
                runtime,
                &InsightPhase::Consolidation,
                &insight_user(),
                &InsightsSection::default(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            llm.calls.load(Ordering::SeqCst),
            1,
            "an acknowledged approval must not be replayed"
        );
        assert_eq!(cache.approvals.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn stable_user_uuid_never_returns_nil_for_empty_text() {
        assert!(!stable_user_uuid("").is_nil());
        assert_eq!(
            stable_user_uuid("sample-user"),
            stable_user_uuid("sample-user")
        );
    }
}
#[cfg(all(test, feature = "insights"))]
mod lifecycle_tests {
    use super::*;
    use crate::providers::adapter::TaskFilter;
    use crate::scheduler::action::ActionRegistry;
    use crate::scheduler::{AutomationScheduler, ConditionRegistry};

    #[tokio::test]
    async fn failed_second_cron_registration_rolls_back_first_and_rejects_restart() {
        let scheduler = Arc::new(AutomationScheduler::with_registries(
            Arc::new(ConditionRegistry::new()),
            Arc::new(ActionRegistry::new()),
        ));
        let insights = InsightService::new(true).unwrap();
        let mut config = InsightsSection::default();
        config.enabled = true;
        config.habit_detection_schedule = "invalid cron".into();
        *insights.config.lock().await = Some(config);
        *insights.db.lock().await = Some(DatabaseConnection::Disconnected);
        *insights.event_bus.lock().await = Some(EventBus::new(8));
        *insights.cancel_token.lock().await = Some(CancellationToken::new());
        *insights.scheduler.lock().await = Some(scheduler.clone());
        assert!(insights.start().await.is_err());
        assert_eq!(
            scheduler
                .list_tasks(TaskFilter::default())
                .await
                .unwrap()
                .len(),
            1
        );
        insights.stop().await.unwrap();
        assert!(
            scheduler
                .list_tasks(TaskFilter::default())
                .await
                .unwrap()
                .is_empty()
        );
        insights.stop().await.unwrap();
        assert!(insights.start().await.is_err());
        assert!(insights.set_conditions(vec![]).await.is_err());
    }
}

#[cfg(test)]
mod availability_tests {
    use super::*;

    #[tokio::test]
    async fn policy_disabled_service_remains_inert_and_healthy() {
        let service = InsightService::new(false).unwrap();
        let owner = Uuid::new_v4();
        assert_eq!(service.name(), "noop");
        assert!(matches!(
            service.health_check().await,
            ModuleHealth::Healthy
        ));
        service.start().await.unwrap();
        service.stop().await.unwrap();
        service.start().await.unwrap();
        service
            .schedule(InsightTask {
                user_id: owner,
                phase: "invalid-but-disabled".into(),
                trigger: "manual".into(),
                context: serde_json::json!({}),
            })
            .await
            .unwrap();
        service
            .trigger(owner, InsightPhase::Extraction)
            .await
            .unwrap();
        service
            .set_conditions(vec![serde_json::json!({"never":true})])
            .await
            .unwrap();
        assert!(service.get_conditions().await.is_empty());
        let applied = service
            .apply_settings(&std::collections::BTreeMap::from([(
                "core.policies.insights.memory_extraction_prompt".into(),
                serde_json::json!("new prompt"),
            )]))
            .await
            .unwrap();
        assert!(applied.is_empty());
        let status = service.status(owner).await.unwrap();
        assert!(!status.enabled);
        assert!(status.last_phase.is_none());
        assert!(status.last_executed_at.is_none());
        assert_eq!(status.pending_tasks, 0);
        assert!(service.config.lock().await.is_none());
        assert!(service.db.lock().await.is_none());
        assert!(service.reflection_task_id.lock().await.is_none());
        assert!(service.habit_task_id.lock().await.is_none());
    }

    #[cfg(not(feature = "insights"))]
    #[test]
    fn enabling_insights_without_the_feature_is_rejected() {
        let error = InsightService::new(true)
            .err()
            .expect("feature is required");
        assert_eq!(
            error.to_string(),
            "enabled insights require feature 'insights'"
        );
    }

    #[cfg(feature = "insights")]
    #[tokio::test]
    async fn enabled_service_preserves_initialization_and_phase_errors() {
        let service = InsightService::new(true).unwrap();
        assert_eq!(service.name(), "scheduled");
        assert!(service.status(Uuid::new_v4()).await.unwrap().enabled);
        assert!(
            service
                .start()
                .await
                .unwrap_err()
                .to_string()
                .contains("not initialized")
        );
        assert!(
            service
                .schedule(InsightTask {
                    user_id: Uuid::new_v4(),
                    phase: "invalid".into(),
                    trigger: "manual".into(),
                    context: serde_json::json!({}),
                })
                .await
                .unwrap_err()
                .to_string()
                .contains("unknown insights phase")
        );
    }
}
