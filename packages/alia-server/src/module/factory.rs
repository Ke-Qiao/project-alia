use crate::providers::adapter::{
    CacheBackend, EmbeddingAdapter, LlmAdapter, MeditationStrategy, MemoryBackend,
    PersonaProvider, RouterStrategy, RuleEvaluator, SkillProvider, SttAdapter, TtsAdapter,
    SchedulerBackend, ToolRegistry,
};
use crate::providers::cache::NoopCache;
use crate::providers::embedding::NoopEmbedding;
use crate::providers::llm::{NoopLlm, NoopStt, NoopTts};
use crate::providers::meditation::NoopMeditation;
use crate::providers::memory::NoopMemory;
use crate::providers::persona::NoopPersona;
use crate::providers::router::PassthroughRouter;
use crate::providers::skills::NoopSkills;
use crate::rules::NoopRuleEngine;
use crate::scheduler::{NoopScheduler, CronScheduler};
use crate::tools::{NoopToolRegistry, BuiltinToolRegistry};

#[cfg(feature = "llm-openai")]
use crate::providers::llm::OpenAiLlm;
#[cfg(feature = "llm-ollama")]
use crate::providers::llm::OllamaLlm;
#[cfg(feature = "stt-whisper")]
use crate::providers::stt::WhisperCppStt;
#[cfg(feature = "tts-sovits")]
use crate::providers::tts::GptSovitsTts;
#[cfg(feature = "embed-ollama")]
use crate::providers::embedding::OllamaEmbedding;
#[cfg(feature = "embed-openai")]
use crate::providers::embedding::OpenAiEmbedding;

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
        "whisper" => {
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

pub fn create_memory(_name: &str) -> Box<dyn MemoryBackend> {
    tracing::info!(module = _name, "creating memory backend (noop placeholder)");
    Box::new(NoopMemory)
}

pub fn create_cache(_name: &str) -> Box<dyn CacheBackend> {
    tracing::info!(module = _name, "creating cache backend (noop placeholder)");
    Box::new(NoopCache)
}

pub fn create_skills(_name: &str) -> Box<dyn SkillProvider> {
    tracing::info!(module = _name, "creating skill provider (noop placeholder)");
    Box::new(NoopSkills)
}

pub fn create_persona(_name: &str) -> Box<dyn PersonaProvider> {
    tracing::info!(module = _name, "creating persona provider (noop placeholder)");
    Box::new(NoopPersona)
}

pub fn create_meditation(_name: &str) -> Box<dyn MeditationStrategy> {
    tracing::info!(module = _name, "creating meditation strategy (noop placeholder)");
    Box::new(NoopMeditation)
}

pub fn create_router(_name: &str) -> Box<dyn RouterStrategy> {
    tracing::info!(module = _name, "creating router strategy (passthrough)");
    Box::new(PassthroughRouter)
}

pub fn create_rule_engine(_name: &str) -> Box<dyn RuleEvaluator> {
    tracing::info!(module = _name, "creating rule engine (noop placeholder)");
    Box::new(NoopRuleEngine)
}

pub fn create_scheduler(name: &str) -> Box<dyn SchedulerBackend> {
    match name {
        "cron" => {
            tracing::info!(module = name, "creating scheduler (CronScheduler)");
            Box::new(CronScheduler::new())
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
            tracing::info!(module = name, "creating tool registry (BuiltinToolRegistry)");
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
