use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use uuid::Uuid;

use crate::entity::{user, user_identity_link};

pub const EMAIL_IDENTITY_PROVIDER: &str = "email";
pub const VOICEPRINT_IDENTITY_PROVIDER: &str = "voiceprint";

#[derive(Debug, Clone)]
pub struct IdentityAssertion {
    pub provider: String,
    pub subject: String,
    pub confidence: Option<f32>,
    pub metadata: Value,
}

impl IdentityAssertion {
    pub fn voiceprint(
        subject: impl Into<String>,
        confidence: Option<f32>,
        metadata: Value,
    ) -> Self {
        Self {
            provider: VOICEPRINT_IDENTITY_PROVIDER.to_string(),
            subject: subject.into(),
            confidence,
            metadata,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedIdentity {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
}

/// Resolve an external identity assertion to a canonical Alia user.
///
/// Future voiceprint/speaker-recognition modules should call this after they
/// produce a provider-specific subject (for example an enrolled speaker id).
/// The LLM pipeline should only receive the returned `user_id`, keeping memory,
/// cache, persona, and conversation history scoped to the canonical user.
pub async fn resolve_identity_assertion(
    db: &DatabaseConnection,
    assertion: &IdentityAssertion,
) -> anyhow::Result<Option<ResolvedIdentity>> {
    let link = user_identity_link::Entity::find()
        .filter(user_identity_link::Column::Provider.eq(assertion.provider.trim()))
        .filter(user_identity_link::Column::Subject.eq(assertion.subject.trim()))
        .one(db)
        .await?;

    let Some(link) = link else {
        return Ok(None);
    };

    if let (Some(confidence), Some(threshold)) = (assertion.confidence, link.confidence_threshold) {
        if confidence < threshold {
            return Ok(None);
        }
    }

    let Some(user) = user::Entity::find_by_id(link.user_id).one(db).await? else {
        return Ok(None);
    };

    if user.status != "active" {
        return Ok(None);
    }

    Ok(Some(ResolvedIdentity {
        user_id: user.id,
        email: user.email,
        name: user.name,
        role: user.role,
    }))
}
