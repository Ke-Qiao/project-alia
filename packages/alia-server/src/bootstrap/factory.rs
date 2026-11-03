use sea_orm::DatabaseConnection;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::channels::adapter::ChannelAdapter;
use crate::channels::cli::CliChannel;
#[cfg(feature = "channel-embedded")]
use crate::channels::embedded::{
    EmbeddedChannel, EmbeddedChannelConfig, EmbeddedRuntimeDependencies,
};
use crate::config::{ChannelConfig, LlmProfileKind, LlmProviderProfileRef, ServerConfig};
use crate::event::EventBus;
use crate::modules::embodiment::sensors::SensorRegistry;
use crate::modules::interaction::conversation::ConversationStore;
use crate::providers::adapter::{EmbeddingAdapter, SttAdapter, TtsAdapter};
use crate::providers::embedding::NoopEmbedding;
use crate::providers::llm::{LlmService, NoopStt, NoopTts};

pub fn create_turn_detector(
    config: &crate::config::EasyTurnSection,
) -> anyhow::Result<Option<Arc<dyn crate::modules::interaction::turn_detector::TurnDetector>>> {
    if !config.enabled {
        return Ok(None);
    }
    #[cfg(feature = "turn-easy")]
    {
        Ok(Some(Arc::new(
            crate::adapters::easy_turn::EasyTurnDetector::new(config)?,
        )))
    }
    #[cfg(not(feature = "turn-easy"))]
    anyhow::bail!("Easy-Turn is configured but this binary was built without the turn-easy feature")
}

#[cfg(feature = "embed-openai")]
use crate::providers::embedding::OpenAiEmbedding;
#[cfg(feature = "stt-whisper")]
use crate::providers::stt::WhisperCppStt;
#[cfg(feature = "tts-sovits")]
use crate::providers::tts::GptSovitsTts;

pub fn create_llm(config: &ServerConfig, profile_id: &str) -> anyhow::Result<Box<LlmService>> {
    if profile_id == "noop" {
        return Ok(Box::new(LlmService::noop()));
    }
    let profile = config.profiles.llm_profile(profile_id)?;
    anyhow::ensure!(
        profile.kind == LlmProfileKind::Chat,
        "LLM profile '{profile_id}' is not a chat profile"
    );
    Ok(Box::new(LlmService::from_profile(
        profile_id,
        profile,
        config.llm_provider_profile(&profile.provider)?,
    )?))
}

pub fn create_router_decision(
    config: &ServerConfig,
    role: &str,
) -> anyhow::Result<Option<Arc<crate::modules::interaction::routing::JevRouter>>> {
    config.validate_routing_profile()?;
    if role == "legacy" {
        return Ok(None);
    }
    let profile = config.profiles.llm_profile(role)?;
    anyhow::ensure!(
        profile.kind == LlmProfileKind::Decision,
        "routing requires a decision profile"
    );
    let LlmProviderProfileRef::TypeSafe(provider) =
        config.llm_provider_profile(&profile.provider)?
    else {
        anyhow::bail!("decision profile requires a TypeSafe provider");
    };
    let api_key = provider.resolved_api_key();
    Ok(Some(Arc::new(
        crate::modules::interaction::routing::JevRouter::new(
            &api_key,
            &provider.base_url,
            &profile.model,
            profile.expected_decision_model()?,
        )?,
    )))
}

pub fn create_stt(_config: &ServerConfig, name: &str) -> anyhow::Result<Box<dyn SttAdapter>> {
    match name {
        #[cfg(feature = "stt-whisper")]
        "whisper" | "whisper_cpp" => {
            Ok(Box::new(WhisperCppStt::new(&_config.providers.whisper_cpp)))
        }
        #[cfg(not(feature = "stt-whisper"))]
        "whisper" | "whisper_cpp" => anyhow::bail!("STT '{name}' requires feature 'stt-whisper'"),
        "noop" => Ok(Box::new(NoopStt)),
        _ => anyhow::bail!("unknown STT adapter '{name}'"),
    }
}

pub fn create_tts(_config: &ServerConfig, name: &str) -> anyhow::Result<Box<dyn TtsAdapter>> {
    match name {
        #[cfg(feature = "tts-sovits")]
        "gpt_sovits" | "sovits" => Ok(Box::new(GptSovitsTts::new(
            &_config.providers.gpt_sovits,
            &_config.profiles.default_voice,
            &_config.profiles.voice,
            &_config.profiles.expression_styles,
        ))),
        #[cfg(not(feature = "tts-sovits"))]
        "gpt_sovits" | "sovits" => anyhow::bail!("TTS '{name}' requires feature 'tts-sovits'"),
        "noop" => Ok(Box::new(NoopTts)),
        _ => anyhow::bail!("unknown TTS adapter '{name}'"),
    }
}

pub fn create_embedding(
    config: &ServerConfig,
    profile_id: &str,
) -> anyhow::Result<Box<dyn EmbeddingAdapter>> {
    if profile_id == "noop" {
        tracing::info!(profile = profile_id, "creating embedding adapter (noop)");
        return Ok(Box::new(NoopEmbedding));
    }
    let profile = config.profiles.llm_profile(profile_id)?;
    if profile.kind != LlmProfileKind::Embedding {
        anyhow::bail!("LLM profile '{profile_id}' is not an embedding profile");
    }
    let provider = config.llm_provider_profile(&profile.provider)?;
    match provider {
        LlmProviderProfileRef::OpenAi(_provider) => {
            #[cfg(feature = "embed-openai")]
            {
                tracing::info!(
                    profile = profile_id,
                    provider = %profile.provider,
                    "creating embedding adapter (OpenAI)"
                );
                Ok(Box::new(OpenAiEmbedding::new(
                    profile_id, profile, _provider,
                )?))
            }
            #[cfg(not(feature = "embed-openai"))]
            {
                anyhow::bail!("profile '{profile_id}' requires feature 'embed-openai'")
            }
        }
        LlmProviderProfileRef::Noop => {
            tracing::info!(profile = profile_id, "creating embedding adapter (noop)");
            Ok(Box::new(NoopEmbedding))
        }
        LlmProviderProfileRef::Anthropic(_)
        | LlmProviderProfileRef::Gemini(_)
        | LlmProviderProfileRef::TypeSafe(_) => {
            anyhow::bail!("profile '{profile_id}' selects an unsupported embedding provider")
        }
    }
}

pub fn create_channel(
    config: &ChannelConfig,
    event_bus: EventBus,
    db: DatabaseConnection,
    cancel: CancellationToken,
    conversation: Arc<dyn ConversationStore>,
    _sensors: Arc<SensorRegistry>,
    _server_config: &ServerConfig,
) -> anyhow::Result<Box<dyn ChannelAdapter>> {
    match config {
        #[cfg(feature = "channel-embedded")]
        ChannelConfig::Embedded {
            id,
            owner_user_id,
            expected_device_id,
            expected_device_public_key,
            approved_animations,
            endpoint,
            discovery_service_name,
            discovery_timeout_sec,
            reconnect_interval_sec,
            reconnect_max_backoff_sec,
            grpc_timeout_sec,
            ..
        } => {
            tracing::info!(channel_id = %id, "creating EmbeddedChannel");
            Ok(Box::new(
                EmbeddedChannel::new(
                    EmbeddedChannelConfig {
                        id: id.clone(),
                        owner_user_id: *owner_user_id,
                        expected_device_id: expected_device_id.clone(),
                        expected_device_public_key: expected_device_public_key.clone(),
                        approved_animations: approved_animations.clone(),
                        endpoint: endpoint.clone(),
                        discovery_service_name: discovery_service_name.clone(),
                        discovery_timeout_sec: *discovery_timeout_sec,
                        reconnect_interval_sec: *reconnect_interval_sec,
                        reconnect_max_backoff_sec: *reconnect_max_backoff_sec,
                        grpc_timeout_sec: *grpc_timeout_sec,
                    },
                    event_bus,
                )
                .with_runtime(EmbeddedRuntimeDependencies {
                    db,
                    sensors: _sensors,
                    server: _server_config.server.clone(),
                    server_keypair_path: _server_config.auth.server_keypair_path.clone(),
                }),
            ))
        }
        #[cfg(not(feature = "channel-embedded"))]
        ChannelConfig::Embedded { id, .. } => {
            anyhow::bail!(
                "embedded channel '{}' requires the 'channel-embedded' feature",
                id
            )
        }
        ChannelConfig::Cli {
            id, owner_user_id, ..
        } => {
            tracing::info!(channel_id = %id, "creating CliChannel");
            Ok(Box::new(
                CliChannel::new(id.clone(), event_bus)
                    .with_owner_user_id(*owner_user_id)
                    .with_runtime(db, cancel, conversation),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapters_require_explicit_valid_selection() {
        let config: ServerConfig =
            toml::from_str(include_str!("../../../../config/server.example.toml")).unwrap();
        assert!(create_stt(&config, "noop").is_ok());
        assert!(create_tts(&config, "noop").is_ok());
        assert!(crate::modules::platform::insights::InsightService::new(false).is_ok());
        assert!(create_stt(&config, "whispr").is_err());
        assert!(create_tts(&config, "sovit").is_err());
        #[cfg(not(feature = "stt-whisper"))]
        assert!(create_stt(&config, "whisper").is_err());
        #[cfg(not(feature = "tts-sovits"))]
        assert!(create_tts(&config, "sovits").is_err());
        #[cfg(not(feature = "insights"))]
        assert!(crate::modules::platform::insights::InsightService::new(true).is_err());
    }

    #[cfg(not(feature = "channel-embedded"))]
    #[test]
    fn embedded_channel_without_feature_returns_error() {
        let config = ChannelConfig::Embedded {
            id: "pi-living-room".to_string(),
            owner_user_id: None,
            expected_device_id: None,
            expected_device_public_key: None,
            approved_animations: vec![],
            enabled: true,
            endpoint: String::new(),
            discovery_service_name: "_alia-embedded._tcp".to_string(),
            discovery_timeout_sec: 3,
            reconnect_interval_sec: 5,
            reconnect_max_backoff_sec: 60,
            grpc_timeout_sec: 10,
        };

        let server_config: ServerConfig =
            toml::from_str(include_str!("../../../../config/server.example.toml")).unwrap();
        match create_channel(
            &config,
            EventBus::new(1),
            DatabaseConnection::Disconnected,
            CancellationToken::new(),
            Arc::new(crate::modules::interaction::conversation::ConversationService::new()),
            Arc::new(SensorRegistry::new(Default::default())),
            &server_config,
        ) {
            Ok(_) => panic!("embedded channel creation should fail without channel-embedded"),
            Err(error) => assert!(error.to_string().contains("channel-embedded")),
        }
    }
}
