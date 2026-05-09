use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::providers::adapter::{ToolCall, ToolContext, ToolDefinition, ToolResult};
use crate::state::AppState;
use crate::tools::builtin::to_openai_functions;

// ========== Request/Response Types ==========

#[derive(Serialize, Deserialize)]
pub struct ExecuteToolRequest {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    #[serde(default)]
    pub call_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct OpenAIFunctionsResponse {
    pub tools: Vec<serde_json::Value>,
}

// ========== Handlers ==========

/// List all available tools
async fn list_tools(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<ToolDefinition>>> {
    let tools = state.modules.tools.list_tools().await?;
    Ok(Json(tools))
}

/// Get tools in OpenAI function calling format
async fn list_tools_openai_format(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<OpenAIFunctionsResponse>> {
    let tools = state.modules.tools.list_tools().await?;
    let openai_tools = to_openai_functions(&tools);
    Ok(Json(OpenAIFunctionsResponse {
        tools: openai_tools,
    }))
}

/// Execute a tool call
async fn execute_tool(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<ExecuteToolRequest>,
) -> Result<Json<ToolResult>> {
    let call = ToolCall {
        tool_name: body.tool_name,
        arguments: body.arguments,
        call_id: body.call_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
    };

    let ctx = ToolContext {
        user_id: auth.id,
        conversation_id: None,
        source: "manual".to_string(),
    };

    let result = state.modules.tools.execute(call, &ctx).await?;
    Ok(Json(result))
}

/// Get a specific tool definition
async fn get_tool(
    State(state): State<AppState>,
    _auth: AuthUser,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<Json<ToolDefinition>> {
    state
        .modules
        .tools
        .get_tool(&name)
        .await?
        .map(Json)
        .ok_or_else(|| crate::error::AppError::NotFound(format!("Tool '{}' not found", name)))
}

// ========== Routes ==========

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tools", get(list_tools))
        .route("/tools/openai", get(list_tools_openai_format))
        .route("/tools/{name}", get(get_tool))
        .route("/tools/execute", post(execute_tool))
}
