use super::*;
use crate::channels::adapter::{TextDeliveryReport, TextPayloadStream};
use alia_core::delivery::PlaybackIdentity;
use alia_core::text_delivery::{TextIdentity, TextProgress};

pub struct TextDelivery {
    pub response_id: Uuid,
    pub output_key: String,
    pub content: Value,
    pub channel: Arc<dyn ChannelAdapter>,
    pub target: ChannelTarget,
    pub text: TextPayloadStream,
    pub cancel: CancellationToken,
}

pub(super) struct DeliveryInput {
    pub response_id: Uuid,
    pub output_key: String,
    pub content: Value,
    pub channel: Arc<dyn ChannelAdapter>,
    pub target: ChannelTarget,
    pub payload: Payload,
    pub cancel: CancellationToken,
}
impl From<AudioDelivery> for DeliveryInput {
    fn from(i: AudioDelivery) -> Self {
        Self {
            response_id: i.response_id,
            output_key: i.output_key,
            content: i.content,
            channel: i.channel,
            target: i.target,
            payload: Payload::Audio(i.audio),
            cancel: i.cancel,
        }
    }
}
impl From<TextDelivery> for DeliveryInput {
    fn from(i: TextDelivery) -> Self {
        Self {
            response_id: i.response_id,
            output_key: i.output_key,
            content: i.content,
            channel: i.channel,
            target: i.target,
            payload: Payload::Text(i.text),
            cancel: i.cancel,
        }
    }
}
pub(super) enum Payload {
    Audio(AudioPayloadStream),
    Text(TextPayloadStream),
}
impl Payload {
    pub fn medium(&self) -> DeliveryMedium {
        match self {
            Self::Audio(_) => DeliveryMedium::Audio,
            Self::Text(_) => DeliveryMedium::Text,
        }
    }
    pub async fn deliver(
        self,
        channel: &dyn ChannelAdapter,
        target: &ChannelTarget,
        identity: MediaIdentity,
        observer: Arc<DurableObserver>,
        cancel: CancellationToken,
    ) -> anyhow::Result<MediaReport> {
        match (self, identity) {
            (Self::Audio(stream), MediaIdentity::Audio(identity)) => Ok(channel
                .deliver_audio(target, identity, stream, observer, cancel)
                .await?
                .into()),
            (Self::Text(stream), MediaIdentity::Text(identity)) => Ok(channel
                .deliver_text(target, identity, stream, observer, cancel)
                .await?
                .into()),
            _ => anyhow::bail!("delivery medium does not match reserved identity"),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum MediaIdentity {
    Audio(PlaybackIdentity),
    Text(TextIdentity),
}
impl MediaIdentity {
    pub async fn cancel(
        self,
        channel: &dyn ChannelAdapter,
        reason: &str,
    ) -> anyhow::Result<MediaProgress> {
        match self {
            Self::Audio(id) => channel
                .cancel_playback(id, reason)
                .await
                .map(MediaProgress::Audio),
            Self::Text(id) => channel
                .cancel_text(id, reason)
                .await
                .map(MediaProgress::Text),
        }
    }
}
pub(super) enum MediaProgress {
    Audio(PlaybackProgress),
    Text(TextProgress),
}
impl MediaProgress {
    pub fn end(&self) -> &Option<ObservationEnd> {
        match self {
            Self::Audio(p) => &p.observation_end,
            Self::Text(p) => &p.observation_end,
        }
    }
}
pub(super) struct MediaReport {
    pub progress: Option<MediaProgress>,
    pub observation_end: ObservationEnd,
}
impl From<AudioDeliveryReport> for MediaReport {
    fn from(r: AudioDeliveryReport) -> Self {
        Self {
            progress: r.progress.map(MediaProgress::Audio),
            observation_end: r.observation_end,
        }
    }
}
impl From<TextDeliveryReport> for MediaReport {
    fn from(r: TextDeliveryReport) -> Self {
        Self {
            progress: r.progress.map(MediaProgress::Text),
            observation_end: r.observation_end,
        }
    }
}
