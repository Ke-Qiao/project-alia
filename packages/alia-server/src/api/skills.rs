use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::entity::skill;
use crate::error::Result;
use crate::providers::skills::tool_format::{self, ToolFormat};
use crate::state::AppState;

// ========== Response types ==========

#[derive(Serialize)]
pub struct SkillResponse {
    pub id: Uuid,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub category: String,
    pub intent_patterns: serde_json::Value,
    pub parameters: serde_json::Value,
    pub execution_config: Option<serde_json::Value>,
    pub prompt_template: Option<String>,
    pub cacheable: bool,
    pub enabled: bool,
    pub priority: i32,
    pub created_at: String,
    pub updated_at: String,
}

impl From<skill::Model> for SkillResponse {
    fn from(m: skill::Model) -> Self {
        Self {
            id: m.id,
            name: m.name,
            display_name: m.display_name,
            description: m.description,
            category: m.category,
            intent_patterns: m.intent_patterns,
            parameters: m.parameters,
            execution_config: m.execution_config,
            prompt_template: m.prompt_template,
            cacheable: m.cacheable,
            enabled: m.enabled,
            priority: m.priority,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
        }
    }
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
    pub execution_config: Option<serde_json::Value>,
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
    pub execution_config: Option<Option<serde_json::Value>>,
    pub prompt_template: Option<Option<String>>,
    pub cacheable: Option<bool>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
}

#[derive(Deserialize)]
pub struct MatchRequest {
    pub intent: String,
}

#[derive(Deserialize)]
pub struct ExecuteRequest {
    pub skill_name: String,
    pub parameters: serde_json::Value,
}

#[derive(Deserialize)]
pub struct ExportToolsRequest {
    /// LLM format: "claude" or "openai"
    pub format: ToolFormat,
    /// Optional category filter
    pub category: Option<String>,
}

// ========== Handlers ==========

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

    let now = Utc::now();
    let model = skill::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set(body.name),
        display_name: Set(body.display_name),
        description: Set(body.description),
        category: Set(body.category),
        intent_patterns: Set(body.intent_patterns),
        parameters: Set(body.parameters),
        execution_config: Set(body.execution_config),
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
    if let Some(execution_config) = body.execution_config {
        active.execution_config = Set(execution_config);
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

async fn execute_skill(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<ExecuteRequest>,
) -> Result<Json<serde_json::Value>> {
    use crate::providers::adapter::SkillMatch;

    let skill_match = SkillMatch {
        name: body.skill_name.clone(),
        confidence: 1.0,
        parameters: serde_json::Value::Object(serde_json::Map::new()),
    };

    let result = state
        .modules
        .skills
        .execute(&skill_match, body.parameters)
        .await
        .map_err(crate::error::AppError::Internal)?;

    Ok(Json(serde_json::json!({
        "success": result.success,
        "output": result.output,
        "message": result.message,
    })))
}

/// Export all enabled skills as LLM tool definitions in the requested format.
/// Returns an array ready to pass directly to Claude's `tools` parameter or
/// OpenAI's `tools` parameter.
async fn export_tools(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<ExportToolsRequest>,
) -> Result<Json<serde_json::Value>> {
    let mut query = skill::Entity::find().filter(skill::Column::Enabled.eq(true));

    if let Some(category) = &body.category {
        query = query.filter(skill::Column::Category.eq(category));
    }

    let skills = query
        .order_by_desc(skill::Column::Priority)
        .all(&state.db)
        .await
        .map_err(crate::error::AppError::Database)?;

    let tools: Vec<serde_json::Value> = tool_format::to_tool_definitions(&skills, body.format)
        .into_iter()
        .map(|t| t.definition)
        .collect();

    Ok(Json(serde_json::json!({
        "format": body.format,
        "tools": tools,
        "count": tools.len(),
    })))
}

/// Export a single skill as an LLM tool definition.
async fn export_skill_tool(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(name): Path<String>,
    axum::extract::Query(params): axum::extract::Query<ExportToolQuery>,
) -> Result<Json<serde_json::Value>> {
    let s = skill::Entity::find()
        .filter(skill::Column::Name.eq(&name))
        .one(&state.db)
        .await
        .map_err(crate::error::AppError::Database)?
        .ok_or_else(|| crate::error::AppError::NotFound(format!("Skill '{}' not found", name)))?;

    let format = params.format.unwrap_or(ToolFormat::Claude);
    let result = tool_format::to_tool_definition(&s, format);

    Ok(Json(result.definition))
}

#[derive(Deserialize)]
pub struct ExportToolQuery {
    /// LLM format: "claude" or "openai" (defaults to claude)
    pub format: Option<ToolFormat>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        // Fixed-path routes must come before parameterized routes
        .route("/skills/match", post(match_skill))
        .route("/skills/execute", post(execute_skill))
        .route("/skills/export", post(export_tools))
        .route("/skills", get(list_skills).post(create_skill))
        .route(
            "/skills/{name}",
            get(get_skill).put(update_skill).delete(delete_skill),
        )
        .route("/skills/{name}/tool", get(export_skill_tool))
}
