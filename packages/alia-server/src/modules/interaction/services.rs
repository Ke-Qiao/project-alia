use crate::expression::ExpressionRuntime;
use crate::providers::llm::LlmService;
use crate::{
    modules::interaction::{
        cache::CacheStore, conversation::ConversationStore, persona::PersonaService,
        routing::RouterService, skills::SkillRegistry,
    },
    providers::adapter::{EmbeddingAdapter, ToolRegistry, TtsAdapter},
};
use std::sync::Arc;

/// Services used by interaction use cases, assembled explicitly by bootstrap.
/// No infrastructure registry, scheduler, STT, or lifecycle ownership is exposed here.
pub struct InteractionServices {
    pub llm: Arc<LlmService>,
    pub minor_llm: Arc<LlmService>,
    pub major_llm: Arc<LlmService>,
    pub memory_capture_llm: Option<Arc<LlmService>>,
    pub sentence_expression_llm: Option<Arc<LlmService>>,
    pub tts: Arc<dyn TtsAdapter>,
    pub embedding: Arc<dyn EmbeddingAdapter>,
    pub conversation: Arc<dyn ConversationStore>,
    pub responses: Arc<super::response::ResponseCommitter>,
    pub memory: Arc<crate::modules::memory::MemoryService>,
    pub context: Arc<crate::modules::memory::context::ContextService>,
    pub cache: Arc<dyn CacheStore>,
    pub skills: Arc<SkillRegistry>,
    pub persona: Arc<PersonaService>,
    pub router: Arc<RouterService>,
    pub tools: Arc<dyn ToolRegistry>,
    pub expression: Arc<ExpressionRuntime>,
    pub delivery: Arc<crate::modules::delivery::DeliveryService>,
}
