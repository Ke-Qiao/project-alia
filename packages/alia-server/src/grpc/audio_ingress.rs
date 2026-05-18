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

pub struct ServerAudioIngressService {
    event_bus: EventBus,
    channels: Arc<ChannelRegistry>,
    default_channel_id: String,
    default_user_id: String,
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
            default_user_id: "embedded-user".to_string(),
        }
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
}

#[tonic::async_trait]
impl ServerAudioIngress for ServerAudioIngressService {
    async fn upload_captured_audio(
        &self,
        request: Request<tonic::Streaming<CapturedAudioChunk>>,
    ) -> Result<Response<AudioUploadResponse>, Status> {
        let mut stream = request.into_inner();
        let mut audio = Vec::new();
        let mut channel_id = None;
        let mut user_id = None;
        let mut sample_rate = 16_000;
        let mut channels = 1;
        let mut encoding = ProtoAudioEncoding::PcmS16le;

        while let Some(frame) = stream.message().await? {
            if !frame.channel_id.is_empty() {
                channel_id = Some(frame.channel_id);
            }
            if !frame.user_id.is_empty() {
                user_id = Some(frame.user_id);
            }
            if frame.sample_rate > 0 {
                sample_rate = frame.sample_rate;
            }
            if frame.channels > 0 {
                channels = frame.channels;
            }
            if let Some(chunk) = frame.chunk {
                encoding = ProtoAudioEncoding::try_from(chunk.encoding)
                    .unwrap_or(ProtoAudioEncoding::PcmS16le);
                audio.extend_from_slice(&chunk.opus_data);
            }
        }

        if audio.is_empty() {
            return Err(Status::invalid_argument("captured audio stream was empty"));
        }
        let channel_id = self.resolve_channel_id(channel_id).await?;

        let format = match encoding {
            ProtoAudioEncoding::PcmS16le => AudioFormat::Pcm16,
            ProtoAudioEncoding::Opus => AudioFormat::Opus,
        };

        self.event_bus
            .publish(ServerEvent::ChannelInput(ChannelEvent::AudioInput {
                channel_id,
                user_id: user_id.unwrap_or_else(|| self.default_user_id.clone()),
                audio: AudioPayload {
                    data: audio,
                    sample_rate,
                    channels: channels as u16,
                    format,
                },
            }));

        Ok(Response::new(AudioUploadResponse { accepted: true }))
    }

    async fn interrupt_active_turn(
        &self,
        request: Request<InterruptActiveTurnRequest>,
    ) -> Result<Response<TurnInterruptResponse>, Status> {
        let req = request.into_inner();
        let channel_id = self.resolve_channel_id(Some(req.channel_id)).await?;
        let user_id = if req.user_id.trim().is_empty() {
            self.default_user_id.clone()
        } else {
            req.user_id
        };
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
        let req = request.into_inner();
        let channel_id = self.resolve_channel_id(Some(req.channel_id)).await?;
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
