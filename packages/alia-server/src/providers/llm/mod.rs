//! LLM adapters for AI chat completion.
//!
//! Available implementations:
//! - `OpenAiLlm`: OpenAI GPT models (gpt-4o, gpt-4-turbo, etc.)
//! - `OllamaLlm`: Local Ollama models (qwen2.5, llama3, etc.)
//! - `NoopLlm`: No-op placeholder

#[cfg(feature = "llm-openai")]
pub mod openai;
#[cfg(feature = "llm-ollama")]
pub mod ollama;

#[cfg(feature = "llm-openai")]
pub use openai::OpenAiLlm;
#[cfg(feature = "llm-ollama")]
pub use ollama::OllamaLlm;

use std::pin::Pin;

use async_trait::async_trait;
use futures::stream;
use futures::Stream;

use crate::providers::adapter::{AudioEncoding, ChatMessage, LlmAdapter, SttAdapter, TranscribeChunk, TtsAdapter};
use crate::module::ModuleLifecycle;

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
    async fn transcribe(&self, _audio: &[u8], _encoding: AudioEncoding) -> anyhow::Result<String> {
        Ok(String::new())
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
    async fn synthesize(&self, _text: &str) -> anyhow::Result<Vec<u8>> {
        Ok(Vec::new())
    }

    async fn synthesize_stream(
        &self,
        _text: &str,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<Vec<u8>>> + Send>>> {
        Ok(Box::pin(stream::empty()))
    }
}
