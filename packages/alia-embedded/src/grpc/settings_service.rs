use std::sync::Arc;

use tonic::{Request, Response, Status};

use alia_proto::settings::embedded_settings_server::EmbeddedSettings as EmbeddedSettingsTrait;
use alia_proto::settings::{SettingsSnapshot as ProtoSettingsSnapshot, UpdateSettingsRequest};

use super::auth::require_auth_if_bound;
use crate::binding::BindingManager;
use crate::module::traits::DiscoveryBackend;
use crate::plugin::PluginRegistry;
use crate::settings::{EmbeddedSettingsManager, SettingsSnapshot};

pub struct EmbeddedSettingsService {
    binding: Arc<BindingManager>,
    settings: Arc<EmbeddedSettingsManager>,
    discovery: Arc<dyn DiscoveryBackend>,
    plugins: Arc<PluginRegistry>,
}

impl EmbeddedSettingsService {
    pub fn new(
        binding: Arc<BindingManager>,
        settings: Arc<EmbeddedSettingsManager>,
        discovery: Arc<dyn DiscoveryBackend>,
        plugins: Arc<PluginRegistry>,
    ) -> Self {
        Self {
            binding,
            settings,
            discovery,
            plugins,
        }
    }

    async fn refresh_discovery_metadata(&self) {
        let mut metadata = self.binding.status().await.discovery_metadata();
        metadata.attributes.insert(
            "provisioning_mode".to_string(),
            self.settings.provisioning_mode().await.to_string(),
        );
        self.plugins.enrich_discovery_metadata(&mut metadata).await;
        if let Err(error) = self.discovery.update_metadata(&metadata).await {
            tracing::warn!(error = %error, "failed to refresh discovery metadata");
        }
    }
}

#[tonic::async_trait]
impl EmbeddedSettingsTrait for EmbeddedSettingsService {
    async fn get_settings(
        &self,
        request: Request<()>,
    ) -> Result<Response<ProtoSettingsSnapshot>, Status> {
        require_auth_if_bound(&self.binding, &request).await?;
        let snapshot = self.settings.snapshot(false).await;
        build_response(snapshot).map(Response::new)
    }

    async fn update_settings(
        &self,
        request: Request<UpdateSettingsRequest>,
    ) -> Result<Response<ProtoSettingsSnapshot>, Status> {
        require_auth_if_bound(&self.binding, &request).await?;
        let snapshot = self
            .settings
            .update_from_json(&request.get_ref().settings_json)
            .await
            .map_err(|error| Status::invalid_argument(error.to_string()))?;
        self.refresh_discovery_metadata().await;
        build_response(snapshot).map(Response::new)
    }
}

fn build_response(snapshot: SettingsSnapshot) -> Result<ProtoSettingsSnapshot, Status> {
    let settings_json = serde_json::to_string_pretty(&snapshot.settings)
        .map_err(|error| Status::internal(error.to_string()))?;
    Ok(ProtoSettingsSnapshot {
        settings_json,
        provisioning_mode: snapshot.provisioning_mode,
        requires_restart: snapshot.requires_restart,
        settings_path: snapshot.settings_path,
    })
}
