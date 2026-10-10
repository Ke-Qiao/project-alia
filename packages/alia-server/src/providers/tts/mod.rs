//! TTS adapters for text-to-speech synthesis.
//!
//! Available implementations:
//! - `GptSovitsTts`: GPT-SoVITS server for high-quality voice synthesis
//! - `NoopTts`: No-op placeholder (re-exported from llm module)

#[cfg(feature = "tts-sovits")]
pub mod gpt_sovits;

#[cfg(feature = "tts-sovits")]
mod http_body;

#[cfg(feature = "tts-sovits")]
pub use gpt_sovits::GptSovitsTts;

#[allow(unused_imports)]
pub use crate::providers::llm::NoopTts;
