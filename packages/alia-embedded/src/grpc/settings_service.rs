use std::sync::Arc;

use tonic::{Request, Response, Status};

use alia_proto::settings::embedded_settings_server::EmbeddedSettings as EmbeddedSettingsTrait;
use alia_proto::settings::{
    DeleteDirectResponseRequest, DeleteDirectResponseResponse, DirectResponseList,
    DirectResponseRule, SettingsSnapshot as ProtoSettingsSnapshot, UpdateSettingsRequest,
};

use super::auth::require_auth_if_bound;
use crate::binding::BindingManager;
use crate::module::traits::DiscoveryBackend;
use crate::plugin::PluginRegistry;
use crate::settings::{EmbeddedSettingsManager, SettingsSnapshot};
use crate::storage::{DirectResponseRuleRecord, EmbeddedStore};

pub struct EmbeddedSettingsService {
    binding: Arc<BindingManager>,
    settings: Arc<EmbeddedSettingsManager>,
    state_store: EmbeddedStore,
    discovery: Arc<dyn DiscoveryBackend>,
    plugins: Arc<PluginRegistry>,
}

impl EmbeddedSettingsService {
    pub fn new(
        binding: Arc<BindingManager>,
        settings: Arc<EmbeddedSettingsManager>,
        state_store: EmbeddedStore,
        discovery: Arc<dyn DiscoveryBackend>,
        plugins: Arc<PluginRegistry>,
    ) -> Self {
        Self {
            binding,
            settings,
            state_store,
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

    async fn list_direct_responses(
        &self,
        request: Request<()>,
    ) -> Result<Response<DirectResponseList>, Status> {
        require_auth_if_bound(&self.binding, &request).await?;
        let rules = self
            .state_store
            .list_direct_responses(true)
            .await
            .map_err(|error| Status::internal(error.to_string()))?
            .into_iter()
            .map(record_to_proto)
            .collect();

        Ok(Response::new(DirectResponseList {
            rules,
            requires_restart: true,
        }))
    }

    async fn upsert_direct_response(
        &self,
        request: Request<DirectResponseRule>,
    ) -> Result<Response<DirectResponseRule>, Status> {
        require_auth_if_bound(&self.binding, &request).await?;
        let record = proto_to_record(request.into_inner())?;
        let saved = self
            .state_store
            .upsert_direct_response(&record)
            .await
            .map_err(|error| Status::internal(error.to_string()))?;
        Ok(Response::new(record_to_proto(saved)))
    }

    async fn delete_direct_response(
        &self,
        request: Request<DeleteDirectResponseRequest>,
    ) -> Result<Response<DeleteDirectResponseResponse>, Status> {
        require_auth_if_bound(&self.binding, &request).await?;
        let id = request.into_inner().id;
        if id <= 0 {
            return Err(Status::invalid_argument(
                "direct response id must be positive",
            ));
        }

        let success = self
            .state_store
            .delete_direct_response(id)
            .await
            .map_err(|error| Status::internal(error.to_string()))?;
        Ok(Response::new(DeleteDirectResponseResponse {
            success,
            requires_restart: true,
        }))
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

fn proto_to_record(rule: DirectResponseRule) -> Result<DirectResponseRuleRecord, Status> {
    let keywords: Vec<String> = rule
        .keywords
        .into_iter()
        .filter(|keyword| !keyword.trim().is_empty())
        .collect();

    if keywords.is_empty() {
        return Err(Status::invalid_argument(
            "direct response requires at least one keyword",
        ));
    }
    if rule.audio_path.trim().is_empty() {
        return Err(Status::invalid_argument(
            "direct response audio_path is required",
        ));
    }

    Ok(DirectResponseRuleRecord {
        id: rule.id,
        keywords,
        audio_path: rule.audio_path,
        action: (!rule.action.trim().is_empty()).then_some(rule.action),
        enabled: rule.enabled,
    })
}

fn record_to_proto(record: DirectResponseRuleRecord) -> DirectResponseRule {
    DirectResponseRule {
        id: record.id,
        keywords: record.keywords,
        audio_path: record.audio_path,
        action: record.action.unwrap_or_default(),
        enabled: record.enabled,
    }
}
