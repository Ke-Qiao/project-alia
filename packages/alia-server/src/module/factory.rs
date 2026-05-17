use std::sync::Arc;

use crate::channels::ChannelRegistry;
use crate::channels::adapter::ChannelAdapter;
use crate::channels::cli::CliChannel;
#[cfg(feature = "channel-embedded")]
use crate::channels::embedded::{EmbeddedChannel, EmbeddedChannelConfig};
#[cfg(feature = "channel-embedded")]
use crate::channels::embedded_volume_tool::EmbeddedVolumeTool;
use crate::config::{ChannelConfig, Mem0ProviderSection, MemorySection};
use crate::event::EventBus;
use crate::providers::adapter::{
    CacheBackend, EmbeddingAdapter, InsightStrategy, LlmAdapter, MemoryBackend, PersonaProvider,
    RouterStrategy, RuleEvaluator, SchedulerBackend, SkillProvider, SttAdapter, ToolRegistry,
    TtsAdapter,
};
#[cfg(feature = "cache-semantic")]
use crate::providers::cache::TwoLayerCache;
use crate::providers::cache::{DbCache, NoopCache};
use crate::providers::embedding::NoopEmbedding;
use crate::providers::insights::NoopInsights;
#[cfg(feature = "insights")]
use crate::providers::insights::ScheduledInsights;
use crate::providers::llm::{NoopLlm, NoopStt, NoopTts};
#[cfg(feature = "memory-mem0")]
use crate::providers::memory::external::ExternalMemoryBackend;
#[cfg(feature = "memory-mem0")]
use crate::providers::memory::mem0::{Mem0Client, Mem0Config};
use crate::providers::memory::{NoopMemory, SimpleMemory};
use crate::providers::persona::{NoopPersona, TemplatePersona};
#[cfg(feature = "llm-ollama")]
use crate::providers::router::MultiLayerRouter;
use crate::providers::router::PassthroughRouter;
use crate::providers::sensor::SensorRegistry;
use crate::providers::skills::{BuiltinSkills, NoopSkills};
use crate::rules::{NoopRuleEngine, SimpleRuleEngine};
use crate::scheduler::{ConditionRegistry, CronScheduler, NoopScheduler};
use crate::tools::{BuiltinToolRegistry, NoopToolRegistry};

#[cfg(feature = "embed-ollama")]
use crate::providers::embedding::OllamaEmbedding;
#[cfg(feature = "embed-openai")]
use crate::providers::embedding::OpenAiEmbedding;
#[cfg(feature = "llm-ollama")]
use crate::providers::llm::OllamaLlm;
#[cfg(feature = "llm-openai")]
use crate::providers::llm::OpenAiLlm;
#[cfg(feature = "stt-whisper")]
use crate::providers::stt::WhisperCppStt;
#[cfg(feature = "tts-sovits")]
use crate::providers::tts::GptSovitsTts;

pub fn create_llm(name: &str) -> Box<dyn LlmAdapter> {
    match name {
        #[cfg(feature = "llm-openai")]
        "openai" => {
            tracing::info!(module = name, "creating LLM adapter (OpenAI)");
            Box::new(OpenAiLlm::new())
        }
        #[cfg(feature = "llm-ollama")]
        "ollama" => {
            tracing::info!(module = name, "creating LLM adapter (Ollama)");
            Box::new(OllamaLlm::new())
        }
        "noop" => {
            tracing::info!(module = name, "creating LLM adapter (noop)");
            Box::new(NoopLlm)
        }
        _ => {
            tracing::warn!(module = name, "unknown LLM adapter, using noop");
            Box::new(NoopLlm)
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

pub fn create_embedding(name: &str) -> Box<dyn EmbeddingAdapter> {
    match name {
        #[cfg(feature = "embed-ollama")]
        "ollama" => {
            tracing::info!(module = name, "creating embedding adapter (Ollama)");
            Box::new(OllamaEmbedding::new())
        }
        #[cfg(feature = "embed-openai")]
        "openai" => {
            tracing::info!(module = name, "creating embedding adapter (OpenAI)");
            Box::new(OpenAiEmbedding::new())
        }
        "noop" => {
            tracing::info!(module = name, "creating embedding adapter (noop)");
            Box::new(NoopEmbedding)
        }
        _ => {
            tracing::warn!(module = name, "unknown embedding adapter, using noop");
            Box::new(NoopEmbedding)
        }
    }
}

pub fn create_memory(
    name: &str,
    config: &MemorySection,
    _mem0_config: &Mem0ProviderSection,
) -> Box<dyn MemoryBackend> {
    if !config.enabled {
        tracing::info!(module = name, "memory disabled by config");
        return Box::new(NoopMemory);
    }

    match name {
        "simple" => {
            tracing::info!(module = name, "creating memory backend (SimpleMemory)");
            Box::new(SimpleMemory::new())
        }
        #[cfg(feature = "memory-mem0")]
        "mem0" => {
            tracing::info!(module = name, "creating memory backend (mem0)");
            Box::new(ExternalMemoryBackend::new(Mem0Client::new(
                Mem0Config::from_config(config, _mem0_config),
            )))
        }
        "noop" => {
            tracing::info!(module = name, "creating memory backend (noop)");
            Box::new(NoopMemory)
        }
        _ => {
            tracing::warn!(module = name, "unknown memory backend, using noop");
            Box::new(NoopMemory)
        }
    }
}

pub fn create_cache(name: &str) -> Box<dyn CacheBackend> {
    match name {
        "db" => {
            tracing::info!(module = name, "creating cache backend (DbCache)");
            Box::new(DbCache::new())
        }
        #[cfg(feature = "cache-semantic")]
        "two_layer" => {
            tracing::info!(module = name, "creating cache backend (TwoLayerCache)");
            Box::new(TwoLayerCache::new())
        }
        "noop" => {
            tracing::info!(module = name, "creating cache backend (noop)");
            Box::new(NoopCache)
        }
        _ => {
            tracing::warn!(module = name, "unknown cache backend, using db");
            Box::new(DbCache::new())
        }
    }
}

pub fn create_skills(name: &str) -> Box<dyn SkillProvider> {
    match name {
        "builtin" => {
            tracing::info!(module = name, "creating skill provider (BuiltinSkills)");
            Box::new(BuiltinSkills::new())
        }
        "noop" => {
            tracing::info!(module = name, "creating skill provider (noop)");
            Box::new(NoopSkills)
        }
        _ => {
            tracing::warn!(module = name, "unknown skill provider, using builtin");
            Box::new(BuiltinSkills::new())
        }
    }
}

pub fn create_persona(name: &str) -> Box<dyn PersonaProvider> {
    match name {
        "template" => {
            tracing::info!(module = name, "creating persona provider (TemplatePersona)");
            Box::new(TemplatePersona::new())
        }
        "noop" => {
            tracing::info!(module = name, "creating persona provider (noop)");
            Box::new(NoopPersona)
        }
        _ => {
            tracing::warn!(module = name, "unknown persona provider, using template");
            Box::new(TemplatePersona::new())
        }
    }
}

pub fn create_insights(name: &str) -> Box<dyn InsightStrategy> {
    match name {
        #[cfg(feature = "insights")]
        "scheduled" => {
            tracing::info!(
                module = name,
                "creating insights strategy (ScheduledInsights)"
            );
            Box::new(ScheduledInsights::new())
        }
        "noop" => {
            tracing::info!(module = name, "creating insights strategy (noop)");
            Box::new(NoopInsights)
        }
        _ => {
            tracing::warn!(module = name, "unknown insights strategy, using noop");
            Box::new(NoopInsights)
        }
    }
}

pub fn create_router(name: &str) -> Box<dyn RouterStrategy> {
    match name {
        #[cfg(feature = "llm-ollama")]
        "multi_layer" => {
            tracing::info!(module = name, "creating router strategy (MultiLayerRouter)");
            Box::new(MultiLayerRouter::new())
        }
        "passthrough" => {
            tracing::info!(module = name, "creating router strategy (passthrough)");
            Box::new(PassthroughRouter)
        }
        _ => {
            tracing::warn!(module = name, "unknown router strategy, using passthrough");
            Box::new(PassthroughRouter)
        }
    }
}

pub fn create_rule_engine(name: &str) -> Box<dyn RuleEvaluator> {
    match name {
        "simple" => {
            tracing::info!(module = name, "creating rule engine (SimpleRuleEngine)");
            Box::new(SimpleRuleEngine::new())
        }
        "noop" => {
            tracing::info!(module = name, "creating rule engine (noop)");
            Box::new(NoopRuleEngine)
        }
        _ => {
            tracing::warn!(module = name, "unknown rule engine, using simple");
            Box::new(SimpleRuleEngine::new())
        }
    }
}

pub fn create_scheduler(
    name: &str,
    conditions: Arc<ConditionRegistry>,
) -> Box<dyn SchedulerBackend> {
    match name {
        "cron" => {
            tracing::info!(module = name, "creating scheduler (CronScheduler)");
            Box::new(CronScheduler::with_conditions(conditions))
        }
        "noop" => {
            tracing::info!(module = name, "creating scheduler (noop)");
            Box::new(NoopScheduler)
        }
        _ => {
            tracing::warn!(module = name, "unknown scheduler, using noop");
            Box::new(NoopScheduler)
        }
    }
}

pub fn create_tools(name: &str) -> Box<dyn ToolRegistry> {
    match name {
        "builtin" => {
            tracing::info!(
                module = name,
                "creating tool registry (BuiltinToolRegistry)"
            );
            Box::new(BuiltinToolRegistry::new())
        }
        "noop" => {
            tracing::info!(module = name, "creating tool registry (noop)");
            Box::new(NoopToolRegistry)
        }
        _ => {
            tracing::warn!(module = name, "unknown tool registry, using noop");
            Box::new(NoopToolRegistry)
        }
    }
}

/// Create tools with sensor registry integration (includes sensor_read + sensor_list).
pub fn create_tools_with_sensors(
    name: &str,
    sensors: Arc<SensorRegistry>,
    channels: Arc<ChannelRegistry>,
) -> Box<dyn ToolRegistry> {
    match name {
        "builtin" => {
            tracing::info!(
                module = name,
                "creating tool registry with sensors (BuiltinToolRegistry)"
            );
            let registry = BuiltinToolRegistry::with_sensors(sensors);
            #[cfg(feature = "channel-embedded")]
            registry.register_executor(Arc::new(EmbeddedVolumeTool::new(channels)));
            #[cfg(not(feature = "channel-embedded"))]
            let _ = channels;
            Box::new(registry)
        }
        _ => create_tools(name),
    }
}

pub fn create_channel(config: &ChannelConfig, event_bus: EventBus) -> Box<dyn ChannelAdapter> {
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
            Box::new(EmbeddedChannel::new(
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
            ))
        }
        #[cfg(not(feature = "channel-embedded"))]
        ChannelConfig::Embedded { id, .. } => {
            tracing::warn!(
                channel_id = %id,
                "embedded channel not available (compile without 'channel-embedded' feature)"
            );
            panic!("embedded channel requires 'channel-embedded' feature");
        }
        ChannelConfig::Cli { id, .. } => {
            tracing::info!(channel_id = %id, "creating CliChannel");
            Box::new(CliChannel::new(id.clone(), event_bus))
        }
    }
}
