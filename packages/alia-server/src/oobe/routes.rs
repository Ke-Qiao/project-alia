//! OOBE HTTP routes — unauthenticated setup API.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::bootstrap::SetupResult;

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

/// Return the example config template for the frontend to display/edit.
async fn generate_config() -> Json<GenerateConfigResponse> {
    Json(GenerateConfigResponse {
        template: include_str!("../../../../config/server.example.toml").to_string(),
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

/// Validate and save config from the frontend wizard.
async fn save_config(
    State(state): State<OobeState>,
    Json(body): Json<SaveConfigRequest>,
) -> Json<SaveConfigResponse> {
    // Validate TOML syntax
    if let Err(e) = toml::from_str::<toml::Value>(&body.config) {
        return Json(SaveConfigResponse {
            success: false,
            path: String::new(),
            error: Some(format!("Invalid TOML: {e}")),
        });
    }

    // Validate it can parse as ServerConfig
    if let Err(e) = toml::from_str::<crate::config::ServerConfig>(&body.config) {
        return Json(SaveConfigResponse {
            success: false,
            path: String::new(),
            error: Some(format!("Config validation failed: {e}")),
        });
    }

    let target: PathBuf = body.path.map(PathBuf::from).unwrap_or_else(|| {
        let local = PathBuf::from("config/server.toml");
        if local.parent().map_or(false, |p| p.exists()) {
            local
        } else {
            dirs::config_dir()
                .map(|d| d.join("alia").join("server.toml"))
                .unwrap_or(local)
        }
    });

    // Write config
    if let Some(parent) = target.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Json(SaveConfigResponse {
                success: false,
                path: target.to_string_lossy().into_owned(),
                error: Some(format!("Failed to create directory: {e}")),
            });
        }
    }

    match std::fs::write(&target, &body.config) {
        Ok(()) => {
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
            error: Some(format!("Failed to write file: {e}")),
        }),
    }
}
