//! Actual automation producers, Execution workers and PostgreSQL privacy fence.
use super::*;
use alia_server::event::ServerEvent;
use alia_server::modules::execution::repository::ExecutionRepository;
use alia_server::modules::execution::types::EffectStatus;
use alia_server::providers::adapter::{
    ActionContext, ActionExecutor, AutomationRule, EventSubscription, ScheduleConfig, ScheduledTask,
};
use alia_server::scheduler::action::ToolCallActionExecutor;
use alia_server::scheduler::condition::{ConditionContext, ConditionEvaluator};
use alia_server::state::AppState;
use sea_orm::{DatabaseBackend, Statement};
use serde_json::{Value, json};

struct SlowCondition {
    entered: Semaphore,
    release: Semaphore,
}
impl Default for SlowCondition {
    fn default() -> Self {
        Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }
}
#[async_trait]
impl ConditionEvaluator for SlowCondition {
    fn condition_type(&self) -> &'static str {
        "source_epoch_gate"
    }
    async fn evaluate(&self, _: &Value, _: &ConditionContext) -> anyhow::Result<bool> {
        self.entered.add_permits(1);
        self.release.acquire().await?.forget();
        Ok(true)
    }
}

struct ObservedAction {
    inner: ToolCallActionExecutor,
    sources: Mutex<Vec<(Uuid, Option<i64>)>>,
    done: Semaphore,
}
#[async_trait]
impl ActionExecutor for ObservedAction {
    fn action_type(&self) -> &str {
        "observed_memory_action"
    }
    async fn execute(&self, config: &Value, ctx: &ActionContext) -> anyhow::Result<Value> {
        self.sources
            .lock()
            .unwrap()
            .push((ctx.user_id, ctx.source_privacy_epoch));
        let result = self.inner.execute(config, ctx).await;
        self.done.add_permits(1);
        result
    }
}

#[derive(Clone, Copy)]
enum Producer {
    Timer,
    Subscription,
    Rule,
}

async fn create_user(state: &AppState) -> Uuid {
    let user = Uuid::new_v4();
    state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Source epoch','unused','user','active')",
        vec![user.into(), format!("{user}@test.invalid").into()])).await.unwrap();
    user
}

async fn queue_source(state: &AppState, owner: Uuid, producer: Producer) {
    let id = Uuid::new_v4();
    let action = json!({"type":"observed_memory_action", "tool_name":"memory_store", "arguments":{"content":"Remember this accepted automation source."}});
    let conditions = json!({"type":"source_epoch_gate"});
    let pattern = json!({"type":"DeviceOnline", "payload":{"device_id":id.to_string()}});
    match producer {
        Producer::Timer => {
            state
                .modules
                .scheduler
                .create_task(ScheduledTask {
                    id,
                    user_id: owner,
                    name: "source epoch timer".into(),
                    enabled: true,
                    schedule: ScheduleConfig {
                        schedule_type: "once".into(),
                        cron_expression: None,
                        run_at: Some(
                            (chrono::Utc::now() + chrono::Duration::milliseconds(100)).to_rfc3339(),
                        ),
                        every_sec: None,
                        poll_interval_sec: None,
                    },
                    conditions,
                    action,
                    cooldown_sec: 0,
                    max_retries: 0,
                    created_by: "test".into(),
                })
                .await
                .unwrap();
        }
        Producer::Subscription => {
            state
                .modules
                .scheduler
                .subscribe_event(EventSubscription {
                    id,
                    user_id: owner,
                    name: "source epoch subscription".into(),
                    enabled: true,
                    event_pattern: pattern,
                    conditions,
                    action,
                    cooldown_sec: 0,
                    created_by: "test".into(),
                })
                .await
                .unwrap();
            state.event_bus.publish(ServerEvent::DeviceOnline {
                device_id: id.to_string(),
            });
        }
        Producer::Rule => {
            state
                .modules
                .rule_engine
                .create_rule(AutomationRule {
                    id,
                    user_id: owner,
                    name: "source epoch rule".into(),
                    enabled: true,
                    event_pattern: pattern,
                    conditions,
                    action,
                    cooldown_sec: 0,
                    created_by: "test".into(),
                })
                .await
                .unwrap();
            state.event_bus.publish(ServerEvent::DeviceOnline {
                device_id: id.to_string(),
            });
        }
    }
}

async fn old_source_cannot_refresh_after_slow_condition(producer: Producer) {
    with_fixture(|mut f| async move {
        f.config.policies.memory.enabled = true;
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let owner = create_user(&state).await;
        let before = state.modules.memory.write_snapshot(owner).await.unwrap();
        let condition = Arc::new(SlowCondition::default());
        state.conditions.register(condition.clone()).await;
        let action = Arc::new(ObservedAction {
            inner: ToolCallActionExecutor::new(state.modules.tools.clone()),
            sources: Mutex::new(Vec::new()),
            done: Semaphore::new(0),
        });
        state.actions.register(action.clone()).await;

        queue_source(&state, owner, producer).await;
        signalled(&condition.entered).await;
        state.modules.memory.clear(owner).await.unwrap();
        let after = state.modules.memory.write_snapshot(owner).await.unwrap();
        assert!(after.privacy_epoch > before.privacy_epoch);
        condition.release.add_permits(1);
        signalled(&action.done).await;
        assert_eq!(
            action.sources.lock().unwrap().as_slice(),
            &[(owner, Some(before.privacy_epoch))]
        );
        let repository = ExecutionRepository::new(f.db.clone());
        let records = repository.list(owner, None, 10, None).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source_privacy_epoch, Some(before.privacy_epoch));
        assert_eq!(
            records[0].status,
            EffectStatus::Unknown,
            "{:?}",
            records[0].completion
        );
        assert!(
            serde_json::to_string(&records[0].completion)
                .unwrap()
                .contains("memory source privacy epoch changed")
        );
        assert!(
            state
                .modules
                .memory
                .list(owner, Default::default())
                .await
                .unwrap()
                .is_empty()
        );

        // A separate occurrence can be admitted in the new epoch. Reusing the
        // previous queued context was forbidden; fresh trusted input is allowed.
        queue_source(&state, owner, producer).await;
        signalled(&condition.entered).await;
        condition.release.add_permits(1);
        signalled(&action.done).await;
        assert_eq!(
            action.sources.lock().unwrap().as_slice(),
            &[
                (owner, Some(before.privacy_epoch)),
                (owner, Some(after.privacy_epoch))
            ]
        );
        let records = repository.list(owner, None, 10, None).await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0].status,
            EffectStatus::Succeeded,
            "{:?}",
            records[0].completion
        );
        assert_eq!(
            state
                .modules
                .memory
                .list(owner, Default::default())
                .await
                .unwrap()
                .len(),
            1
        );
        runtime.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn timer_captures_source_before_condition_and_action_queue() {
    old_source_cannot_refresh_after_slow_condition(Producer::Timer).await;
}
#[tokio::test]
async fn subscription_captures_source_before_condition_and_action_queue() {
    old_source_cannot_refresh_after_slow_condition(Producer::Subscription).await;
}
#[tokio::test]
async fn rule_captures_source_before_slow_condition() {
    old_source_cannot_refresh_after_slow_condition(Producer::Rule).await;
}

#[tokio::test]
async fn unowned_system_action_does_not_acquire_memory_authority() {
    with_fixture(|mut f| async move {
        f.config.policies.memory.enabled = true;
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let condition = Arc::new(SlowCondition::default());
        state.conditions.register(condition.clone()).await;
        let action = Arc::new(ObservedAction {
            inner: ToolCallActionExecutor::new(state.modules.tools.clone()),
            sources: Mutex::new(Vec::new()),
            done: Semaphore::new(0),
        });
        state.actions.register(action.clone()).await;
        queue_source(&state, Uuid::nil(), Producer::Rule).await;
        signalled(&condition.entered).await;
        condition.release.add_permits(1);
        signalled(&action.done).await;
        assert_eq!(
            action.sources.lock().unwrap().as_slice(),
            &[(Uuid::nil(), None)]
        );
        let count = state
            .db
            .query_one(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT count(*)::bigint AS n FROM execution_effects".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count.try_get::<i64>("", "n").unwrap(), 0);
        runtime.shutdown().await.unwrap();
    })
    .await;
}
