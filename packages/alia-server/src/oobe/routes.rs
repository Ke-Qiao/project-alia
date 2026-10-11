//! OOBE HTTP routes — unauthenticated setup API.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::bootstrap::{self, SetupCompletion};

#[cfg(test)]
mod tests;

// ── State for the OOBE server ──────────────────────────────────────────

#[derive(Clone)]
pub struct OobeState {
    pub completion: Arc<SetupCompletion>,
    pub cancel: tokio_util::sync::CancellationToken,
}

// ── Routes ─────────────────────────────────────────────────────────────

pub fn routes() -> Router<OobeState> {
    Router::new()
        .route("/api/v1/setup/status", get(get_status))
        .route("/api/v1/setup/generate-config", post(generate_config))
        .route("/api/v1/setup/save-config", post(save_config))
        .route("/health", get(health))
        .layer(middleware::from_fn(require_local_origin))
}

/// Setup trusts local operators (including SSH tunnels), not arbitrary browser
/// origins. JSON extraction separately rejects simple form/text POSTs. This is
/// not authentication against another process running on the same host.
async fn require_local_origin(request: Request, next: Next) -> Result<Response, StatusCode> {
    let mut hosts = request.headers().get_all(header::HOST).iter();
    let header_host = hosts
        .next()
        .map(|value| value.to_str())
        .transpose()
        .map_err(|_| StatusCode::FORBIDDEN)?;
    if hosts.next().is_some() {
        return Err(StatusCode::FORBIDDEN);
    }
    let uri_host = request.uri().authority().map(|value| value.as_str());
    let host = header_host.or(uri_host).ok_or(StatusCode::FORBIDDEN)?;
    if uri_host.is_some_and(|authority| authority != host) {
        return Err(StatusCode::FORBIDDEN);
    }
    let authority: axum::http::uri::Authority = host.parse().map_err(|_| StatusCode::FORBIDDEN)?;
    if !["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|local| authority.host().eq_ignore_ascii_case(local))
        || (authority.port().is_some() && authority.port_u16().is_none())
        || authority.as_str().contains('@')
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let mut origins = request.headers().get_all(header::ORIGIN).iter();
    if let Some(origin) = origins.next() {
        if origins.next().is_some()
            || origin.to_str().ok() != Some(format!("http://{host}").as_str())
        {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(next.run(request).await)
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
    match state.completion.save(&target, &body.config, &state.cancel) {
        Ok(target) => {
            let path_str = target.to_string_lossy().into_owned();
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
