//! Authentication and source admission shared by continuous voice sessions.

use std::sync::Arc;

use tonic::{Request, Status};

use crate::channels::adapter::ChannelAdapter;
use crate::channels::registry::ChannelRegistry;

const CHANNEL_ID_METADATA: &str = "x-alia-channel-id";

/// A capability for one already admitted source. Keep its adapter pinned so a
/// same-name channel replacement cannot lend authority to old media.
pub(crate) struct AdmittedSource {
    pub user_id: uuid::Uuid,
    pub privacy_epoch: i64,
    pub relationship_epoch: i64,
    channel: Arc<dyn ChannelAdapter>,
}

impl AdmittedSource {
    pub(crate) fn channel(&self) -> Arc<dyn ChannelAdapter> {
        self.channel.clone()
    }
}

#[derive(Clone)]
pub struct VoiceIngressAuth {
    channels: Arc<ChannelRegistry>,
}

impl VoiceIngressAuth {
    pub fn new(channels: Arc<ChannelRegistry>) -> Self {
        Self { channels }
    }

    pub(crate) async fn owner_user_id(&self, channel_id: &str) -> Result<uuid::Uuid, Status> {
        let channel = self
            .channels
            .get(channel_id)
            .await
            .ok_or_else(|| Status::failed_precondition("channel is no longer available"))?;
        verified_owner(channel.as_ref()).await
    }

    /// The successful database transaction is the logical source admission
    /// point. Call before reading media, never at dispatch of buffered content.
    pub(crate) async fn admit_source(&self, channel_id: &str) -> Result<AdmittedSource, Status> {
        let channel = self
            .channels
            .get(channel_id)
            .await
            .ok_or_else(|| Status::failed_precondition("channel is no longer available"))?;
        let user_id = verified_owner(channel.as_ref()).await?;
        let epochs = source_epochs(channel.as_ref()).await?;
        Ok(AdmittedSource {
            user_id,
            privacy_epoch: epochs.privacy_epoch,
            relationship_epoch: epochs.relationship_epoch,
            channel,
        })
    }

    /// Revalidation may retire a source, but never replaces its original stamp.
    pub(crate) async fn revalidate_source(
        &self,
        channel_id: &str,
        source: &AdmittedSource,
    ) -> Result<(), Status> {
        let channel =
            self.channels.get(channel_id).await.ok_or_else(|| {
                Status::failed_precondition("source channel is no longer available")
            })?;
        if !Arc::ptr_eq(&channel, &source.channel) {
            return Err(Status::permission_denied("source channel was replaced"));
        }
        if verified_owner(channel.as_ref()).await? != source.user_id {
            return Err(Status::permission_denied("source account changed"));
        }
        let current = source_epochs(channel.as_ref()).await?;
        if current.privacy_epoch != source.privacy_epoch {
            return Err(Status::permission_denied(
                "source privacy epoch changed; open a new source",
            ));
        }
        if current.relationship_epoch != source.relationship_epoch {
            return Err(Status::permission_denied(
                "source relationship epoch changed; open a new source",
            ));
        }
        Ok(())
    }

    async fn resolve_channel_id(&self, requested: Option<String>) -> Result<String, Status> {
        let requested = requested
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Status::invalid_argument("an explicit channel id is required"))?;

        self.channels
            .get(requested)
            .await
            .map(|channel| channel.channel_id().to_owned())
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

async fn verified_owner(channel: &dyn ChannelAdapter) -> Result<uuid::Uuid, Status> {
    let configured = channel
        .owner_user_id()
        .filter(|id| !id.is_nil())
        .ok_or_else(|| {
            Status::failed_precondition(
                "identity_unlinked: configure this channel's owner_user_id before voice input",
            )
        })?;
    let verified = channel.authenticated_owner().await.map_err(|error| {
            tracing::warn!(channel_id = channel.channel_id(), %error, "channel owner verification failed");
            Status::permission_denied("channel owner is inactive or cannot be verified")
        })?;
    if verified != configured {
        return Err(Status::permission_denied(
            "channel owner verification mismatch",
        ));
    }
    Ok(verified)
}

async fn source_epochs(
    channel: &dyn ChannelAdapter,
) -> Result<crate::modules::memory::source::SourceEpochs, Status> {
    let epoch = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        channel.authenticated_source_epochs(),
    )
    .await
    .map_err(|_| Status::unavailable("source admission deadline"))?
    .map_err(|error| {
        tracing::warn!(channel_id = channel.channel_id(), %error, "source admission failed");
        Status::unavailable("source admission failed")
    })?;
    if epoch.privacy_epoch < 0 {
        return Err(Status::failed_precondition("invalid source privacy epoch"));
    }
    if epoch.relationship_epoch < 0 {
        return Err(Status::failed_precondition(
            "invalid source relationship epoch",
        ));
    }
    Ok(epoch)
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

#[cfg(test)]
mod tests;
