//! Event-driven automation rule engine.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::event::{EventBus, ServerEvent};
use crate::module::background::OwnedBackground;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    ActionContext, AutomationRule, RuleAction, RuleEvaluator, RuleFilter, RulePatch,
};
use crate::scheduler::action::{ActionRegistry, capture_action_sources};
use crate::scheduler::condition::{ConditionContext, ConditionRegistry};

#[derive(Debug, Clone)]
struct RuleState {
    rule: AutomationRule,
    last_triggered: Option<DateTime<Utc>>,
}

pub struct EventRuleEngine {
    background: OwnedBackground,
    rules: Arc<RwLock<HashMap<Uuid, RuleState>>>,
    event_bus: Mutex<Option<EventBus>>,
    db: Mutex<Option<DatabaseConnection>>,
    cancel_token: Mutex<Option<CancellationToken>>,
    conditions: Mutex<Option<Arc<ConditionRegistry>>>,
    actions: Mutex<Option<Arc<ActionRegistry>>>,
    max_rules_per_user: Mutex<u32>,
}

impl EventRuleEngine {
    pub fn new() -> Self {
        Self {
            background: OwnedBackground::default(),
            rules: Arc::new(RwLock::new(HashMap::new())),
            event_bus: Mutex::new(None),
            db: Mutex::new(None),
            cancel_token: Mutex::new(None),
            conditions: Mutex::new(None),
            actions: Mutex::new(None),
            max_rules_per_user: Mutex::new(50),
        }
    }

    pub fn with_registries(
        conditions: Arc<ConditionRegistry>,
        actions: Arc<ActionRegistry>,
    ) -> Self {
        Self {
            background: OwnedBackground::default(),
            rules: Arc::new(RwLock::new(HashMap::new())),
            event_bus: Mutex::new(None),
            db: Mutex::new(None),
            cancel_token: Mutex::new(None),
            conditions: Mutex::new(Some(conditions)),
            actions: Mutex::new(Some(actions)),
            max_rules_per_user: Mutex::new(50),
        }
    }

    async fn db(&self) -> Option<DatabaseConnection> {
        self.db.lock().await.clone()
    }

    async fn event_loop(
        rules: Arc<RwLock<HashMap<Uuid, RuleState>>>,
        event_bus: EventBus,
        cancel_token: CancellationToken,
        conditions: Arc<ConditionRegistry>,
        actions: Arc<ActionRegistry>,
        db: DatabaseConnection,
    ) {
        let mut rx = event_bus.subscribe();

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("rule engine event loop cancelled");
                    break;
                }
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            if let Err(error) = Self::handle_event(
                                &rules,
                                &event_bus,
                                &conditions,
                                &actions,
                                &db,
                                event,
                            ).await {
                                tracing::warn!(error = %error, "rule event handling failed");
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(lagged = n, "rule engine event loop lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::info!("event bus closed, stopping rule engine");
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn handle_event(
        rules: &Arc<RwLock<HashMap<Uuid, RuleState>>>,
        event_bus: &EventBus,
        conditions: &ConditionRegistry,
        actions: &ActionRegistry,
        db: &DatabaseConnection,
        event: ServerEvent,
    ) -> anyhow::Result<()> {
        let event_json = serde_json::to_value(&event).unwrap_or_default();
        let ctx = ConditionContext::new(Some(event_json.clone()));
        let now = Utc::now();

        let candidates = {
            let rules = rules.read().await;
            rules
                .iter()
                .filter_map(|(id, state)| {
                    if !state.rule.enabled
                        || !matches_pattern(&event_json, &state.rule.event_pattern)
                    {
                        return None;
                    }
                    if let Some(last) = state.last_triggered {
                        let cooldown = chrono::Duration::seconds(state.rule.cooldown_sec as i64);
                        if now - last < cooldown {
                            return None;
                        }
                    }
                    Some((*id, state.rule.clone()))
                })
                .collect::<Vec<_>>()
        };

        let source_epochs =
            capture_action_sources(db, candidates.iter().map(|(_, rule)| rule.user_id)).await;
        for (rule_id, rule) in candidates {
            let Some(source_privacy_epoch) = source_epochs.get(&rule.user_id).copied() else {
                continue;
            };
            let matched = conditions
                .evaluate_all(&rule.conditions, &ctx)
                .await
                .map_err(|error| {
                    anyhow::anyhow!("rule '{}' condition evaluation failed: {error}", rule.name)
                })?;
            if !matched {
                continue;
            }

            let action_type = rule
                .action
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            event_bus.publish(ServerEvent::RuleTriggered {
                rule_id,
                rule_name: rule.name.clone(),
                action: action_type,
            });

            let action_ctx = ActionContext {
                user_id: rule.user_id,
                request_id: Uuid::new_v4(),
                source_received_at: now,
                source_privacy_epoch,
                source: "rule".to_string(),
                trigger_id: Some(rule_id),
                trigger_name: Some(rule.name.clone()),
                event: Some(event_json.clone()),
            };
            if let Err(error) = actions.execute(&rule.action, &action_ctx).await {
                tracing::warn!(
                    rule_id = %rule_id,
                    rule_name = %rule.name,
                    error = %error,
                    "rule action failed"
                );
            }

            let mut rules = rules.write().await;
            if let Some(state) = rules.get_mut(&rule_id) {
                state.last_triggered = Some(Utc::now());
            }
        }

        Ok(())
    }
}

impl Default for EventRuleEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for EventRuleEngine {
    fn name(&self) -> &'static str {
        "event_rule_engine"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        *self.db.lock().await = Some(ctx.db.clone());
        *self.event_bus.lock().await = Some(ctx.event_bus.clone());
        *self.cancel_token.lock().await = Some(ctx.cancel_token.clone());
        *self.conditions.lock().await = ctx.conditions.clone();
        *self.actions.lock().await = ctx.actions.clone();
        *self.max_rules_per_user.lock().await = ctx.config.policies.rules.max_rules_per_user;
        let loaded = load_rules_from_db(&ctx.db).await?;
        *self.rules.write().await = loaded;
        tracing::info!("EventRuleEngine initialized");
        Ok(())
    }

    async fn start(&self) -> anyhow::Result<()> {
        let parent = self
            .cancel_token
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("rule engine not initialized"))?;
        let mut background = self.background.begin_start(&parent).await?;
        let event_bus = self
            .event_bus
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("rule engine not initialized"))?;
        let cancel_token = background.token();
        let conditions = self
            .conditions
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("rule engine missing condition registry"))?;
        let actions = self
            .actions
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("rule engine missing action registry"))?;
        let rules = self.rules.clone();
        let db = self
            .db()
            .await
            .ok_or_else(|| anyhow::anyhow!("rule engine database not initialized"))?;

        background.spawn_loop(
            "rule events",
            Self::event_loop(rules, event_bus, cancel_token, conditions, actions, db),
        );
        background.commit();
        tracing::info!("EventRuleEngine started");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.background.stop().await
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}

#[async_trait]
impl RuleEvaluator for EventRuleEngine {
    async fn evaluate(&self, event: &ServerEvent) -> anyhow::Result<Vec<RuleAction>> {
        let event_json = serde_json::to_value(event).unwrap_or_default();
        let ctx = ConditionContext::new(Some(event_json.clone()));
        let Some(conditions) = self.conditions.lock().await.clone() else {
            return Ok(Vec::new());
        };
        let rules = self.rules.read().await;
        let mut out = Vec::new();

        for state in rules.values() {
            if !state.rule.enabled || !matches_pattern(&event_json, &state.rule.event_pattern) {
                continue;
            }
            if conditions
                .evaluate_all(&state.rule.conditions, &ctx)
                .await?
            {
                out.push(RuleAction {
                    rule_id: state.rule.id,
                    rule_name: state.rule.name.clone(),
                    action_type: state
                        .rule
                        .action
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    payload: state.rule.action.clone(),
                });
            }
        }

        Ok(out)
    }

    async fn reload(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn create_rule(&self, rule: AutomationRule) -> anyhow::Result<Uuid> {
        let id = rule.id;
        let max_rules_per_user = *self.max_rules_per_user.lock().await;
        if max_rules_per_user > 0 {
            let rules = self.rules.read().await;
            let count = rules
                .values()
                .filter(|state| state.rule.user_id == rule.user_id)
                .count();
            if count >= max_rules_per_user as usize {
                anyhow::bail!("max rules per user exceeded");
            }
        }
        if let Some(db) = self.db().await {
            upsert_rule(&db, &rule).await?;
        }
        self.rules.write().await.insert(
            id,
            RuleState {
                rule,
                last_triggered: None,
            },
        );
        Ok(id)
    }

    async fn update_rule(&self, rule_id: Uuid, patch: RulePatch) -> anyhow::Result<()> {
        let mut rules = self.rules.write().await;
        let state = rules
            .get_mut(&rule_id)
            .ok_or_else(|| anyhow::anyhow!("rule not found: {rule_id}"))?;
        if let Some(name) = patch.name {
            state.rule.name = name;
        }
        if let Some(enabled) = patch.enabled {
            state.rule.enabled = enabled;
        }
        if let Some(event_pattern) = patch.event_pattern {
            state.rule.event_pattern = event_pattern;
        }
        if let Some(conditions) = patch.conditions {
            state.rule.conditions = conditions;
        }
        if let Some(action) = patch.action {
            state.rule.action = action;
        }
        if let Some(cooldown_sec) = patch.cooldown_sec {
            state.rule.cooldown_sec = cooldown_sec;
        }
        let rule = state.rule.clone();
        drop(rules);
        if let Some(db) = self.db().await {
            upsert_rule(&db, &rule).await?;
        }
        Ok(())
    }

    async fn delete_rule(&self, rule_id: Uuid) -> anyhow::Result<()> {
        self.rules
            .write()
            .await
            .remove(&rule_id)
            .ok_or_else(|| anyhow::anyhow!("rule not found: {rule_id}"))?;
        if let Some(db) = self.db().await {
            db.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "DELETE FROM automation_rules WHERE id = $1::uuid",
                vec![rule_id.to_string().into()],
            ))
            .await?;
        }
        Ok(())
    }

    async fn list_rules(&self, filter: RuleFilter) -> anyhow::Result<Vec<AutomationRule>> {
        let rules = self.rules.read().await;
        Ok(rules
            .values()
            .filter(|state| {
                if let Some(user_id) = filter.user_id
                    && state.rule.user_id != user_id
                {
                    return false;
                }
                if filter.enabled_only && !state.rule.enabled {
                    return false;
                }
                true
            })
            .map(|state| state.rule.clone())
            .collect())
    }

    async fn pause_rule(&self, rule_id: Uuid) -> anyhow::Result<()> {
        self.update_rule(
            rule_id,
            RulePatch {
                name: None,
                enabled: Some(false),
                event_pattern: None,
                conditions: None,
                action: None,
                cooldown_sec: None,
            },
        )
        .await
    }

    async fn resume_rule(&self, rule_id: Uuid) -> anyhow::Result<()> {
        self.update_rule(
            rule_id,
            RulePatch {
                name: None,
                enabled: Some(true),
                event_pattern: None,
                conditions: None,
                action: None,
                cooldown_sec: None,
            },
        )
        .await
    }
}

fn matches_pattern(event: &serde_json::Value, pattern: &serde_json::Value) -> bool {
    match (event, pattern) {
        (_, serde_json::Value::Null) => true,
        (serde_json::Value::Object(ev), serde_json::Value::Object(pat)) => pat
            .iter()
            .all(|(k, v)| ev.get(k).is_some_and(|ev_v| matches_pattern(ev_v, v))),
        (ev, pat) => ev == pat,
    }
}

async fn load_rules_from_db(db: &DatabaseConnection) -> anyhow::Result<HashMap<Uuid, RuleState>> {
    let rows = db
        .query_all(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT id::text, user_id::text, name, enabled, event_pattern,
                   conditions, action, cooldown_sec, created_by
            FROM automation_rules
            "#,
        ))
        .await?;
    let mut rules = HashMap::new();

    for row in rows {
        let id = Uuid::parse_str(&row.try_get::<String>("", "id")?)?;
        let user_id = Uuid::parse_str(&row.try_get::<String>("", "user_id")?)?;
        let cooldown_sec: i32 = row.try_get("", "cooldown_sec")?;
        rules.insert(
            id,
            RuleState {
                rule: AutomationRule {
                    id,
                    user_id,
                    name: row.try_get("", "name")?,
                    enabled: row.try_get("", "enabled")?,
                    event_pattern: row.try_get("", "event_pattern")?,
                    conditions: row.try_get("", "conditions")?,
                    action: row.try_get("", "action")?,
                    cooldown_sec: cooldown_sec.max(0) as u32,
                    created_by: row.try_get("", "created_by")?,
                },
                last_triggered: None,
            },
        );
    }

    Ok(rules)
}

async fn upsert_rule(db: &DatabaseConnection, rule: &AutomationRule) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"
        INSERT INTO automation_rules
            (id, user_id, name, enabled, event_pattern, conditions, action,
             cooldown_sec, created_by, updated_at)
        VALUES
            ($1::uuid, $2::uuid, $3, $4, $5::jsonb, $6::jsonb, $7::jsonb,
             $8, $9, NOW())
        ON CONFLICT (id) DO UPDATE SET
            user_id = EXCLUDED.user_id,
            name = EXCLUDED.name,
            enabled = EXCLUDED.enabled,
            event_pattern = EXCLUDED.event_pattern,
            conditions = EXCLUDED.conditions,
            action = EXCLUDED.action,
            cooldown_sec = EXCLUDED.cooldown_sec,
            created_by = EXCLUDED.created_by,
            updated_at = NOW()
        "#,
        vec![
            rule.id.to_string().into(),
            rule.user_id.to_string().into(),
            rule.name.clone().into(),
            rule.enabled.into(),
            rule.event_pattern.clone().into(),
            rule.conditions.clone().into(),
            rule.action.clone().into(),
            (rule.cooldown_sec as i32).into(),
            rule.created_by.clone().into(),
        ],
    ))
    .await?;
    Ok(())
}
