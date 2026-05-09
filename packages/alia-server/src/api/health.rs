use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use std::collections::HashMap;
use utoipa::ToSchema;

use crate::module::lifecycle::ModuleHealth;
use crate::state::AppState;

#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub modules: HashMap<String, ModuleStatus>,
}

#[derive(Serialize, ToSchema)]
pub struct ModuleStatus {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl From<ModuleHealth> for ModuleStatus {
    fn from(h: ModuleHealth) -> Self {
        match h {
            ModuleHealth::Healthy => ModuleStatus {
                status: "healthy".into(),
                message: None,
            },
            ModuleHealth::Degraded(msg) => ModuleStatus {
                status: "degraded".into(),
                message: Some(msg),
            },
            ModuleHealth::Unhealthy(msg) => ModuleStatus {
                status: "unhealthy".into(),
                message: Some(msg),
            },
        }
    }
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Aggregated health status of all modules", body = HealthResponse)
    )
)]
async fn health_check(State(state): State<AppState>) -> Json<HealthResponse> {
    let module_health = state.modules.health().await;

    let modules: HashMap<String, ModuleStatus> = module_health
        .into_iter()
        .map(|(name, health)| (name.to_string(), ModuleStatus::from(health)))
        .collect();

    let overall = if modules.values().all(|m| m.status == "healthy") {
        "ok"
    } else if modules.values().any(|m| m.status == "unhealthy") {
        "unhealthy"
    } else {
        "degraded"
    };

    Json(HealthResponse {
        status: overall.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        modules,
    })
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/health", get(health_check))
}
