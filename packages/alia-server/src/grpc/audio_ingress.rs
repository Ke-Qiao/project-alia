use std::sync::Arc;

use alia_proto::audio::server_audio_ingress_server::ServerAudioIngress;
use alia_proto::audio::{
    AudioEncoding as ProtoAudioEncoding, AudioUploadResponse, CapturedAudioChunk,
    InterruptActiveTurnRequest, SensorDataRequest, SensorDataResponse, TurnInterruptResponse,
};
use tonic::{Request, Response, Status};

use crate::channels::adapter::{AudioFormat, AudioPayload, ChannelEvent};
use crate::channels::registry::ChannelRegistry;
use crate::event::{EventBus, ServerEvent};

const CHANNEL_ID_METADATA: &str = "x-alia-channel-id";

#[derive(Clone)]
pub struct ServerAudioIngressService {
    event_bus: EventBus,
    channels: Arc<ChannelRegistry>,
    default_channel_id: String,
}

impl ServerAudioIngressService {
    pub fn new(
        event_bus: EventBus,
        channels: Arc<ChannelRegistry>,
        default_channel_id: impl Into<String>,
    ) -> Self {
        Self {
            event_bus,
            channels,
            default_channel_id: default_channel_id.into(),
        }
    }

    pub(crate) async fn owner_user_id(&self, channel_id: &str) -> Result<uuid::Uuid, Status> {
        let channel = self
            .channels
            .get(channel_id)
            .await
            .ok_or_else(|| Status::failed_precondition("channel is no longer available"))?;
        let configured = channel
            .owner_user_id()
            .filter(|id| !id.is_nil())
            .ok_or_else(|| {
                Status::failed_precondition(
                    "identity_unlinked: configure this channel's owner_user_id before voice input",
                )
            })?;
        let verified = channel.authenticated_owner().await.map_err(|error| {
            tracing::warn!(%channel_id, %error, "channel owner verification failed");
            Status::permission_denied("channel owner is inactive or cannot be verified")
        })?;
        if verified != configured {
            return Err(Status::permission_denied(
                "channel owner verification mismatch",
            ));
        }
        Ok(verified)
    }

    async fn resolve_channel_id(&self, requested: Option<String>) -> Result<String, Status> {
        let requested = requested
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&self.default_channel_id);

        self.channels
            .resolve_id_or_type(requested)
            .await
            .map_err(|error| Status::failed_precondition(error.to_string()))?
            .ok_or_else(|| Status::invalid_argument(format!("channel '{requested}' not found")))
    }

    pub(crate) async fn authenticate_ingress<T>(
        &self,
        request: &Request<T>,
        requested_channel_id: Option<&str>,
    ) -> Result<String, Status> {
        let metadata_channel_id = request
            .metadata()
            .get(CHANNEL_ID_METADATA)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let token = bearer_token(request)?;
        self.authenticate_ingress_token(
            token,
            requested_channel_id.map(str::to_string),
            metadata_channel_id,
        )
        .await
    }

    async fn authenticate_ingress_token(
        &self,
        token: String,
        requested_channel_id: Option<String>,
        metadata_channel_id: Option<String>,
    ) -> Result<String, Status> {
        let requested = requested_channel_id.or(metadata_channel_id);
        let channel_id = self.resolve_channel_id(requested).await?;

        let channel =
            self.channels.get(&channel_id).await.ok_or_else(|| {
                Status::invalid_argument(format!("channel '{channel_id}' not found"))
            })?;
        if channel.channel_type() != "embedded" {
            return Err(Status::permission_denied(format!(
                "channel '{channel_id}' does not accept embedded ingress"
            )));
        }

        channel
            .execute_action(crate::channels::adapter::ChannelAction {
                name: "verify_audio_ingress_auth".to_string(),
                params: serde_json::json!({ "token": token }),
            })
            .await
            .map_err(|error| Status::unauthenticated(error.to_string()))?;

        Ok(channel_id)
    }
}

#[tonic::async_trait]
impl ServerAudioIngress for ServerAudioIngressService {
    async fn upload_captured_audio(
        &self,
        request: Request<tonic::Streaming<CapturedAudioChunk>>,
    ) -> Result<Response<AudioUploadResponse>, Status> {
        // Capture before identity/database waits and retain through the upload.
        let source_received_at = chrono::Utc::now();
        let metadata_channel_id = request
            .metadata()
            .get(CHANNEL_ID_METADATA)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let token = bearer_token(&request)?;
        let authenticated_channel_id = self
            .authenticate_ingress_token(token.clone(), None, metadata_channel_id)
            .await?;
        let owner_user_id = self.owner_user_id(&authenticated_channel_id).await?;
        let mut stream = request.into_inner();
        let mut audio = alia_core::voice::AudioWindowBuffer::default();
        let upload_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(40);
        while let Some(frame) = tokio::time::timeout_at(upload_deadline, stream.message())
            .await
            .map_err(|_| Status::deadline_exceeded("captured utterance upload deadline"))??
        {
            if !frame.channel_id.is_empty() {
                let frame_channel_id = self.resolve_channel_id(Some(frame.channel_id)).await?;
                if frame_channel_id != authenticated_channel_id {
                    return Err(Status::permission_denied(
                        "captured audio channel_id does not match authenticated channel",
                    ));
                }
            }
            // Legacy user_id is an untrusted hint. The authenticated channel
            // selects the account; no frame may select another user's authority.
            if frame.sample_rate != 16_000 || frame.channels != 1 {
                return Err(Status::invalid_argument(
                    "captured speech requires mono 16000 Hz PCM16",
                ));
            }
            let chunk = frame
                .chunk
                .ok_or_else(|| Status::invalid_argument("missing captured audio chunk"))?;
            if chunk.encoding != ProtoAudioEncoding::PcmS16le as i32 {
                return Err(Status::invalid_argument(
                    "captured speech requires pcm_s16le",
                ));
            }
            audio
                .push(chunk.sequence, &chunk.opus_data)
                .map_err(|error| Status::invalid_argument(error.to_string()))?;
        }
        let audio = audio
            .into_window()
            .map_err(|error| Status::invalid_argument(error.to_string()))?;

        // Uploads may last 40 seconds. Recheck binding before publishing content.
        self.authenticate_ingress_token(token, Some(authenticated_channel_id.clone()), None)
            .await?;
        if self.owner_user_id(&authenticated_channel_id).await? != owner_user_id {
            return Err(Status::permission_denied(
                "channel owner changed during upload",
            ));
        }

        self.event_bus
            .publish(ServerEvent::ChannelInput(ChannelEvent::AudioInput {
                channel_id: authenticated_channel_id,
                user_id: owner_user_id.to_string(),
                source_received_at,
                audio: AudioPayload {
                    data: audio.as_bytes().to_vec(),
                    sample_rate: 16_000,
                    channels: 1,
                    format: AudioFormat::Pcm16,
                },
            }));

        Ok(Response::new(AudioUploadResponse { accepted: true }))
    }

    async fn interrupt_active_turn(
        &self,
        request: Request<InterruptActiveTurnRequest>,
    ) -> Result<Response<TurnInterruptResponse>, Status> {
        let channel_id = self
            .authenticate_ingress(&request, Some(&request.get_ref().channel_id))
            .await?;
        let user_id = self.owner_user_id(&channel_id).await?.to_string();
        let req = request.into_inner();
        let reason = if req.reason.trim().is_empty() {
            "embedded_interrupt".to_string()
        } else {
            req.reason
        };

        self.event_bus.publish(ServerEvent::ChannelInput(
            ChannelEvent::InterruptActiveTurn {
                channel_id,
                user_id,
                reason,
            },
        ));

        Ok(Response::new(TurnInterruptResponse {
            accepted: true,
            interrupted: true,
        }))
    }

    async fn report_sensor_data(
        &self,
        request: Request<SensorDataRequest>,
    ) -> Result<Response<SensorDataResponse>, Status> {
        let channel_id = self
            .authenticate_ingress(&request, Some(&request.get_ref().channel_id))
            .await?;
        let req = request.into_inner();
        let sensor_type = req.sensor_type.trim();
        if sensor_type.is_empty() {
            return Err(Status::invalid_argument("sensor_type is required"));
        }
        let payload = if req.value_json.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&req.value_json)
                .map_err(|error| Status::invalid_argument(format!("invalid value_json: {error}")))?
        };
        let sensor_id = format!("{channel_id}:{sensor_type}");

        self.event_bus
            .publish(ServerEvent::ChannelInput(ChannelEvent::SensorData {
                channel_id,
                sensor_type: sensor_type.to_string(),
                payload,
            }));

        Ok(Response::new(SensorDataResponse {
            accepted: true,
            sensor_id,
        }))
    }
}

fn bearer_token<T>(request: &Request<T>) -> Result<String, Status> {
    let raw = request
        .metadata()
        .get("authorization")
        .ok_or_else(|| Status::unauthenticated("missing authorization metadata"))?;
    let raw = raw
        .to_str()
        .map_err(|_| Status::unauthenticated("invalid authorization metadata"))?;
    raw.strip_prefix("Bearer ")
        .map(str::to_string)
        .ok_or_else(|| Status::unauthenticated("expected Bearer token"))
}
