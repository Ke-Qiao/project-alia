use crate::providers::adapter::{
    CacheBackend, EmbeddingAdapter, LlmAdapter, MeditationStrategy, MemoryBackend,
    PersonaProvider, RouterStrategy, RuleEvaluator, SkillProvider, SttAdapter, TtsAdapter,
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

pub fn create_llm(_name: &str) -> Box<dyn LlmAdapter> {
    // TODO: match on name to create OpenAI/Ollama implementations
    // For now return noop
    tracing::info!(module = _name, "creating LLM adapter (noop placeholder)");
    Box::new(NoopLlm)
}

pub fn create_stt(_name: &str) -> Box<dyn SttAdapter> {
    tracing::info!(module = _name, "creating STT adapter (noop placeholder)");
    Box::new(NoopStt)
}

pub fn create_tts(_name: &str) -> Box<dyn TtsAdapter> {
    tracing::info!(module = _name, "creating TTS adapter (noop placeholder)");
    Box::new(NoopTts)
}

pub fn create_embedding(_name: &str) -> Box<dyn EmbeddingAdapter> {
    tracing::info!(module = _name, "creating embedding adapter (noop placeholder)");
    Box::new(NoopEmbedding)
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
