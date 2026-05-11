use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::entity::skill;
use crate::error::Result;
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
    pub parameters: serde_json::Value,
    /// Tool names available to the LLM during this skill's execution
    pub tools: Vec<String>,
    pub prompt_template: Option<String>,
    pub cacheable: bool,
    pub enabled: bool,
    pub priority: i32,
    /// Source: 'builtin', 'file', 'api', or plugin name
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<skill::Model> for SkillResponse {
    fn from(m: skill::Model) -> Self {
        // Prefer native `tools` column; fall back to parsing `execution_config`
        let tools = parse_tools_for_api(&m.tools)
            .or_else(|| parse_tools_for_api(&m.execution_config))
            .unwrap_or_default();

        Self {
            id: m.id,
            name: m.name,
            display_name: m.display_name,
            description: m.description,
            category: m.category,
            intent_patterns: m.intent_patterns,
            parameters: m.parameters,
            tools,
            prompt_template: m.prompt_template,
            cacheable: m.cacheable,
            enabled: m.enabled,
            priority: m.priority,
            source: m.source,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
        }
    }
}

/// Parse tool names from a JSONB column for API responses.
/// Supports both new format (JSON array) and legacy format (nested object).
fn parse_tools_for_api(config: &Option<serde_json::Value>) -> Option<Vec<String>> {
    let config = config.as_ref()?;

    if let Some(arr) = config.as_array() {
        if arr.is_empty() {
            return None;
        }
        return Some(
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
        );
    }

    if let Some(name) = config
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
    {
        return Some(vec![name.to_string()]);
    }

    None
}

// ========== Request types ==========

#[derive(Deserialize)]
pub struct CreateSkillRequest {
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub category: String,
    pub intent_patterns: serde_json::Value,
    #[serde(default = "default_params")]
    pub parameters: serde_json::Value,
    /// Tool names to make available to the LLM during this skill's execution
    #[serde(default)]
    pub tools: Vec<String>,
    pub prompt_template: Option<String>,
    #[serde(default)]
    pub cacheable: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i32,
}

fn default_params() -> serde_json::Value {
    serde_json::json!({})
}
fn default_enabled() -> bool {
    true
}

#[derive(Deserialize)]
pub struct UpdateSkillRequest {
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub intent_patterns: Option<serde_json::Value>,
    pub parameters: Option<serde_json::Value>,
    pub tools: Option<Vec<String>>,
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

    Ok(Json(skills.into_iter().map(SkillResponse::from).collect()))
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

    Ok(Json(SkillResponse::from(s)))
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

    // Convert tools list for native tools column
    let tools_col = if body.tools.is_empty() {
        None
    } else {
        Some(serde_json::Value::Array(
            body.tools.iter().map(|t| serde_json::json!(t)).collect(),
        ))
    };

    // Convert tools list to execution_config JSON array for DB storage
    let execution_config = if body.tools.is_empty() {
        None
    } else {
        Some(serde_json::Value::Array(
            body.tools.iter().map(|t| serde_json::json!(t)).collect(),
        ))
    };

    let now = Utc::now();
    let model = skill::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set(body.name),
        display_name: Set(body.display_name),
        description: Set(body.description),
        category: Set(body.category),
        intent_patterns: Set(body.intent_patterns),
        parameters: Set(body.parameters),
        execution_config: Set(execution_config),
        tools: Set(tools_col),
        source: Set("api".to_string()),
        prompt_template: Set(body.prompt_template),
        cacheable: Set(body.cacheable),
        enabled: Set(body.enabled),
        priority: Set(body.priority),
        created_at: Set(now),
        updated_at: Set(now),
    };

    let result = model
        .insert(&state.db)
        .await
        .map_err(crate::error::AppError::Database)?;

    // TODO: trigger skills reload in SkillProvider module

    Ok(Json(SkillResponse::from(result)))
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

    let mut active: skill::ActiveModel = existing.into();

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
    if let Some(parameters) = body.parameters {
        active.parameters = Set(parameters);
    }
    if let Some(tools) = body.tools {
        let execution_config = if tools.is_empty() {
            None
        } else {
            Some(serde_json::Value::Array(
                tools.iter().map(|t| serde_json::json!(t)).collect(),
            ))
        };
        let tools_col = if tools.is_empty() {
            None
        } else {
            Some(serde_json::Value::Array(
                tools.iter().map(|t| serde_json::json!(t)).collect(),
            ))
        };
        active.execution_config = Set(execution_config);
        active.tools = Set(tools_col);
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

    let result = active
        .update(&state.db)
        .await
        .map_err(crate::error::AppError::Database)?;

    // TODO: trigger skills reload in SkillProvider module

    Ok(Json(SkillResponse::from(result)))
}

async fn delete_skill(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let result = skill::Entity::delete_many()
        .filter(skill::Column::Name.eq(&name))
        .exec(&state.db)
        .await
        .map_err(crate::error::AppError::Database)?;

    if result.rows_affected == 0 {
        return Err(crate::error::AppError::NotFound(format!(
            "Skill '{}' not found",
            name
        )));
    }

    // TODO: trigger skills reload in SkillProvider module

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
            "parameters": m.parameters,
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
