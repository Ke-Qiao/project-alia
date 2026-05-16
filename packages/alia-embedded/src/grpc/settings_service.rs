use std::path::{Path, PathBuf};
use std::sync::Arc;

use tonic::{Request, Response, Status};
use uuid::Uuid;

use alia_proto::settings::embedded_settings_server::EmbeddedSettings as EmbeddedSettingsTrait;
use alia_proto::settings::{
    DeleteDirectResponseRequest, DeleteDirectResponseResponse, DirectResponseAudioVariant,
    DirectResponseList, DirectResponseRule, SetVolumeRequest,
    SettingsSnapshot as ProtoSettingsSnapshot, UpdateSettingsRequest, VolumeSettings,
};

use super::auth::require_auth_if_bound;
use crate::binding::BindingManager;
use crate::module::traits::DiscoveryBackend;
use crate::module::volume::{
    AUDIO_VOLUME_STATE_KEY, AudioVolumeConfig, AudioVolumeState, MAX_AUDIO_VOLUME, MIN_AUDIO_VOLUME,
};
use crate::plugin::PluginRegistry;
use crate::settings::{EmbeddedSettingsManager, SettingsSnapshot};
use crate::storage::{DirectResponseAudioRecord, DirectResponseRuleRecord, EmbeddedStore};

pub struct EmbeddedSettingsService {
    binding: Arc<BindingManager>,
    settings: Arc<EmbeddedSettingsManager>,
    state_store: EmbeddedStore,
    volume: AudioVolumeState,
    discovery: Arc<dyn DiscoveryBackend>,
    plugins: Arc<PluginRegistry>,
}

impl EmbeddedSettingsService {
    pub fn new(
        binding: Arc<BindingManager>,
        settings: Arc<EmbeddedSettingsManager>,
        state_store: EmbeddedStore,
        volume: AudioVolumeState,
        discovery: Arc<dyn DiscoveryBackend>,
        plugins: Arc<PluginRegistry>,
    ) -> Self {
        Self {
            binding,
            settings,
            state_store,
            volume,
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
        if let Some(volume) = volume_patch_from_settings_json(&request.get_ref().settings_json) {
            let mut next = self.volume.get();
            if let Some(capture_volume) = volume.capture_volume {
                next.capture_volume = capture_volume;
            }
            if let Some(playback_volume) = volume.playback_volume {
                next.playback_volume = playback_volume;
            }
            let next = self.volume.set(next);
            self.state_store
                .save_json(AUDIO_VOLUME_STATE_KEY, &next)
                .await
                .map_err(|error| Status::internal(error.to_string()))?;
        }
        self.refresh_discovery_metadata().await;
        build_response(snapshot).map(Response::new)
    }

    async fn get_volume(&self, request: Request<()>) -> Result<Response<VolumeSettings>, Status> {
        require_auth_if_bound(&self.binding, &request).await?;
        Ok(Response::new(volume_to_proto(self.volume.get())))
    }

    async fn set_volume(
        &self,
        request: Request<SetVolumeRequest>,
    ) -> Result<Response<VolumeSettings>, Status> {
        require_auth_if_bound(&self.binding, &request).await?;
        let request = request.into_inner();
        let mut next = self.volume.get();

        if request.set_capture_volume {
            next.capture_volume = request.capture_volume;
        }
        if request.set_playback_volume {
            next.playback_volume = request.playback_volume;
        }

        let next = self.volume.set(next);
        self.state_store
            .save_json(AUDIO_VOLUME_STATE_KEY, &next)
            .await
            .map_err(|error| Status::internal(error.to_string()))?;

        Ok(Response::new(volume_to_proto(next)))
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
        let record = self.proto_to_record(request.into_inner()).await?;
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

fn volume_to_proto(volume: AudioVolumeConfig) -> VolumeSettings {
    VolumeSettings {
        capture_volume: volume.capture_volume,
        playback_volume: volume.playback_volume,
        min_volume: MIN_AUDIO_VOLUME,
        max_volume: MAX_AUDIO_VOLUME,
    }
}

struct VolumePatch {
    capture_volume: Option<f32>,
    playback_volume: Option<f32>,
}

fn volume_patch_from_settings_json(settings_json: &str) -> Option<VolumePatch> {
    let patch = serde_json::from_str::<serde_json::Value>(settings_json).ok()?;
    let audio = patch.get("audio")?;
    let capture_volume = audio
        .get("capture_volume")
        .and_then(serde_json::Value::as_f64)
        .map(|value| value as f32);
    let playback_volume = audio
        .get("playback_volume")
        .and_then(serde_json::Value::as_f64)
        .map(|value| value as f32);

    (capture_volume.is_some() || playback_volume.is_some()).then_some(VolumePatch {
        capture_volume,
        playback_volume,
    })
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

impl EmbeddedSettingsService {
    async fn proto_to_record(
        &self,
        rule: DirectResponseRule,
    ) -> Result<DirectResponseRuleRecord, Status> {
        let snapshot = self.settings.snapshot(false).await;
        let audio_dir = snapshot.settings.direct_response_defaults.audio_dir;
        proto_to_record(rule, &audio_dir).await
    }
}

async fn proto_to_record(
    rule: DirectResponseRule,
    audio_dir: &Path,
) -> Result<DirectResponseRuleRecord, Status> {
    let slug = direct_response_slug(&rule);
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
    let variant_audio_dir = if slug.is_empty() {
        audio_dir.to_path_buf()
    } else {
        audio_dir.join(&slug)
    };
    let mut variants = rule.audio_variants;
    if variants.is_empty() && !rule.audio_path.trim().is_empty() {
        variants.push(DirectResponseAudioVariant {
            audio_path: rule.audio_path,
            weight: 1.0,
            audio_data: Vec::new(),
        });
    }
    let audio_variants = write_audio_variants(variants, &variant_audio_dir).await?;

    if audio_variants.is_empty() {
        return Err(Status::invalid_argument(
            "direct response requires audio_path or at least one audio variant",
        ));
    }

    Ok(DirectResponseRuleRecord {
        id: rule.id,
        slug,
        keywords,
        audio_variants,
        action: (!rule.action.trim().is_empty()).then_some(rule.action),
        enabled: rule.enabled,
    })
}

fn record_to_proto(record: DirectResponseRuleRecord) -> DirectResponseRule {
    DirectResponseRule {
        id: record.id,
        slug: record.slug,
        keywords: record.keywords,
        audio_path: record
            .audio_variants
            .first()
            .map(|variant| variant.audio_path.clone())
            .unwrap_or_default(),
        action: record.action.unwrap_or_default(),
        enabled: record.enabled,
        audio_variants: record
            .audio_variants
            .into_iter()
            .map(|variant| DirectResponseAudioVariant {
                audio_path: variant.audio_path,
                weight: variant.weight,
                audio_data: Vec::new(),
            })
            .collect(),
    }
}

fn direct_response_slug(rule: &DirectResponseRule) -> String {
    let explicit = sanitize_slug(&rule.slug);
    if !explicit.is_empty() {
        return explicit;
    }

    let action = sanitize_slug(&rule.action);
    if !action.is_empty() {
        return action;
    }

    rule.keywords
        .iter()
        .map(|keyword| sanitize_slug(keyword))
        .find(|slug| !slug.is_empty())
        .unwrap_or_else(|| format!("direct-response-{}", Uuid::new_v4()))
}

fn sanitize_slug(input: &str) -> String {
    input
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

async fn write_audio_variants(
    variants: Vec<DirectResponseAudioVariant>,
    audio_dir: &Path,
) -> Result<Vec<DirectResponseAudioRecord>, Status> {
    let mut records = Vec::new();

    for variant in variants {
        let audio_path = if variant.audio_data.is_empty() {
            let requested = variant.audio_path.trim();
            if requested.is_empty() {
                String::new()
            } else {
                audio_file_path(audio_dir, requested).display().to_string()
            }
        } else {
            let path = audio_file_path(audio_dir, &variant.audio_path);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|error| Status::internal(error.to_string()))?;
            }
            tokio::fs::write(&path, &variant.audio_data)
                .await
                .map_err(|error| Status::internal(error.to_string()))?;
            path.display().to_string()
        };

        if !audio_path.trim().is_empty() {
            records.push(DirectResponseAudioRecord {
                audio_path,
                weight: if variant.weight <= 0.0 {
                    1.0
                } else {
                    variant.weight
                },
            });
        }
    }

    Ok(records)
}

fn audio_file_path(audio_dir: &Path, requested: &str) -> PathBuf {
    let requested = requested.trim();
    if requested.is_empty() {
        return audio_dir.join(format!("direct-response-{}.wav", Uuid::new_v4()));
    }

    let path = Path::new(requested);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        audio_dir.join(path)
    }
}
