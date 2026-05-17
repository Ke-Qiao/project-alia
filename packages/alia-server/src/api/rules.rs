use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::providers::adapter::{AutomationRule, RuleFilter, RulePatch};
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRuleRequest {
    pub name: String,
    pub event_pattern: serde_json::Value,
    #[serde(default)]
    pub conditions: serde_json::Value,
    pub action: serde_json::Value,
    #[serde(default)]
    pub cooldown_sec: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateRuleRequest {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub event_pattern: Option<serde_json::Value>,
    pub conditions: Option<serde_json::Value>,
    pub action: Option<serde_json::Value>,
    pub cooldown_sec: Option<u32>,
}

async fn list_rules(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<AutomationRule>>> {
    let rules = state
        .modules
        .rule_engine
        .list_rules(RuleFilter {
            user_id: Some(auth.id),
            enabled_only: false,
        })
        .await?;
    Ok(Json(rules))
}

async fn create_rule(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<CreateRuleRequest>,
) -> Result<Json<AutomationRule>> {
    let rule = AutomationRule {
        id: Uuid::new_v4(),
        user_id: auth.id,
        name: body.name,
        enabled: true,
        event_pattern: body.event_pattern,
        conditions: body.conditions,
        action: body.action,
        cooldown_sec: body.cooldown_sec,
        created_by: "user".to_string(),
    };
    state.modules.rule_engine.create_rule(rule.clone()).await?;
    Ok(Json(rule))
}

async fn get_rule(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<AutomationRule>> {
    let rules = state
        .modules
        .rule_engine
        .list_rules(RuleFilter {
            user_id: Some(auth.id),
            enabled_only: false,
        })
        .await?;
    rules
        .into_iter()
        .find(|rule| rule.id == id)
        .map(Json)
        .ok_or_else(|| crate::error::AppError::NotFound("Rule not found".into()))
}

async fn update_rule(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRuleRequest>,
) -> Result<Json<serde_json::Value>> {
    state
        .modules
        .rule_engine
        .update_rule(
            id,
            RulePatch {
                name: body.name,
                enabled: body.enabled,
                event_pattern: body.event_pattern,
                conditions: body.conditions,
                action: body.action,
                cooldown_sec: body.cooldown_sec,
            },
        )
        .await?;
    Ok(Json(serde_json::json!({ "success": true, "rule_id": id })))
}

async fn delete_rule(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    state.modules.rule_engine.delete_rule(id).await?;
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn pause_rule(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    state.modules.rule_engine.pause_rule(id).await?;
    Ok(Json(serde_json::json!({ "success": true, "paused": true })))
}

async fn resume_rule(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    state.modules.rule_engine.resume_rule(id).await?;
    Ok(Json(
        serde_json::json!({ "success": true, "paused": false }),
    ))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/rules", get(list_rules).post(create_rule))
        .route(
            "/rules/{id}",
            get(get_rule).put(update_rule).delete(delete_rule),
        )
        .route("/rules/{id}/pause", post(pause_rule))
        .route("/rules/{id}/resume", post(resume_rule))
}
