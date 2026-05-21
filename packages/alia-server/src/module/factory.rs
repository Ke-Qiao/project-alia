use std::sync::Arc;

use crate::channels::ChannelRegistry;
use crate::channels::adapter::ChannelAdapter;
use crate::channels::cli::CliChannel;
use crate::channels::dialogue_control_tool::EndContinuousDialogueTool;
#[cfg(feature = "channel-embedded")]
use crate::channels::embedded::{EmbeddedChannel, EmbeddedChannelConfig};
#[cfg(feature = "channel-embedded")]
use crate::channels::embedded_volume_tool::EmbeddedVolumeTool;
use crate::config::{ChannelConfig, LlmProfileKind, LlmProviderKind, MemorySection, ServerConfig};
use crate::event::EventBus;
use crate::providers::adapter::{
    CacheBackend, EmbeddingAdapter, InsightStrategy, LlmAdapter, MemoryBackend, PersonaProvider,
    RouterStrategy, RuleEvaluator, SchedulerBackend, SkillProvider, SttAdapter, ToolRegistry,
    TtsAdapter,
};
use crate::providers::cache::DbCache;
use crate::providers::embedding::NoopEmbedding;
use crate::providers::insights::NoopInsights;
#[cfg(feature = "insights")]
use crate::providers::insights::ScheduledInsights;
use crate::providers::llm::{NoopLlm, NoopStt, NoopTts};
use crate::providers::memory::{AliaMemory, NoopMemory};
use crate::providers::persona::TemplatePersona;
use crate::providers::router::MultiLayerRouter;
use crate::providers::sensor::SensorRegistry;
use crate::providers::skills::BuiltinSkills;
use crate::rules::EventRuleEngine;
use crate::scheduler::{ActionRegistry, AutomationScheduler, ConditionRegistry};
use crate::tools::BuiltinToolRegistry;

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
    let provider = match config.llm_provider_profile(&profile.provider) {
        Ok(provider) => provider,
        Err(error) => {
            tracing::warn!(
                profile = profile_id,
                provider = %profile.provider,
                error = %error,
                "unknown LLM provider profile, using noop"
            );
            return Ok(Box::new(NoopLlm));
        }
    };
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
                tracing::warn!(
                    profile = profile_id,
                    provider = %profile.provider,
                    "OpenAI LLM feature disabled, using noop"
                );
                Ok(Box::new(NoopLlm))
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
                tracing::warn!(
                    profile = profile_id,
                    provider = %profile.provider,
                    "Anthropic LLM feature disabled, using noop"
                );
                Ok(Box::new(NoopLlm))
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
                tracing::warn!(
                    profile = profile_id,
                    provider = %profile.provider,
                    "Gemini LLM feature disabled, using noop"
                );
                Ok(Box::new(NoopLlm))
            }
        }
        LlmProviderKind::Noop => {
            tracing::info!(profile = profile_id, "creating LLM adapter (noop)");
            Ok(Box::new(NoopLlm))
        }
    }
}

pub fn create_stt(name: &str) -> Box<dyn SttAdapter> {
    match name {
        #[cfg(feature = "stt-whisper")]
        "whisper" | "whisper_cpp" => {
            tracing::info!(module = name, "creating STT adapter (Whisper)");
            Box::new(WhisperCppStt::new())
        }
        "noop" => {
            tracing::info!(module = name, "creating STT adapter (noop)");
            Box::new(NoopStt)
        }
        _ => {
            tracing::warn!(module = name, "unknown STT adapter, using noop");
            Box::new(NoopStt)
        }
    }
}

pub fn create_tts(name: &str) -> Box<dyn TtsAdapter> {
    match name {
        #[cfg(feature = "tts-sovits")]
        "gpt_sovits" | "sovits" => {
            tracing::info!(module = name, "creating TTS adapter (GPT-SoVITS)");
            Box::new(GptSovitsTts::new())
        }
        "noop" => {
            tracing::info!(module = name, "creating TTS adapter (noop)");
            Box::new(NoopTts)
        }
        _ => {
            tracing::warn!(module = name, "unknown TTS adapter, using noop");
            Box::new(NoopTts)
        }
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
    let provider = match config.llm_provider_profile(&profile.provider) {
        Ok(provider) => provider,
        Err(error) => {
            tracing::warn!(
                profile = profile_id,
                provider = %profile.provider,
                error = %error,
                "unknown embedding provider profile, using noop"
            );
            return Ok(Box::new(NoopEmbedding));
        }
    };
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
                tracing::warn!(
                    profile = profile_id,
                    provider = %profile.provider,
                    "OpenAI embedding feature disabled, using noop"
                );
                Ok(Box::new(NoopEmbedding))
            }
        }
        LlmProviderKind::Noop => {
            tracing::info!(profile = profile_id, "creating embedding adapter (noop)");
            Ok(Box::new(NoopEmbedding))
        }
        LlmProviderKind::Anthropic | LlmProviderKind::Gemini => {
            tracing::warn!(
                profile = profile_id,
                provider = %profile.provider,
                "unknown embedding adapter, using noop"
            );
            Ok(Box::new(NoopEmbedding))
        }
    }
}

pub fn create_memory(config: &MemorySection) -> Box<dyn MemoryBackend> {
    if !config.enabled {
        tracing::info!("memory disabled by config");
        return Box::new(NoopMemory);
    }

    tracing::info!("creating built-in memory system (AliaMemory)");
    Box::new(AliaMemory::new())
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

pub fn create_insights(enabled: bool) -> Box<dyn InsightStrategy> {
    if !enabled {
        tracing::info!("insights disabled by policy");
        return Box::new(NoopInsights);
    }

    #[cfg(feature = "insights")]
    {
        tracing::info!("creating core insights strategy (ScheduledInsights)");
        return Box::new(ScheduledInsights::new());
    }

    #[cfg(not(feature = "insights"))]
    {
        tracing::warn!("insights feature is disabled; using noop insights strategy");
        Box::new(NoopInsights)
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
    registry.register_executor(Arc::new(EndContinuousDialogueTool::new(channels.clone())));
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
            endpoint,
            discovery_service_name,
            discovery_timeout_sec,
            reconnect_interval_sec,
            reconnect_max_backoff_sec,
            grpc_timeout_sec,
            voice_turn_interrupt_enabled,
            cancel_streaming_tts_on_interrupt,
            discard_interrupted_turn_history,
            ..
        } => {
            tracing::info!(channel_id = %id, "creating EmbeddedChannel");
            Ok(Box::new(EmbeddedChannel::new(
                EmbeddedChannelConfig {
                    id: id.clone(),
                    endpoint: endpoint.clone(),
                    discovery_service_name: discovery_service_name.clone(),
                    discovery_timeout_sec: *discovery_timeout_sec,
                    reconnect_interval_sec: *reconnect_interval_sec,
                    reconnect_max_backoff_sec: *reconnect_max_backoff_sec,
                    grpc_timeout_sec: *grpc_timeout_sec,
                    voice_turn_interrupt_enabled: *voice_turn_interrupt_enabled,
                    cancel_streaming_tts_on_interrupt: *cancel_streaming_tts_on_interrupt,
                    discard_interrupted_turn_history: *discard_interrupted_turn_history,
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
        ChannelConfig::Cli { id, .. } => {
            tracing::info!(channel_id = %id, "creating CliChannel");
            Ok(Box::new(CliChannel::new(id.clone(), event_bus)))
        }
    }
}

#[cfg(all(test, not(feature = "channel-embedded")))]
mod tests {
    use super::*;

    #[test]
    fn embedded_channel_without_feature_returns_error() {
        let config = ChannelConfig::Embedded {
            id: "pi-living-room".to_string(),
            enabled: true,
            endpoint: String::new(),
            discovery_service_name: "_alia-embedded._tcp".to_string(),
            discovery_timeout_sec: 3,
            reconnect_interval_sec: 5,
            reconnect_max_backoff_sec: 60,
            grpc_timeout_sec: 10,
            voice_turn_interrupt_enabled: true,
            cancel_streaming_tts_on_interrupt: true,
            discard_interrupted_turn_history: true,
        };

        match create_channel(&config, EventBus::new(1)) {
            Ok(_) => panic!("embedded channel creation should fail without channel-embedded"),
            Err(error) => assert!(error.to_string().contains("channel-embedded")),
        }
    }
}
