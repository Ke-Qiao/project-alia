use std::sync::Arc;

use crate::channels::ChannelRegistry;
use crate::channels::adapter::ChannelAdapter;
use crate::channels::cli::CliChannel;
#[cfg(feature = "channel-embedded")]
use crate::channels::embedded::{EmbeddedChannel, EmbeddedChannelConfig};
#[cfg(feature = "channel-embedded")]
use crate::channels::embedded_volume_tool::EmbeddedVolumeTool;
use crate::config::{ChannelConfig, LlmProfileKind, LlmProviderKind, ServerConfig};
use crate::event::EventBus;
use crate::providers::adapter::{
    CacheBackend, EmbeddingAdapter, InsightStrategy, LlmAdapter, PersonaProvider, RouterStrategy,
    RuleEvaluator, SchedulerBackend, SkillProvider, SttAdapter, ToolRegistry, TtsAdapter,
};
use crate::providers::cache::DbCache;
use crate::providers::embedding::NoopEmbedding;
use crate::providers::insights::NoopInsights;
#[cfg(feature = "insights")]
use crate::providers::insights::ScheduledInsights;
use crate::providers::llm::{NoopLlm, NoopStt, NoopTts};
use crate::providers::persona::TemplatePersona;
use crate::providers::router::MultiLayerRouter;
use crate::providers::sensor::SensorRegistry;
use crate::providers::skills::BuiltinSkills;
use crate::rules::EventRuleEngine;
use crate::scheduler::{ActionRegistry, AutomationScheduler, ConditionRegistry};
use crate::tools::BuiltinToolRegistry;

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
#[cfg(feature = "llm-anthropic")]
use crate::providers::llm::AnthropicLlm;
#[cfg(feature = "llm-gemini")]
use crate::providers::llm::GeminiLlm;
#[cfg(feature = "llm-openai")]
use crate::providers::llm::OpenAiLlm;
#[cfg(feature = "stt-whisper")]
use crate::providers::stt::WhisperCppStt;
#[cfg(feature = "tts-sovits")]
use crate::providers::tts::GptSovitsTts;

pub fn create_llm(config: &ServerConfig, profile_id: &str) -> anyhow::Result<Box<dyn LlmAdapter>> {
    if profile_id == "noop" {
        tracing::info!(profile = profile_id, "creating LLM adapter (noop)");
        return Ok(Box::new(NoopLlm));
    }
    let profile = config.profiles.llm_profile(profile_id)?;
    if profile.kind != LlmProfileKind::Chat {
        anyhow::bail!("LLM profile '{profile_id}' is not a chat profile");
    }
    let provider = config.llm_provider_profile(&profile.provider)?;
    match provider.kind() {
        LlmProviderKind::OpenAi => {
            #[cfg(feature = "llm-openai")]
            {
                tracing::info!(
                    profile = profile_id,
                    provider = %profile.provider,
                    "creating LLM adapter (OpenAI)"
                );
                Ok(Box::new(OpenAiLlm::for_profile(profile_id)))
            }
            #[cfg(not(feature = "llm-openai"))]
            {
                anyhow::bail!("profile '{profile_id}' requires feature 'llm-openai'")
            }
        }
        LlmProviderKind::Anthropic => {
            #[cfg(feature = "llm-anthropic")]
            {
                tracing::info!(
                    profile = profile_id,
                    provider = %profile.provider,
                    "creating LLM adapter (Anthropic)"
                );
                Ok(Box::new(AnthropicLlm::for_profile(profile_id)))
            }
            #[cfg(not(feature = "llm-anthropic"))]
            {
                anyhow::bail!("profile '{profile_id}' requires feature 'llm-anthropic'")
            }
        }
        LlmProviderKind::Gemini => {
            #[cfg(feature = "llm-gemini")]
            {
                tracing::info!(
                    profile = profile_id,
                    provider = %profile.provider,
                    "creating LLM adapter (Gemini)"
                );
                Ok(Box::new(GeminiLlm::for_profile(profile_id)))
            }
            #[cfg(not(feature = "llm-gemini"))]
            {
                anyhow::bail!("profile '{profile_id}' requires feature 'llm-gemini'")
            }
        }
        LlmProviderKind::Noop => {
            tracing::info!(profile = profile_id, "creating LLM adapter (noop)");
            Ok(Box::new(NoopLlm))
        }
    }
}

pub fn create_stt(name: &str) -> anyhow::Result<Box<dyn SttAdapter>> {
    match name {
        #[cfg(feature = "stt-whisper")]
        "whisper" | "whisper_cpp" => Ok(Box::new(WhisperCppStt::new())),
        #[cfg(not(feature = "stt-whisper"))]
        "whisper" | "whisper_cpp" => anyhow::bail!("STT '{name}' requires feature 'stt-whisper'"),
        "noop" => Ok(Box::new(NoopStt)),
        _ => anyhow::bail!("unknown STT adapter '{name}'"),
    }
}

pub fn create_tts(name: &str) -> anyhow::Result<Box<dyn TtsAdapter>> {
    match name {
        #[cfg(feature = "tts-sovits")]
        "gpt_sovits" | "sovits" => Ok(Box::new(GptSovitsTts::new())),
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
    match provider.kind() {
        LlmProviderKind::OpenAi => {
            #[cfg(feature = "embed-openai")]
            {
                tracing::info!(
                    profile = profile_id,
                    provider = %profile.provider,
                    "creating embedding adapter (OpenAI)"
                );
                Ok(Box::new(OpenAiEmbedding::for_profile(profile_id)))
            }
            #[cfg(not(feature = "embed-openai"))]
            {
                anyhow::bail!("profile '{profile_id}' requires feature 'embed-openai'")
            }
        }
        LlmProviderKind::Noop => {
            tracing::info!(profile = profile_id, "creating embedding adapter (noop)");
            Ok(Box::new(NoopEmbedding))
        }
        LlmProviderKind::Anthropic | LlmProviderKind::Gemini => {
            anyhow::bail!("profile '{profile_id}' selects an unsupported embedding provider")
        }
    }
}

pub fn create_cache() -> Box<dyn CacheBackend> {
    tracing::info!("creating core cache backend (DbCache)");
    Box::new(DbCache::new())
}

pub fn create_skills() -> Box<dyn SkillProvider> {
    tracing::info!("creating core skill registry (BuiltinSkills)");
    Box::new(BuiltinSkills::new())
}

pub fn create_persona() -> Box<dyn PersonaProvider> {
    tracing::info!("creating core persona provider (TemplatePersona)");
    Box::new(TemplatePersona::new())
}

pub fn create_insights(enabled: bool) -> anyhow::Result<Box<dyn InsightStrategy>> {
    if !enabled {
        tracing::info!("insights disabled by policy");
        return Ok(Box::new(NoopInsights));
    }

    #[cfg(feature = "insights")]
    {
        tracing::info!("creating core insights strategy (ScheduledInsights)");
        return Ok(Box::new(ScheduledInsights::new()));
    }

    #[cfg(not(feature = "insights"))]
    {
        anyhow::bail!("enabled insights require feature 'insights'")
    }
}

pub fn create_router() -> Box<dyn RouterStrategy> {
    tracing::info!("creating core router strategy (multi_layer)");
    Box::new(MultiLayerRouter::new())
}

pub fn create_rule_engine() -> Box<dyn RuleEvaluator> {
    tracing::info!("creating core rule engine (EventRuleEngine)");
    Box::new(EventRuleEngine::new())
}

pub fn create_scheduler(
    conditions: Arc<ConditionRegistry>,
    actions: Arc<ActionRegistry>,
) -> Box<dyn SchedulerBackend> {
    tracing::info!("creating core scheduler (AutomationScheduler)");
    Box::new(AutomationScheduler::with_registries(conditions, actions))
}

/// Create tools with sensor registry integration (includes sensor_read + sensor_list).
pub fn create_tools_with_sensors(
    sensors: Arc<SensorRegistry>,
    channels: Arc<ChannelRegistry>,
) -> Box<dyn ToolRegistry> {
    tracing::info!("creating core tool registry with sensors (BuiltinToolRegistry)");
    let registry = BuiltinToolRegistry::with_sensors(sensors);
    #[cfg(feature = "channel-embedded")]
    registry.register_executor(Arc::new(EmbeddedVolumeTool::new(channels)));
    #[cfg(not(feature = "channel-embedded"))]
    let _ = channels;
    Box::new(registry)
}

pub fn create_channel(
    config: &ChannelConfig,
    event_bus: EventBus,
) -> anyhow::Result<Box<dyn ChannelAdapter>> {
    match config {
        #[cfg(feature = "channel-embedded")]
        ChannelConfig::Embedded {
            id,
            owner_user_id,
            expected_device_id,
            expected_device_public_key,
            endpoint,
            discovery_service_name,
            discovery_timeout_sec,
            reconnect_interval_sec,
            reconnect_max_backoff_sec,
            grpc_timeout_sec,
            ..
        } => {
            tracing::info!(channel_id = %id, "creating EmbeddedChannel");
            Ok(Box::new(EmbeddedChannel::new(
                EmbeddedChannelConfig {
                    id: id.clone(),
                    owner_user_id: *owner_user_id,
                    expected_device_id: expected_device_id.clone(),
                    expected_device_public_key: expected_device_public_key.clone(),
                    endpoint: endpoint.clone(),
                    discovery_service_name: discovery_service_name.clone(),
                    discovery_timeout_sec: *discovery_timeout_sec,
                    reconnect_interval_sec: *reconnect_interval_sec,
                    reconnect_max_backoff_sec: *reconnect_max_backoff_sec,
                    grpc_timeout_sec: *grpc_timeout_sec,
                },
                event_bus,
            )))
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
                CliChannel::new(id.clone(), event_bus).with_owner_user_id(*owner_user_id),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapters_require_explicit_valid_selection() {
        assert!(create_stt("noop").is_ok());
        assert!(create_tts("noop").is_ok());
        assert!(create_insights(false).is_ok());
        assert!(create_stt("whispr").is_err());
        assert!(create_tts("sovit").is_err());
        #[cfg(not(feature = "stt-whisper"))]
        assert!(create_stt("whisper").is_err());
        #[cfg(not(feature = "tts-sovits"))]
        assert!(create_tts("sovits").is_err());
        #[cfg(not(feature = "insights"))]
        assert!(create_insights(true).is_err());
    }

    #[cfg(not(feature = "channel-embedded"))]
    #[test]
    fn embedded_channel_without_feature_returns_error() {
        let config = ChannelConfig::Embedded {
            id: "pi-living-room".to_string(),
            owner_user_id: None,
            expected_device_id: None,
            expected_device_public_key: None,
            enabled: true,
            endpoint: String::new(),
            discovery_service_name: "_alia-embedded._tcp".to_string(),
            discovery_timeout_sec: 3,
            reconnect_interval_sec: 5,
            reconnect_max_backoff_sec: 60,
            grpc_timeout_sec: 10,
        };

        match create_channel(&config, EventBus::new(1)) {
            Ok(_) => panic!("embedded channel creation should fail without channel-embedded"),
            Err(error) => assert!(error.to_string().contains("channel-embedded")),
        }
    }
}
