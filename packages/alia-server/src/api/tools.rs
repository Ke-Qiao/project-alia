//! Tools API: Tool registry and execution
//!
//! Endpoints:
//! - GET /tools - List available tools
//! - POST /tools/{name}/execute - Execute tool (for manual testing/debugging)

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;
use crate::providers::adapter::ToolDefinition;

#[derive(Serialize, Deserialize)]
pub struct ExecuteToolRequest {
    pub arguments: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub source: String,
    pub require_confirmation: bool,
    pub required_role: Option<String>,
}

impl From<ToolDefinition> for ToolInfo {
    fn from(def: ToolDefinition) -> Self {
        Self {
            name: def.name,
            description: def.description,
            parameters: def.parameters,
            source: def.source,
            require_confirmation: def.require_confirmation,
            required_role: def.required_role,
        }
    }
}

// TODO: List available tools
async fn list_tools(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<ToolInfo>>> {
    Ok(Json(Vec::new()))
}

// TODO: Execute tool (manual execution for testing)
async fn execute_tool(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_name): Path<String>,
    Json(_body): Json<ExecuteToolRequest>,
) -> Result<Json<serde_json::Value>> {
    Err(crate::error::AppError::Internal(anyhow::anyhow!("Not implemented")))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tools", get(list_tools))
        .route("/tools/{name}/execute", post(execute_tool))
}
