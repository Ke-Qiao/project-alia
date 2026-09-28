//! OOBE HTTP routes — unauthenticated setup API.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::bootstrap::{self, SetupResult};

#[cfg(test)]
mod tests;

// ── State for the OOBE server ──────────────────────────────────────────

#[derive(Clone)]
pub struct OobeState {
    /// Channel to signal setup completion
    pub done_tx: Arc<watch::Sender<Option<SetupResult>>>,
}

// ── Routes ─────────────────────────────────────────────────────────────

pub fn routes() -> Router<OobeState> {
    Router::new()
        .route("/api/v1/setup/status", get(get_status))
        .route("/api/v1/setup/generate-config", post(generate_config))
        .route("/api/v1/setup/save-config", post(save_config))
        .route("/health", get(health))
}

// ── Handlers ───────────────────────────────────────────────────────────

#[derive(Serialize)]
struct StatusResponse {
    mode: &'static str,
    needs_setup: bool,
    config_locations: Vec<String>,
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "setup",
        "mode": "oobe",
    }))
}

async fn get_status() -> Json<StatusResponse> {
    let config_locations = vec![
        "config/server.toml".to_string(),
        dirs::config_dir()
            .map(|d| {
                d.join("alia")
                    .join("server.toml")
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| "~/.config/alia/server.toml".to_string()),
        "/etc/alia/server.toml".to_string(),
    ];

    Json(StatusResponse {
        mode: "oobe",
        needs_setup: true,
        config_locations,
    })
}

#[derive(Serialize)]
struct GenerateConfigResponse {
    template: String,
}

/// Return the full embedded example, including all referenced profiles.
async fn generate_config() -> Result<Json<GenerateConfigResponse>, (axum::http::StatusCode, String)>
{
    bootstrap::example_config()
        .map(|template| Json(GenerateConfigResponse { template }))
        .map_err(|error| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Setup template unavailable: {error:#}"),
            )
        })
}

#[derive(Deserialize)]
struct SaveConfigRequest {
    /// TOML content to write
    config: String,
    /// Where to write; if omitted, uses the default location
    #[serde(default)]
    path: Option<String>,
}

#[derive(Serialize)]
struct SaveConfigResponse {
    success: bool,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Validate the effective configuration before atomically publishing its base.
async fn save_config(
    State(state): State<OobeState>,
    Json(body): Json<SaveConfigRequest>,
) -> Json<SaveConfigResponse> {
    let target = body
        .path
        .map(PathBuf::from)
        .unwrap_or_else(bootstrap::default_config_target);
    match bootstrap::save_config(&target, &body.config) {
        Ok(target) => {
            let path_str = target.to_string_lossy().into_owned();
            // Signal setup complete
            let _ = state.done_tx.send(Some(SetupResult::ConfigReady(target)));
            Json(SaveConfigResponse {
                success: true,
                path: path_str,
                error: None,
            })
        }
        Err(e) => Json(SaveConfigResponse {
            success: false,
            path: target.to_string_lossy().into_owned(),
            error: Some(format!("Config save failed: {e:#}")),
        }),
    }
}
