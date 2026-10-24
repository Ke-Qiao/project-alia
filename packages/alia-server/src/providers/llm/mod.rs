//! Rig-backed model profiles and no-op audio providers.
mod service;
pub use service::{LlmService, TextStream};
pub(crate) use service::{MODEL_TIMEOUT, text_request, validate_response};

use crate::modules::platform::lifecycle::ModuleLifecycle;
use crate::providers::adapter::{AudioEncoding, SttAdapter, TranscribeChunk, TtsAdapter};
use async_trait::async_trait;
use futures::{Stream, stream};
use std::pin::Pin;

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
