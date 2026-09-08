use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

/// Channels select an owner from trusted configuration, then recheck the
/// current account before accepting conversation input. No account is created.
pub async fn require_active_owner(
    db: Option<&DatabaseConnection>,
    owner: Option<Uuid>,
) -> anyhow::Result<Uuid> {
    let owner = owner
        .filter(|id| !id.is_nil())
        .ok_or_else(|| anyhow::anyhow!("identity_unlinked: channel owner is not configured"))?;
    let db = db.ok_or_else(|| anyhow::anyhow!("account directory is not initialized"))?;
    let user = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        crate::entity::user::Entity::find_by_id(owner).one(db),
    )
    .await??;
    anyhow::ensure!(
        user.is_some_and(|user| user.status == "active"),
        "channel owner account is inactive or missing"
    );
    Ok(owner)
}
