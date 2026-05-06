use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

#[derive(Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub category: String,
}

async fn list_skills(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<SkillInfo>>> {
    let skills = state
        .modules
        .skills
        .list_skills()
        .await
        .map_err(|e| crate::error::AppError::Internal(e))?;

    let result = skills
        .into_iter()
        .map(|s| SkillInfo {
            name: s.name,
            display_name: s.display_name,
            description: s.description,
            category: s.category,
        })
        .collect();

    Ok(Json(result))
}

async fn get_skill(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_name): Path<String>,
) -> Result<Json<SkillInfo>> {
    Err(crate::error::AppError::NotFound("Skill not found".into()))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/skills", get(list_skills))
        .route("/skills/{name}", get(get_skill))
}
