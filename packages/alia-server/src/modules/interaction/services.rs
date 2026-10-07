use crate::expression::ExpressionRuntime;
use crate::providers::adapter::{
    CacheBackend, ConversationBackend, EmbeddingAdapter, LlmAdapter, PersonaProvider,
    RouterStrategy, SkillProvider, ToolRegistry, TtsAdapter,
};
use std::sync::Arc;

/// Services used by interaction use cases, assembled explicitly by bootstrap.
/// No infrastructure registry, scheduler, STT, or lifecycle ownership is exposed here.
pub struct InteractionServices {
    pub llm: Arc<dyn LlmAdapter>,
    pub minor_llm: Arc<dyn LlmAdapter>,
    pub major_llm: Arc<dyn LlmAdapter>,
    pub memory_capture_llm: Option<Arc<dyn LlmAdapter>>,
    pub sentence_expression_llm: Option<Arc<dyn LlmAdapter>>,
    pub tts: Arc<dyn TtsAdapter>,
    pub embedding: Arc<dyn EmbeddingAdapter>,
    pub conversation: Arc<dyn ConversationBackend>,
    pub memory: Arc<crate::modules::memory::MemoryService>,
    pub context: Arc<crate::modules::memory::context::ContextService>,
    pub cache: Arc<dyn CacheBackend>,
    pub skills: Arc<dyn SkillProvider>,
    pub persona: Arc<dyn PersonaProvider>,
    pub router: Arc<dyn RouterStrategy>,
    pub tools: Arc<dyn ToolRegistry>,
    pub expression: Arc<ExpressionRuntime>,
    pub delivery: Arc<crate::modules::delivery::DeliveryService>,
}
