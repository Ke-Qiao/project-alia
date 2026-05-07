//! STT adapters for speech-to-text transcription.
//!
//! Available implementations:
//! - `WhisperCppStt`: whisper.cpp server for local transcription
//! - `NoopStt`: No-op placeholder (re-exported from llm module)

#[cfg(feature = "stt-whisper")]
pub mod whisper;

#[cfg(feature = "stt-whisper")]
pub use whisper::WhisperCppStt;

#[allow(unused_imports)]
pub use crate::providers::llm::NoopStt;
