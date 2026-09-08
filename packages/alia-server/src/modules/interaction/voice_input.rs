//! Bounded application entrance for recognized speech. This is intentionally
//! separate from the lossy broadcast bus used for observations and UI updates.

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug)]
pub struct RecognizedVoiceInput {
    pub channel_id: String,
    pub user_id: String,
    pub transcript: String,
    /// Server receive time of this utterance's first retained audio frame.
    pub source_received_at: DateTime<Utc>,
    pub session_id: Uuid,
    pub device_boot_id: Uuid,
    pub generation_epoch: u64,
    pub cancel: CancellationToken,
}

#[derive(Clone)]
pub struct VoiceInputHandle {
    sender: mpsc::Sender<RecognizedVoiceInput>,
}

pub fn channel() -> (VoiceInputHandle, mpsc::Receiver<RecognizedVoiceInput>) {
    let (sender, receiver) = mpsc::channel(8);
    (VoiceInputHandle { sender }, receiver)
}

impl VoiceInputHandle {
    pub fn try_submit(
        &self,
        input: RecognizedVoiceInput,
    ) -> Result<(), mpsc::error::TrySendError<RecognizedVoiceInput>> {
        self.sender.try_send(input)
    }
}
