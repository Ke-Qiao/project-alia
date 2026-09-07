//! Bounded application entrance for recognized speech. This is intentionally
//! separate from the lossy broadcast bus used for observations and UI updates.

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug)]
pub struct RecognizedVoiceInput {
    pub channel_id: String,
    pub user_id: String,
    pub transcript: String,
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
