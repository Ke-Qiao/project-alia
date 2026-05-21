use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tonic::{Request, Response, Status};

use alia_proto::audio::audio_stream_server::AudioStream as AudioStreamTrait;
use alia_proto::audio::{
    AudioChunk, AudioEncoding, AudioFrame, AudioUploadResponse, PlayAudioRequest,
    PlayDirectResponseRequest, PlayDirectResponseResponse, PlaybackControlResponse,
    StopPlaybackRequest,
};

use super::auth::{extract_bearer_token, require_auth, require_auth_token};
use crate::binding::BindingManager;
use crate::module::audio_debug::AudioDebugRecorder;
use crate::module::context::EventBus;
use crate::module::context::ModuleEvent;
use crate::module::direct_response_playback::play_direct_response;
use crate::module::traits::{AudioPlaybackBackend, DirectResponseEngine};

#[derive(Debug, Clone)]
struct PendingPlayback {
    request_id: String,
    source: String,
    encoding: i32,
    sample_rate: u32,
}

pub struct AudioStreamService {
    playback: Arc<dyn AudioPlaybackBackend>,
    direct_response: Arc<dyn DirectResponseEngine>,
    binding: Arc<BindingManager>,
    event_bus: EventBus,
    audio_debug: AudioDebugRecorder,
    pending_playback: Arc<Mutex<Option<PendingPlayback>>>,
}

impl AudioStreamService {
    pub fn new(
        playback: Arc<dyn AudioPlaybackBackend>,
        direct_response: Arc<dyn DirectResponseEngine>,
        binding: Arc<BindingManager>,
        event_bus: EventBus,
        audio_debug: AudioDebugRecorder,
    ) -> Self {
        Self {
            playback,
            direct_response,
            binding,
            event_bus,
            audio_debug,
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
        let initial = self.pending_playback.lock().await.clone();
        let initial_request_id = initial.as_ref().map(|pending| pending.request_id.clone());
        let playback_started_at = Instant::now();
        let mut uploaded_pcm_samples = 0usize;
        let mut debug_playback_pcm = self.audio_debug.record_playback_enabled().then(Vec::new);

        if let Some(pending) = &initial {
            self.event_bus.publish(ModuleEvent::PlaybackStarted {
                source: pending.source.clone(),
            });
        }

        while let Some(chunk) = stream.message().await? {
            let current = self.pending_playback.lock().await.clone();
            let Some(current) = current else {
                break;
            };
            if initial_request_id
                .as_ref()
                .map(|request_id| request_id != &current.request_id)
                .unwrap_or(false)
            {
                break;
            }
            if matches!(
                AudioEncoding::try_from(chunk.encoding).unwrap_or(AudioEncoding::Opus),
                AudioEncoding::PcmS16le
            ) {
                uploaded_pcm_samples =
                    uploaded_pcm_samples.saturating_add(chunk.opus_data.len() / 2);
                if let Some(debug_pcm) = debug_playback_pcm.as_mut()
                    && let Ok(pcm) = pcm16_from_le_bytes(&chunk.opus_data)
                {
                    debug_pcm.extend_from_slice(&pcm);
                }
            }
            self.playback_chunk(&chunk, current.sample_rate)?;
        }

        if let Some(pending) = initial {
            if let Some(debug_pcm) = debug_playback_pcm.take() {
                self.audio_debug.record_playback(
                    &pending.source,
                    &debug_pcm,
                    pending.sample_rate,
                    1,
                );
            }
            if self.pending_matches(&pending).await {
                self.wait_for_estimated_playback_drain(
                    &pending,
                    uploaded_pcm_samples,
                    pending.sample_rate,
                    playback_started_at,
                )
                .await;
            }
            let mut slot = self.pending_playback.lock().await;
            if slot
                .as_ref()
                .map(|current| current.request_id == pending.request_id)
                .unwrap_or(false)
            {
                *slot = None;
                self.event_bus.publish(ModuleEvent::PlaybackFinished {
                    source: pending.source,
                });
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

    async fn stop_playback(
        &self,
        request: Request<StopPlaybackRequest>,
    ) -> Result<Response<PlaybackControlResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        let reason = req.reason.clone();
        let pending = {
            let mut slot = self.pending_playback.lock().await;
            slot.take()
        };

        self.playback
            .stop_playback()
            .map_err(|error| Status::internal(error.to_string()))?;

        if let Some(pending) = pending {
            self.event_bus.publish(ModuleEvent::PlaybackFinished {
                source: pending.source,
            });
        }
        if is_continuous_dialogue_end_reason(&reason) {
            self.event_bus
                .publish(ModuleEvent::ContinuousDialogueEndRequested {
                    reason: reason
                        .split_once(':')
                        .map(|(_, detail)| detail.trim())
                        .filter(|detail| !detail.is_empty())
                        .unwrap_or("server requested continuous dialogue end")
                        .to_string(),
                });
        }
        tracing::debug!(
            request_id = %req.request_id,
            reason = %req.reason,
            "playback stopped by control RPC"
        );

        Ok(Response::new(PlaybackControlResponse { accepted: true }))
    }

    async fn play_direct_response(
        &self,
        request: Request<PlayDirectResponseRequest>,
    ) -> Result<Response<PlayDirectResponseResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        let action = req.action.trim();
        if action.is_empty() {
            return Err(Status::invalid_argument(
                "direct response action must not be empty",
            ));
        }

        if req.interrupt_current {
            let pending = {
                let mut slot = self.pending_playback.lock().await;
                slot.take()
            };
            self.playback
                .stop_playback()
                .map_err(|error| Status::internal(error.to_string()))?;
            if let Some(pending) = pending {
                self.event_bus.publish(ModuleEvent::PlaybackFinished {
                    source: pending.source,
                });
            }
        }

        let Some(response) = self.direct_response.match_action(action) else {
            tracing::warn!(
                request_id = %req.request_id,
                action,
                reason = %req.reason,
                "direct response action is not configured"
            );
            return Ok(Response::new(PlayDirectResponseResponse {
                accepted: true,
                played: false,
                message: format!("direct response action '{action}' is not configured"),
            }));
        };

        let played = play_direct_response(
            response,
            self.playback.clone(),
            self.event_bus.clone(),
            self.audio_debug.clone(),
        )
        .await;
        Ok(Response::new(PlayDirectResponseResponse {
            accepted: true,
            played,
            message: if played {
                "direct response played".to_string()
            } else {
                format!("direct response action '{action}' has no playable audio")
            },
        }))
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

fn is_continuous_dialogue_end_reason(reason: &str) -> bool {
    let reason = reason.trim();
    reason == "end_continuous_dialogue" || reason.starts_with("end_continuous_dialogue:")
}

impl AudioStreamService {
    async fn pending_matches(&self, pending: &PendingPlayback) -> bool {
        self.pending_playback
            .lock()
            .await
            .as_ref()
            .map(|current| current.request_id == pending.request_id)
            .unwrap_or(false)
    }

    async fn wait_for_estimated_playback_drain(
        &self,
        pending: &PendingPlayback,
        pcm_samples: usize,
        sample_rate: u32,
        playback_started_at: Instant,
    ) {
        if pcm_samples == 0 || sample_rate == 0 {
            return;
        }

        let estimated = Duration::from_secs_f64(pcm_samples as f64 / sample_rate as f64);
        let elapsed = playback_started_at.elapsed();
        if estimated <= elapsed {
            return;
        }

        let deadline = Instant::now() + (estimated - elapsed);
        loop {
            if !self.pending_matches(pending).await {
                return;
            }
            let now = Instant::now();
            if now >= deadline {
                return;
            }
            tokio::time::sleep((deadline - now).min(Duration::from_millis(100))).await;
        }
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
