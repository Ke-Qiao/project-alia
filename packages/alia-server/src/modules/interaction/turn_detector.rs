use alia_core::voice::{DetectionId, PcmWindow, TurnObservation};
use async_trait::async_trait;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum DetectionError {
    #[error("turn detection was cancelled")]
    Cancelled,
    #[error("turn detector is busy")]
    Busy,
    #[error("turn detector deadline exceeded")]
    Timeout,
    #[error("turn detector unavailable: {0}")]
    Unavailable(String),
    #[error("invalid turn detector response: {0}")]
    InvalidResponse(String),
}

/// A model supplies evidence; the interaction runtime decides whether to take or yield a turn.
#[async_trait]
pub trait TurnDetector: Send + Sync {
    async fn detect(
        &self,
        id: DetectionId,
        audio: &PcmWindow,
        cancel: &CancellationToken,
    ) -> Result<TurnObservation, DetectionError>;
}
