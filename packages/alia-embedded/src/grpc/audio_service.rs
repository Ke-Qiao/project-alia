use std::sync::Arc;

use tokio::sync::Mutex;
use tonic::{Request, Response, Status};

use alia_proto::audio::audio_stream_server::AudioStream as AudioStreamTrait;
use alia_proto::audio::{
    AudioChunk, AudioEncoding, AudioFrame, AudioUploadResponse, PlayAudioRequest,
};

use super::auth::{extract_bearer_token, require_auth, require_auth_token};
use crate::binding::BindingManager;
use crate::module::context::EventBus;
use crate::module::context::ModuleEvent;
use crate::module::traits::AudioPlaybackBackend;

#[derive(Debug, Clone)]
struct PendingPlayback {
    request_id: String,
    source: String,
    encoding: i32,
    sample_rate: u32,
}

pub struct AudioStreamService {
    playback: Arc<dyn AudioPlaybackBackend>,
    binding: Arc<BindingManager>,
    event_bus: EventBus,
    pending_playback: Arc<Mutex<Option<PendingPlayback>>>,
}

impl AudioStreamService {
    pub fn new(
        playback: Arc<dyn AudioPlaybackBackend>,
        binding: Arc<BindingManager>,
        event_bus: EventBus,
    ) -> Self {
        Self {
            playback,
            binding,
            event_bus,
            pending_playback: Arc::new(Mutex::new(None)),
        }
    }

    fn playback_chunk(&self, chunk: &AudioChunk, sample_rate: u32) -> Result<(), Status> {
        match AudioEncoding::try_from(chunk.encoding).unwrap_or(AudioEncoding::Opus) {
            AudioEncoding::PcmS16le => {
                let pcm = pcm16_from_le_bytes(&chunk.opus_data)?;
                self.playback
                    .play(&pcm, sample_rate)
                    .map_err(|error| Status::internal(error.to_string()))?;
                Ok(())
            }
            AudioEncoding::Opus => Err(Status::unimplemented(
                "opus playback not implemented on alia-embedded yet",
            )),
        }
    }
}

#[tonic::async_trait]
impl AudioStreamTrait for AudioStreamService {
    async fn upload_audio(
        &self,
        request: Request<tonic::Streaming<AudioChunk>>,
    ) -> Result<Response<AudioUploadResponse>, Status> {
        let token = extract_bearer_token(request.metadata())?;
        require_auth_token(&self.binding, &token).await?;
        let mut stream = request.into_inner();
        let pending = self.pending_playback.lock().await.clone();

        if let Some(pending) = &pending {
            self.event_bus.publish(ModuleEvent::PlaybackStarted {
                source: pending.source.clone(),
            });
        }

        while let Some(chunk) = stream.message().await? {
            if let Some(pending) = &pending {
                self.playback_chunk(&chunk, pending.sample_rate)?;
            }
        }

        if let Some(pending) = pending {
            self.event_bus.publish(ModuleEvent::PlaybackFinished {
                source: pending.source,
            });
            let mut slot = self.pending_playback.lock().await;
            if slot
                .as_ref()
                .map(|current| current.request_id == pending.request_id)
                .unwrap_or(false)
            {
                *slot = None;
            }
        }

        Ok(Response::new(AudioUploadResponse { accepted: true }))
    }

    type PlayAudioStream = tokio_stream::wrappers::ReceiverStream<Result<AudioChunk, Status>>;

    async fn play_audio(
        &self,
        request: Request<PlayAudioRequest>,
    ) -> Result<Response<Self::PlayAudioStream>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();

        {
            let mut pending = self.pending_playback.lock().await;
            *pending = Some(PendingPlayback {
                request_id: req.request_id.clone(),
                source: req.source.clone(),
                encoding: req.encoding,
                sample_rate: req.sample_rate,
            });
        }

        if req.interrupt_current {
            self.playback
                .stop_playback()
                .map_err(|error| Status::internal(error.to_string()))?;
        }

        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let ack = AudioChunk {
            opus_data: Vec::new(),
            sequence: 0,
            timestamp_us: timestamp_us(),
            encoding: req.encoding,
        };
        let _ = tx.send(Ok(ack)).await;

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
    }

    type BiDirectionalAudioStream =
        tokio_stream::wrappers::ReceiverStream<Result<AudioFrame, Status>>;

    async fn bi_directional_audio(
        &self,
        request: Request<tonic::Streaming<AudioFrame>>,
    ) -> Result<Response<Self::BiDirectionalAudioStream>, Status> {
        let token = extract_bearer_token(request.metadata())?;
        require_auth_token(&self.binding, &token).await?;
        let mut stream = request.into_inner();
        let playback = Arc::clone(&self.playback);
        let event_bus = self.event_bus.clone();

        tokio::spawn(async move {
            while let Ok(Some(frame)) = stream.message().await {
                if let Some(payload) = frame.payload {
                    match payload {
                        alia_proto::audio::audio_frame::Payload::Audio(chunk) => {
                            if let Ok(AudioEncoding::PcmS16le) =
                                AudioEncoding::try_from(chunk.encoding)
                            {
                                if let Ok(pcm) = pcm16_from_le_bytes(&chunk.opus_data) {
                                    let _ = playback.play(&pcm, 24_000);
                                }
                            }
                        }
                        alia_proto::audio::audio_frame::Payload::Control(control) => {
                            if control.r#type == 2 || control.r#type == 3 {
                                let _ = playback.stop_playback();
                                event_bus.publish(ModuleEvent::PlaybackFinished {
                                    source: "bidi".to_string(),
                                });
                            }
                        }
                    }
                }
            }
        });

        let (_tx, rx) = tokio::sync::mpsc::channel(4);
        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
    }
}

fn pcm16_from_le_bytes(bytes: &[u8]) -> Result<Vec<i16>, Status> {
    if bytes.len() % 2 != 0 {
        return Err(Status::invalid_argument(
            "pcm payload length must be even for s16le",
        ));
    }

    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
}

fn timestamp_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
