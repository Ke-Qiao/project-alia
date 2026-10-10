//! LLM adapters for AI chat completion.
//!
//! Available implementations:
//! - `OpenAiLlm`: OpenAI GPT models (gpt-4o, gpt-4-turbo, etc.)
//! - `AnthropicLlm`: Anthropic Messages API models
//! - `GeminiLlm`: Google Gemini API models
//! - `NoopLlm`: No-op placeholder

#[cfg(feature = "llm-anthropic")]
pub mod anthropic;
#[cfg(any(
    feature = "llm-openai",
    feature = "llm-anthropic",
    feature = "llm-gemini"
))]
pub mod errors;
#[cfg(feature = "llm-gemini")]
pub mod gemini;
#[cfg(feature = "llm-openai")]
pub mod openai;
pub mod streaming;

#[cfg(feature = "llm-anthropic")]
pub use anthropic::AnthropicLlm;
#[cfg(feature = "llm-gemini")]
pub use gemini::GeminiLlm;
#[cfg(feature = "llm-openai")]
pub use openai::OpenAiLlm;

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use futures::stream;

use crate::modules::platform::lifecycle::ModuleLifecycle;
use crate::providers::adapter::{
    AudioEncoding, ChatMessage, LlmAdapter, LlmCapabilities, SttAdapter, TranscribeChunk,
    TtsAdapter,
};

// ========== Noop LLM ==========

pub struct NoopLlm;

#[async_trait]
impl ModuleLifecycle for NoopLlm {
    fn name(&self) -> &'static str {
        "noop_llm"
    }
}

#[async_trait]
impl LlmAdapter for NoopLlm {
    async fn llm_capabilities(&self) -> LlmCapabilities {
        LlmCapabilities::minimal(self.name())
    }

    async fn chat(&self, _messages: &[ChatMessage]) -> anyhow::Result<String> {
        Ok("LLM module not configured".into())
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
        Ok(Box::pin(stream::once(async {
            Ok("LLM module not configured".into())
        })))
    }
}

// ========== Noop STT ==========

pub struct NoopStt;

#[async_trait]
impl ModuleLifecycle for NoopStt {
    fn name(&self) -> &'static str {
        "noop_stt"
    }
}

#[async_trait]
impl SttAdapter for NoopStt {
    async fn transcribe_result(
        &self,
        _audio: &[u8],
        _encoding: AudioEncoding,
    ) -> anyhow::Result<crate::providers::adapter::TranscriptionResult> {
        Ok(crate::providers::adapter::TranscriptionResult::plain(
            self.name(),
            String::new(),
        ))
    }

    async fn transcribe_stream(
        &self,
        _audio_stream: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
        _encoding: AudioEncoding,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<TranscribeChunk>> + Send>>> {
        Ok(Box::pin(stream::empty()))
    }
}

// ========== Noop TTS ==========

pub struct NoopTts;

#[async_trait]
impl ModuleLifecycle for NoopTts {
    fn name(&self) -> &'static str {
        "noop_tts"
    }
}

#[async_trait]
impl TtsAdapter for NoopTts {
    async fn synthesize_request(
        &self,
        _: &crate::providers::adapter::TtsRequest,
        budget: crate::modules::delivery::audio::ResponseAudioBudget,
    ) -> anyhow::Result<crate::modules::delivery::audio::EncodedAudio> {
        budget.copy_from_slice(&[]).await
    }
    async fn synthesize_stream_request(
        &self,
        _: &crate::providers::adapter::TtsRequest,
        _budget: crate::modules::delivery::audio::ResponseAudioBudget,
    ) -> anyhow::Result<crate::providers::adapter::TtsAudioStream> {
        Ok(Box::pin(stream::empty()))
    }
}

pub(crate) mod response;
