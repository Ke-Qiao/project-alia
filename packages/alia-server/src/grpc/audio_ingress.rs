use alia_proto::audio::server_audio_ingress_server::ServerAudioIngress;
use alia_proto::audio::{
    AudioEncoding as ProtoAudioEncoding, AudioUploadResponse, CapturedAudioChunk,
};
use tonic::{Request, Response, Status};

use crate::channels::adapter::{AudioFormat, AudioPayload, ChannelEvent};
use crate::event::{EventBus, ServerEvent};

pub struct ServerAudioIngressService {
    event_bus: EventBus,
    default_channel_id: String,
    default_user_id: String,
}

impl ServerAudioIngressService {
    pub fn new(event_bus: EventBus, default_channel_id: impl Into<String>) -> Self {
        Self {
            event_bus,
            default_channel_id: default_channel_id.into(),
            default_user_id: "embedded-user".to_string(),
        }
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

        let format = match encoding {
            ProtoAudioEncoding::PcmS16le => AudioFormat::Pcm16,
            ProtoAudioEncoding::Opus => AudioFormat::Opus,
        };

        self.event_bus
            .publish(ServerEvent::ChannelInput(ChannelEvent::AudioInput {
                channel_id: channel_id.unwrap_or_else(|| self.default_channel_id.clone()),
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
}
