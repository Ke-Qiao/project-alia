use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::entity::{skill, skill_preferred_tool, tool};
use crate::error::Result;
use crate::registry::{ArtifactRef, ArtifactRegistry};
use crate::state::AppState;

// ========== Response types ==========

#[derive(Serialize, ToSchema)]
pub struct SkillResponse {
    pub id: Uuid,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub category: String,
    pub intent_patterns: serde_json::Value,
    /// Tool names preferred during this skill's execution.
    pub preferred_tools: Vec<String>,
    pub prompt_template: Option<String>,
    pub cacheable: bool,
    pub enabled: bool,
    pub priority: i32,
    /// Source: 'builtin', 'file', 'api', or plugin name
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

async fn skill_response(
    db: &sea_orm::DatabaseConnection,
    m: skill::Model,
) -> Result<SkillResponse> {
    let preferred_tools = preferred_tool_names_for_skill(db, m.id).await?;
    Ok(SkillResponse {
        id: m.id,
        name: m.name,
        display_name: m.display_name,
        description: m.description,
        category: m.category,
        intent_patterns: m.intent_patterns,
        preferred_tools,
        prompt_template: m.prompt_template,
        cacheable: m.cacheable,
        enabled: m.enabled,
        priority: m.priority,
        source: m.source,
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    })
}

// ========== Request types ==========

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSkillRequest {
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub category: String,
    pub intent_patterns: serde_json::Value,
    /// Tool names to prefer during this skill's execution.
    #[serde(default)]
    pub preferred_tools: Vec<String>,
    pub prompt_template: Option<String>,
    #[serde(default)]
    pub cacheable: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i32,
}

fn default_enabled() -> bool {
    true
}

async fn preferred_tool_names_for_skill(
    db: &impl ConnectionTrait,
    skill_id: Uuid,
) -> Result<Vec<String>> {
    let bindings = skill_preferred_tool::Entity::find()
        .filter(skill_preferred_tool::Column::SkillId.eq(skill_id))
        .all(db)
        .await
        .map_err(crate::error::AppError::Database)?;
    let mut names = Vec::with_capacity(bindings.len());

    for binding in bindings {
        if let Some(row) = tool::Entity::find_by_id(binding.tool_id)
            .one(db)
            .await
            .map_err(crate::error::AppError::Database)?
            && row.enabled
        {
            names.push(row.name);
        }
    }

    Ok(names)
}

async fn replace_skill_preferred_tools(
    artifacts: &ArtifactRegistry,
    db: &impl ConnectionTrait,
    skill_id: Uuid,
    skill_name: &str,
    preferred_tool_names: &[String],
) -> Result<()> {
    skill_preferred_tool::Entity::delete_many()
        .filter(skill_preferred_tool::Column::SkillId.eq(skill_id))
        .exec(db)
        .await
        .map_err(crate::error::AppError::Database)?;

    let now = Utc::now();
    let mut seen = HashSet::new();
    let mut dependencies = Vec::new();
    for name in preferred_tool_names {
        if !seen.insert(name.as_str()) {
            continue;
        }
        let tool = tool::Entity::find()
            .filter(tool::Column::Name.eq(name))
            .one(db)
            .await
            .map_err(crate::error::AppError::Database)?
            .ok_or_else(|| {
                crate::error::AppError::Validation(format!(
                    "unknown tool referenced by skill: {name}"
                ))
            })?;
        artifacts
            .upsert_artifact_in(db, &ArtifactRef::tool(tool.name.clone()), &tool.source)
            .await
            .map_err(crate::error::AppError::Internal)?;
        dependencies.push(ArtifactRef::tool(tool.name.clone()));

        skill_preferred_tool::ActiveModel {
            skill_id: Set(skill_id),
            tool_id: Set(tool.id),
            created_at: Set(now),
        }
        .insert(db)
        .await
        .map_err(crate::error::AppError::Database)?;
    }

    artifacts
        .replace_dependencies_in(
            db,
            &ArtifactRef::skill(skill_name.to_string()),
            &dependencies,
        )
        .await
        .map_err(crate::error::AppError::Internal)?;

    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSkillRequest {
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub intent_patterns: Option<serde_json::Value>,
    pub preferred_tools: Option<Vec<String>>,
    pub prompt_template: Option<Option<String>>,
    pub cacheable: Option<bool>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
}

#[derive(Deserialize)]
pub struct MatchRequest {
    pub intent: String,
}

// ========== Handlers ==========

#[utoipa::path(
    get,
    path = "/api/v1/skills",
    tag = "skills",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "List all skills", body = Vec<SkillResponse>)
    )
)]
async fn list_skills(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<SkillResponse>>> {
    let skills = skill::Entity::find()
        .order_by_desc(skill::Column::Priority)
        .all(&state.db)
        .await
        .map_err(crate::error::AppError::Database)?;

    let mut response = Vec::with_capacity(skills.len());
    for skill in skills {
        response.push(skill_response(&state.db, skill).await?);
    }

    Ok(Json(response))
}

#[utoipa::path(
    get,
    path = "/api/v1/skills/{name}",
    tag = "skills",
    security(("bearer" = [])),
    params(("name" = String, Path, description = "Skill name")),
    responses(
        (status = 200, description = "Skill details", body = SkillResponse),
        (status = 404, description = "Skill not found")
    )
)]
async fn get_skill(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<SkillResponse>> {
    let s = skill::Entity::find()
        .filter(skill::Column::Name.eq(&name))
        .one(&state.db)
        .await
        .map_err(crate::error::AppError::Database)?
        .ok_or_else(|| crate::error::AppError::NotFound(format!("Skill '{}' not found", name)))?;

    Ok(Json(skill_response(&state.db, s).await?))
}

async fn create_skill(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<CreateSkillRequest>,
) -> Result<Json<SkillResponse>> {
    // Validate intent_patterns is an array
    if !body.intent_patterns.is_array() {
        return Err(crate::error::AppError::Validation(
            "intent_patterns must be a JSON array of strings".into(),
        ));
    }

    let now = Utc::now();
    let skill_id = Uuid::new_v4();
    let skill_name = body.name;
    let tx = state
        .db
        .begin()
        .await
        .map_err(crate::error::AppError::Database)?;
    let model = skill::ActiveModel {
        id: Set(skill_id),
        name: Set(skill_name.clone()),
        display_name: Set(body.display_name),
        description: Set(body.description),
        category: Set(body.category),
        intent_patterns: Set(body.intent_patterns),
        source: Set("api".to_string()),
        prompt_template: Set(body.prompt_template),
        cacheable: Set(body.cacheable),
        enabled: Set(body.enabled),
        priority: Set(body.priority),
        created_at: Set(now),
        updated_at: Set(now),
    };

    let result = model
        .insert(&tx)
        .await
        .map_err(crate::error::AppError::Database)?;
    state
        .artifacts
        .upsert_artifact_in(&tx, &ArtifactRef::skill(skill_name.clone()), "api")
        .await
        .map_err(crate::error::AppError::Internal)?;
    replace_skill_preferred_tools(
        &state.artifacts,
        &tx,
        skill_id,
        &skill_name,
        &body.preferred_tools,
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::error::AppError::Database)?;

    // TODO: trigger skills reload in SkillRegistry module

    Ok(Json(skill_response(&state.db, result).await?))
}

async fn update_skill(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(name): Path<String>,
    Json(body): Json<UpdateSkillRequest>,
) -> Result<Json<SkillResponse>> {
    let existing = skill::Entity::find()
        .filter(skill::Column::Name.eq(&name))
        .one(&state.db)
        .await
        .map_err(crate::error::AppError::Database)?
        .ok_or_else(|| crate::error::AppError::NotFound(format!("Skill '{}' not found", name)))?;

    let skill_id = existing.id;
    let mut active: skill::ActiveModel = existing.into();
    let preferred_tools_update = body.preferred_tools;

    if let Some(display_name) = body.display_name {
        active.display_name = Set(display_name);
    }
    if let Some(description) = body.description {
        active.description = Set(Some(description));
    }
    if let Some(category) = body.category {
        active.category = Set(category);
    }
    if let Some(intent_patterns) = body.intent_patterns {
        if !intent_patterns.is_array() {
            return Err(crate::error::AppError::Validation(
                "intent_patterns must be a JSON array of strings".into(),
            ));
        }
        active.intent_patterns = Set(intent_patterns);
    }
    if let Some(prompt_template) = body.prompt_template {
        active.prompt_template = Set(prompt_template);
    }
    if let Some(cacheable) = body.cacheable {
        active.cacheable = Set(cacheable);
    }
    if let Some(enabled) = body.enabled {
        active.enabled = Set(enabled);
    }
    if let Some(priority) = body.priority {
        active.priority = Set(priority);
    }

    active.updated_at = Set(Utc::now());

    let tx = state
        .db
        .begin()
        .await
        .map_err(crate::error::AppError::Database)?;
    let result = active
        .update(&tx)
        .await
        .map_err(crate::error::AppError::Database)?;
    state
        .artifacts
        .upsert_artifact_in(&tx, &ArtifactRef::skill(name.clone()), &result.source)
        .await
        .map_err(crate::error::AppError::Internal)?;
    if let Some(preferred_tools) = preferred_tools_update {
        replace_skill_preferred_tools(&state.artifacts, &tx, skill_id, &name, &preferred_tools)
            .await?;
    }
    tx.commit()
        .await
        .map_err(crate::error::AppError::Database)?;

    // TODO: trigger skills reload in SkillRegistry module

    Ok(Json(skill_response(&state.db, result).await?))
}

async fn delete_skill(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let tx = state
        .db
        .begin()
        .await
        .map_err(crate::error::AppError::Database)?;
    let artifact = ArtifactRef::skill(name.clone());
    state
        .artifacts
        .ensure_no_external_dependents_in(&tx, std::slice::from_ref(&artifact))
        .await
        .map_err(crate::error::AppError::Internal)?;
    let result = skill::Entity::delete_many()
        .filter(skill::Column::Name.eq(&name))
        .exec(&tx)
        .await
        .map_err(crate::error::AppError::Database)?;

    if result.rows_affected == 0 {
        return Err(crate::error::AppError::NotFound(format!(
            "Skill '{}' not found",
            name
        )));
    }
    state
        .artifacts
        .delete_artifact_in(&tx, &artifact)
        .await
        .map_err(crate::error::AppError::Internal)?;
    tx.commit()
        .await
        .map_err(crate::error::AppError::Database)?;

    // TODO: trigger skills reload in SkillRegistry module

    Ok(Json(serde_json::json!({"success": true})))
}

async fn match_skill(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<MatchRequest>,
) -> Result<Json<serde_json::Value>> {
    let result = state
        .modules
        .skills
        .match_skill(&body.intent)
        .await
        .map_err(crate::error::AppError::Internal)?;

    match result {
        Some(m) => Ok(Json(serde_json::json!({
            "matched": true,
            "skill": m.name,
            "confidence": m.confidence,
        }))),
        None => Ok(Json(serde_json::json!({
            "matched": false,
        }))),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        // Fixed-path routes must come before parameterized routes
        .route("/skills/match", post(match_skill))
        .route("/skills", get(list_skills).post(create_skill))
        .route(
            "/skills/{name}",
            get(get_skill).put(update_skill).delete(delete_skill),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_skill_request_accepts_preferred_tools() {
        let request: CreateSkillRequest = serde_json::from_value(serde_json::json!({
            "name": "weather",
            "display_name": "Weather",
            "description": null,
            "category": "information",
            "intent_patterns": ["weather"],
            "preferred_tools": ["sensor_read"],
            "prompt_template": null
        }))
        .expect("preferred_tools request should parse");

        assert_eq!(request.preferred_tools, vec!["sensor_read"]);
    }

    #[test]
    fn create_skill_request_rejects_legacy_tools_field() {
        let result: std::result::Result<CreateSkillRequest, _> =
            serde_json::from_value(serde_json::json!({
                "name": "weather",
                "display_name": "Weather",
                "description": null,
                "category": "information",
                "intent_patterns": ["weather"],
                "tools": ["sensor_read"],
                "prompt_template": null
            }));

        let err = result.err().expect("legacy tools field should be rejected");
        assert!(err.to_string().contains("unknown field `tools`"));
    }

    #[test]
    fn update_skill_request_rejects_legacy_tools_field() {
        let result: std::result::Result<UpdateSkillRequest, _> =
            serde_json::from_value(serde_json::json!({
                "tools": ["sensor_read"]
            }));

        let err = result.err().expect("legacy tools field should be rejected");
        assert!(err.to_string().contains("unknown field `tools`"));
    }
}
