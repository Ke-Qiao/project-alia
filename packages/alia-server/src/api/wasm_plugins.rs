use crate::{
    auth::AuthUser,
    error::{AppError, Result},
    state::AppState,
};
use alia_plugin_runtime::{MAX_MODULE_BYTES, package::PackageManifest};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    routing::{get, post},
};
use base64::Engine;
use sea_orm::TransactionTrait;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Upload {
    manifest: PackageManifest,
    module_base64: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    expected_pin: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Upgrade {
    manifest: PackageManifest,
    module_base64: String,
    expected_pin: String,
}

enum Mutation {
    Install(Upload),
    Enable(String, Pin),
    Revoke(String, Pin),
    Upgrade(String, Upgrade),
    Uninstall(String, Pin),
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> Result<Json<Value>> {
    super::plugins::require_admin(&auth)?;
    super::plugins::require_current_actor(&state.db, &auth, true, false).await?;
    let plugins = state.wasm_plugins.as_ref().ok_or(AppError::Unavailable(
        "configure plugins.wasm_dir to manage WASM packages",
    ))?;
    Ok(Json(
        serde_json::to_value(plugins.list().await).map_err(anyhow::Error::from)?,
    ))
}
async fn install(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(upload): Json<Upload>,
) -> Result<Json<Value>> {
    mutate(state, auth, Mutation::Install(upload)).await
}
async fn enable(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(name): Path<String>,
    Json(pin): Json<Pin>,
) -> Result<Json<Value>> {
    mutate(state, auth, Mutation::Enable(name, pin)).await
}
async fn revoke(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(name): Path<String>,
    Json(pin): Json<Pin>,
) -> Result<Json<Value>> {
    mutate(state, auth, Mutation::Revoke(name, pin)).await
}
async fn upgrade(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(name): Path<String>,
    Json(upload): Json<Upgrade>,
) -> Result<Json<Value>> {
    mutate(state, auth, Mutation::Upgrade(name, upload)).await
}
async fn uninstall(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(name): Path<String>,
    Json(pin): Json<Pin>,
) -> Result<Json<Value>> {
    mutate(state, auth, Mutation::Uninstall(name, pin)).await
}

async fn mutate(state: AppState, auth: AuthUser, mutation: Mutation) -> Result<Json<Value>> {
    super::plugins::require_admin(&auth)?;
    let plugins = state.wasm_plugins.clone().ok_or(AppError::Unavailable(
        "configure plugins.wasm_dir to manage WASM packages",
    ))?;
    let (reply, response) = tokio::sync::oneshot::channel();
    let db = state.db.clone();
    // Disconnecting the HTTP waiter cannot abandon an installation or a revocation.
    state
        .runtime_tasks
        .try_spawn("wasm_package_management", async move {
            let result: Result<Value> = async {
                let tx = db.begin().await?;
                super::plugins::require_current_actor(&tx, &auth, true, true).await?;
                let result: anyhow::Result<Value> = async {
                    match mutation {
                        Mutation::Install(upload) => Ok(serde_json::to_value(
                            plugins
                                .install(upload.manifest, decode(&upload.module_base64)?)
                                .await?,
                        )?),
                        Mutation::Enable(name, pin) => {
                            plugins.enable(&name, &pin.expected_pin).await?;
                            Ok(json!({"name":name,"enabled":true}))
                        }
                        Mutation::Revoke(name, pin) => {
                            plugins.revoke(&name, &pin.expected_pin).await?;
                            Ok(json!({"name":name,"enabled":false}))
                        }
                        Mutation::Upgrade(name, upload) => {
                            anyhow::ensure!(
                                name == upload.manifest.name,
                                "path and manifest package names differ"
                            );
                            Ok(serde_json::to_value(
                                plugins
                                    .upgrade(
                                        upload.manifest,
                                        decode(&upload.module_base64)?,
                                        &upload.expected_pin,
                                    )
                                    .await?,
                            )?)
                        }
                        Mutation::Uninstall(name, pin) => {
                            plugins.uninstall(&name, &pin.expected_pin).await?;
                            Ok(json!({"name":name,"uninstalled":true}))
                        }
                    }
                }
                .await;
                let value = result.map_err(|error| AppError::Validation(error.to_string()))?;
                tx.commit().await?;
                Ok(value)
            }
            .await;
            let _ = reply.send(result);
            Ok(())
        })
        .map_err(|_| AppError::Unavailable("server runtime is stopping"))?;
    response
        .await
        .map_err(|_| {
            AppError::Unavailable(
                "WASM operation owner failed; inspect current package state before retrying",
            )
        })?
        .map(Json)
}
fn decode(encoded: &str) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        encoded.len() <= MAX_MODULE_BYTES.div_ceil(3) * 4,
        "WASM asset exceeds upload budget"
    );
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    anyhow::ensure!(
        bytes.len() <= MAX_MODULE_BYTES,
        "WASM asset exceeds upload budget"
    );
    Ok(bytes)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/plugins/wasm", get(list).post(install))
        .route(
            "/plugins/wasm/{name}",
            axum::routing::put(upgrade).delete(uninstall),
        )
        .route("/plugins/wasm/{name}/enable", post(enable))
        .route("/plugins/wasm/{name}/revoke", post(revoke))
        .layer(DefaultBodyLimit::max(24 * 1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upload_does_not_accept_malformed_or_oversized_base64() {
        assert!(decode("not base64").is_err());
        assert!(decode(&"A".repeat(MAX_MODULE_BYTES.div_ceil(3) * 4 + 1)).is_err());
        assert_eq!(decode("AGFzbQ==").unwrap(), b"\0asm");
    }
}
