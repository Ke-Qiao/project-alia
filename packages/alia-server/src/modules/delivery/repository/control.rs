//! Bounded evidence for the original output owner. No method sends or replays PCM.
use std::{future::Future, time::Duration};

use alia_core::{
    delivery::VoiceScope,
    output_control::{BoundControlCommand, BoundControlReceipt, OutputBinding, OutputRegistration},
    playback_control::{ControlAction, ControlPhase, MAX_CONTROL_COMMANDS, MAX_HOLD_MS},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::*;

pub const DEFAULT_CONTROL_HOLD_MS: u64 = 60_000;
const DB_DEADLINE: Duration = Duration::from_secs(2);
const SCOPE: &str = "SELECT to_jsonb(s) AS record FROM output_control_scopes s ";
const COMMAND: &str = "SELECT to_jsonb(c) AS record FROM output_control_commands c ";

#[derive(Debug, thiserror::Error)]
pub enum ControlLedgerError {
    #[error(transparent)]
    Delivery(#[from] DeliveryError),
    #[error(transparent)]
    Database(#[from] sea_orm::DbErr),
    #[error(transparent)]
    Data(#[from] serde_json::Error),
    #[error("invalid output control: {0}")]
    Invalid(String),
    #[error("output control identity or immutable intent conflicts")]
    Conflict,
    #[error("output control authority is unavailable: {0}")]
    Revoked(String),
    #[error("output control command capacity exhausted")]
    Capacity,
    #[error("output control transaction outcome is unknown; query the original identity")]
    CommitUnknown {
        binding: Option<OutputBinding>,
        command_id: Option<Uuid>,
    },
}
pub type ControlLedgerResult<T> = Result<T, ControlLedgerError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputControlStatus {
    Active,
    Retired,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlCommandStatus {
    Requested,
    Unknown,
    Receipted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputControlRecord {
    pub owner_id: Uuid,
    pub registration: OutputRegistration,
    pub registration_digest: String,
    pub deadline_at: DateTime<Utc>,
    pub status: OutputControlStatus,
    pub retirement_reason: Option<String>,
    pub last_command_sequence: u64,
    /// An observed device fact, never permission to resume a locally retired scope.
    pub latest_receipt: Option<BoundControlReceipt>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlCommandRecord {
    pub command: BoundControlCommand,
    pub intent_digest: String,
    pub deadline_at: DateTime<Utc>,
    pub status: ControlCommandStatus,
    pub unknown_reason: Option<String>,
    /// Fixed original outcome. Its historical snapshot is not the current state.
    pub first_receipt: Option<BoundControlReceipt>,
    pub latest_receipt: Option<BoundControlReceipt>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputControlView {
    pub scope: OutputControlRecord,
    pub commands: Vec<ControlCommandRecord>,
}
#[derive(Debug)]
pub enum ControlAdmission {
    Fresh(ControlPermit),
    Existing(ControlCommandRecord),
}
/// Only a positively acknowledged first SQL commit creates this value. It cannot
/// be reconstructed by lookup, retries, deserialization or process recovery.
#[derive(Debug)]
pub struct ControlPermit {
    command: BoundControlCommand,
    deadline_at: DateTime<Utc>,
}
impl ControlPermit {
    pub fn command(&self) -> &BoundControlCommand {
        &self.command
    }
    pub fn deadline_at(&self) -> DateTime<Utc> {
        self.deadline_at
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlRecordDisposition {
    Advanced,
    LateOutcome,
    Duplicate,
    /// The old observation is no longer retained; no equality is claimed.
    Stale,
}
#[derive(Debug)]
pub struct ControlRecordResult {
    pub disposition: ControlRecordDisposition,
    pub scope: OutputControlRecord,
    pub command: ControlCommandRecord,
}

impl DeliveryRepository {
    pub async fn register_control(
        &self,
        owner: Uuid,
        registration: OutputRegistration,
    ) -> ControlLedgerResult<OutputControlRecord> {
        let binding = registration.binding;
        bounded(Some(binding), None, async {
            registration.validate().map_err(invalid)?;
            let tx = control_transaction(self).await?;
            if let Some(old) = scope_row(&tx, binding.response_id, false).await? {
                active_owner(&tx, owner).await?;
                same_registration(&old, owner, &registration)?;
                tx.commit().await.map_err(|_| unknown(Some(binding), None))?;
                return Ok(old);
            }
            let response = validate_response_source(&tx, owner, binding.response_id).await?;
            validate_response_binding(&response, &binding)?;
            let deliveries = scope_deliveries(&tx, &binding).await?;
            validate_deliveries(&deliveries, &registration)?;
            let mut deadline = response.created_at + chrono::Duration::milliseconds(MAX_HOLD_MS as i64);
            for delivery in &deliveries {
                if let Some(original) = delivery.observation_deadline_at { deadline = deadline.min(original); }
            }
            if deadline <= database_now(&tx).await? { return Err(revoked("original output lifetime expired")); }
            let digest = registration_digest(&registration)?;
            let inserted = tx.execute(stmt("INSERT INTO output_control_scopes(response_id,owner_id,device_boot_id,session_id,generation_epoch,registration,registration_digest,deadline_at) VALUES($1,$2,$3,$4,$5::text::numeric,$6,$7,$8) ON CONFLICT DO NOTHING", vec![binding.response_id.into(),owner.into(),binding.scope.device_boot_id.into(),binding.scope.session_id.into(),binding.scope.generation_epoch.to_string().into(),serde_json::to_value(&registration)?.into(),digest.into(),deadline.into()])).await?.rows_affected();
            let old = required_scope(&tx, binding.response_id, true).await?;
            same_registration(&old, owner, &registration)?;
            if inserted > 0 { audit(&tx, binding.response_id, None, "control_registered", json!({"registration":registration,"deadline_at":deadline})).await?; }
            tx.commit().await.map_err(|_| unknown(Some(binding), None))?;
            Ok(old)
        }).await
    }

    /// Duplicate admission returns evidence only, including after revocation.
    /// The caller must never send a command from Existing or a failed commit.
    pub async fn admit_control(
        &self,
        owner: Uuid,
        server_instance_id: Uuid,
        command: &BoundControlCommand,
    ) -> ControlLedgerResult<ControlAdmission> {
        let binding = command.binding;
        let command_id = command.command.command_id;
        bounded(Some(binding), Some(command_id), async {
            command.validate().map_err(invalid)?;
            if server_instance_id != binding.server_instance_id { return Err(revoked("original server instance differs")); }
            let tx = control_transaction(self).await?;
            if let Some(old) = command_row(&tx, command_id).await? {
                active_owner(&tx, owner).await?;
                let scope = required_scope(&tx, binding.response_id, false).await?;
                same_binding(&scope, owner, &binding)?;
                same_command(&old, command)?;
                tx.commit().await.map_err(|_| unknown(Some(binding), Some(command_id)))?;
                return Ok(ControlAdmission::Existing(old));
            }
            // The shared validator takes policy, active account, privacy,
            // relationship and exact Context root locks before Response.
            let response = validate_response_source(&tx, owner, binding.response_id).await?;
            validate_response_binding(&response, &binding)?;
            let scope = required_scope(&tx, binding.response_id, true).await?;
            same_binding(&scope, owner, &binding)?;
            if command.expected_backend != scope.registration.backend { return Err(ControlLedgerError::Conflict); }
            if let Some(old) = command_row(&tx, command_id).await? {
                same_command(&old, command)?;
                tx.commit().await.map_err(|_| unknown(Some(binding), Some(command_id)))?;
                return Ok(ControlAdmission::Existing(old));
            }
            let now = database_now(&tx).await?;
            if scope.status != OutputControlStatus::Active || scope.deadline_at <= now { return Err(revoked("original output retired or expired")); }
            if scope.last_command_sequence >= MAX_CONTROL_COMMANDS as u64 { return Err(ControlLedgerError::Capacity); }
            if command.command.sequence != scope.last_command_sequence + 1 { return Err(ControlLedgerError::Invalid("control sequence must be the next original sequence".into())); }
            let deliveries = scope_deliveries(&tx, &binding).await?;
            validate_deliveries(&deliveries, &scope.registration)?;
            let originals = command_rows(&tx, binding.response_id).await?;
            let deadline = match command.command.action {
                ControlAction::Pause { budget_ms, .. } => {
                    if budget_ms > scope.registration.maximum_hold_ms { return Err(ControlLedgerError::Invalid("hold exceeds original registration budget".into())); }
                    if originals.iter().any(|old| old.command.command.hold_id == command.command.hold_id) { return Err(ControlLedgerError::Conflict); }
                    scope.deadline_at.min(now + chrono::Duration::milliseconds(budget_ms as i64))
                }
                ControlAction::Resume { .. } | ControlAction::ConfirmExplicitWait { .. } => {
                    originals.iter().find(|old| old.command.command.hold_id == command.command.hold_id && matches!(old.command.command.action, ControlAction::Pause { .. }))
                        .ok_or_else(|| revoked("hold has no original Pause intent"))?.deadline_at.min(scope.deadline_at)
                }
            };
            if deadline <= now { return Err(revoked("original hold expired")); }
            tx.execute(stmt("INSERT INTO output_control_commands(command_id,response_id,sequence,hold_id,command,intent_digest,deadline_at) VALUES($1,$2,$3,$4,$5,$6,$7)",vec![command_id.into(),binding.response_id.into(),(command.command.sequence as i64).into(),command.command.hold_id.into(),serde_json::to_value(command)?.into(),command.intent_digest().into(),deadline.into()])).await?;
            tx.execute(stmt("UPDATE output_control_scopes SET last_command_sequence=$2,updated_at=clock_timestamp() WHERE response_id=$1",vec![binding.response_id.into(),(command.command.sequence as i64).into()])).await?;
            audit(&tx,binding.response_id,None,"control_requested",json!({"command":command,"intent_digest":command.intent_digest(),"deadline_at":deadline})).await?;
            tx.commit().await.map_err(|_| unknown(Some(binding), Some(command_id)))?;
            Ok(ControlAdmission::Fresh(ControlPermit { command: command.clone(), deadline_at: deadline }))
        }).await
    }

    /// Read-only evidence. No original Context refresh or new dispatch permit.
    pub async fn query_control(
        &self,
        owner: Uuid,
        binding: &OutputBinding,
    ) -> ControlLedgerResult<OutputControlView> {
        bounded(Some(*binding), None, async {
            let tx = control_transaction(self).await?;
            active_owner(&tx, owner).await?;
            // Serializes the bounded view with command and receipt writers.
            owned_response(&tx, owner, binding.response_id).await?;
            let scope = required_scope(&tx, binding.response_id, true).await?;
            same_binding(&scope, owner, binding)?;
            let commands = command_rows(&tx, binding.response_id).await?;
            tx.commit().await?;
            Ok(OutputControlView { scope, commands })
        })
        .await
    }

    /// Called by the trusted original runtime after transport verification.
    /// Device facts remain recordable after policy/owner/Context revocation.
    pub async fn record_control(
        &self,
        receipt: &BoundControlReceipt,
    ) -> ControlLedgerResult<ControlRecordResult> {
        let command = &receipt.command;
        let binding = command.binding;
        let command_id = command.command.command_id;
        bounded(Some(binding),Some(command_id),async {
            receipt.validate(command).map_err(invalid)?;
            let tx = control_transaction(self).await?;
            let (scope, old) = lock_original_command(&tx,command).await?;
            let records = command_rows(&tx,binding.response_id).await?;
            let retained = scope.latest_receipt.iter().chain(records.iter().flat_map(|r| r.first_receipt.iter().chain(r.latest_receipt.iter())));
            let mut duplicate = false;
            for retained in retained {
                if retained.observation_sequence == receipt.observation_sequence {
                    if retained != receipt { return Err(ControlLedgerError::Conflict); }
                    duplicate = true;
                }
            }
            if let Some(first) = &old.first_receipt {
                if first.receipt.outcome != receipt.receipt.outcome { return Err(ControlLedgerError::Conflict); }
            }
            let advances = scope.latest_receipt.as_ref().is_none_or(|r| receipt.observation_sequence > r.observation_sequence);
            if advances {
                if let Some(previous) = &scope.latest_receipt { validate_snapshot_advance(previous,receipt)?; }
                tx.execute(stmt("UPDATE output_control_scopes SET latest_receipt=$2,updated_at=clock_timestamp() WHERE response_id=$1",vec![binding.response_id.into(),serde_json::to_value(receipt)?.into()])).await?;
            }
            let first = old.first_receipt.is_none();
            if first || old.latest_receipt.as_ref().is_some_and(|r| receipt.observation_sequence > r.observation_sequence) {
                if let Some(previous) = &old.latest_receipt { validate_snapshot_advance(previous,receipt)?; }
                tx.execute(stmt("UPDATE output_control_commands SET status='receipted',first_receipt=COALESCE(first_receipt,$2),latest_receipt=$2,updated_at=clock_timestamp() WHERE command_id=$1",vec![command_id.into(),serde_json::to_value(receipt)?.into()])).await?;
                if first { audit(&tx,binding.response_id,None,"control_outcome",json!({"command_id":command_id,"intent_digest":old.intent_digest,"outcome":receipt.receipt.outcome,"observation_sequence":receipt.observation_sequence})).await?; }
            }
            if scope.status == OutputControlStatus::Active {
                if receipt.receipt.snapshot.phase == ControlPhase::Retired && advances {
                    retire_scope(&tx,binding.response_id,"device_retired").await?;
                } else if scope.deadline_at <= database_now(&tx).await? {
                    retire_scope(&tx,binding.response_id,"observation_deadline_expired").await?;
                }
            }
            let result = ControlRecordResult {
                disposition: if duplicate { ControlRecordDisposition::Duplicate } else if advances { ControlRecordDisposition::Advanced } else if first { ControlRecordDisposition::LateOutcome } else { ControlRecordDisposition::Stale },
                scope: required_scope(&tx,binding.response_id,false).await?,
                command: command_row(&tx,command_id).await?.ok_or(DeliveryError::NotFound)?,
            };
            tx.commit().await.map_err(|_| unknown(Some(binding),Some(command_id)))?;
            Ok(result)
        }).await
    }

    pub async fn unknown_control(
        &self,
        command: &BoundControlCommand,
        reason: &str,
    ) -> ControlLedgerResult<ControlCommandRecord> {
        let binding = command.binding;
        let id = command.command.command_id;
        bounded(Some(binding),Some(id),async {
            validate_reason(reason)?;
            let tx = control_transaction(self).await?;
            let (_,old) = lock_original_command(&tx,command).await?;
            if old.status == ControlCommandStatus::Requested {
                tx.execute(stmt("UPDATE output_control_commands SET status='unknown',unknown_reason=$2,updated_at=clock_timestamp() WHERE command_id=$1",vec![id.into(),reason.into()])).await?;
                audit(&tx,binding.response_id,None,"control_unknown",json!({"command_id":id,"reason":reason})).await?;
            }
            let record = command_row(&tx,id).await?.ok_or(DeliveryError::NotFound)?;
            tx.commit().await.map_err(|_| unknown(Some(binding),Some(id)))?;
            Ok(record)
        }).await
    }

    /// Permanent local retirement does not require the now-revoked Context.
    pub async fn retire_control(
        &self,
        owner: Uuid,
        server_instance_id: Uuid,
        binding: &OutputBinding,
        reason: &str,
    ) -> ControlLedgerResult<OutputControlRecord> {
        bounded(Some(*binding), None, async {
            validate_reason(reason)?;
            if server_instance_id != binding.server_instance_id {
                return Err(revoked("only original instance can retire this output"));
            }
            let tx = control_transaction(self).await?;
            owned_response(&tx, owner, binding.response_id).await?;
            let old = required_scope(&tx, binding.response_id, true).await?;
            same_binding(&old, owner, binding)?;
            if old.status == OutputControlStatus::Active {
                retire_scope(&tx, binding.response_id, reason).await?;
            }
            let result = required_scope(&tx, binding.response_id, false).await?;
            tx.commit()
                .await
                .map_err(|_| unknown(Some(*binding), None))?;
            Ok(result)
        })
        .await
    }

    /// Expiry is evidence of lost authority; another live instance is not.
    /// Recovery never grants a permit and never changes a binding or deadline.
    pub async fn recover_expired_controls(&self, limit: u32) -> ControlLedgerResult<usize> {
        bounded(None,None,async {
            let tx = control_transaction(self).await?;
            let ids = tx.query_all(stmt("SELECT r.response_id FROM delivery_responses r JOIN output_control_scopes s USING(response_id) WHERE s.status='active' AND s.deadline_at<=clock_timestamp() ORDER BY s.deadline_at LIMIT $1 FOR NO KEY UPDATE OF r SKIP LOCKED",vec![(limit.clamp(1,256) as i64).into()])).await?;
            let mut count = 0;
            for row in ids {
                let id: Uuid = row.try_get("","response_id")?;
                let old = required_scope(&tx,id,true).await?;
                if old.status == OutputControlStatus::Active && old.deadline_at <= database_now(&tx).await? {
                    retire_scope(&tx,id,"observation_deadline_expired").await?;
                    count += 1;
                }
            }
            tx.commit().await.map_err(|_| unknown(None,None))?;
            Ok(count)
        }).await
    }
}

async fn bounded<T>(
    binding: Option<OutputBinding>,
    command_id: Option<Uuid>,
    future: impl Future<Output = ControlLedgerResult<T>>,
) -> ControlLedgerResult<T> {
    tokio::time::timeout(DB_DEADLINE, future)
        .await
        .map_err(|_| unknown(binding, command_id))?
}
async fn control_transaction(
    repo: &DeliveryRepository,
) -> ControlLedgerResult<DatabaseTransaction> {
    let tx = repo.db.begin().await?;
    tx.execute_unprepared("SET LOCAL lock_timeout='1800ms'; SET LOCAL statement_timeout='1800ms'")
        .await?;
    Ok(tx)
}
fn unknown(binding: Option<OutputBinding>, command_id: Option<Uuid>) -> ControlLedgerError {
    ControlLedgerError::CommitUnknown {
        binding,
        command_id,
    }
}
fn invalid(error: impl std::fmt::Display) -> ControlLedgerError {
    ControlLedgerError::Invalid(error.to_string())
}
fn revoked(reason: &str) -> ControlLedgerError {
    ControlLedgerError::Revoked(reason.into())
}
fn validate_reason(reason: &str) -> ControlLedgerResult<()> {
    if reason.trim().is_empty() || reason.len() > 2048 {
        return Err(invalid("reason must contain 1..2048 bytes"));
    }
    Ok(())
}
fn registration_digest(registration: &OutputRegistration) -> ControlLedgerResult<String> {
    let mut bytes = b"alia.playback.output-registration.v1\0".to_vec();
    bytes.extend(serde_json::to_vec(registration)?);
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn same_binding(
    record: &OutputControlRecord,
    owner: Uuid,
    binding: &OutputBinding,
) -> ControlLedgerResult<()> {
    if record.owner_id != owner || record.registration.binding != *binding {
        return Err(ControlLedgerError::Conflict);
    }
    Ok(())
}
fn same_registration(
    record: &OutputControlRecord,
    owner: Uuid,
    input: &OutputRegistration,
) -> ControlLedgerResult<()> {
    same_binding(record, owner, &input.binding)?;
    if record.registration != *input || record.registration_digest != registration_digest(input)? {
        return Err(ControlLedgerError::Conflict);
    }
    Ok(())
}
fn same_command(
    old: &ControlCommandRecord,
    input: &BoundControlCommand,
) -> ControlLedgerResult<()> {
    if old.command != *input || old.intent_digest != input.intent_digest() {
        return Err(ControlLedgerError::Conflict);
    }
    Ok(())
}
fn validate_response_binding(
    response: &ResponseRecord,
    binding: &OutputBinding,
) -> ControlLedgerResult<()> {
    let original: Option<VoiceScope> = serde_json::from_value(
        response
            .content
            .get("voice_scope")
            .cloned()
            .unwrap_or(Value::Null),
    )?;
    if response.context_id != Some(binding.context_id)
        || original != Some(binding.scope)
        || response.kind == "source_notice"
        || response.cancel_requested
        || matches!(
            response.status,
            ResponseStatus::Interrupted | ResponseStatus::Discarded
        )
    {
        return Err(revoked(
            "prepared Response no longer authorizes this original control binding",
        ));
    }
    Ok(())
}
async fn active_owner(db: &impl ConnectionTrait, owner: Uuid) -> ControlLedgerResult<()> {
    if db
        .query_one(stmt(
            "SELECT id FROM users WHERE id=$1 AND status='active'",
            vec![owner.into()],
        ))
        .await?
        .is_none()
    {
        return Err(revoked("owner is not active"));
    }
    Ok(())
}
async fn database_now(db: &impl ConnectionTrait) -> ControlLedgerResult<DateTime<Utc>> {
    Ok(db
        .query_one(stmt("SELECT clock_timestamp() AS now", vec![]))
        .await?
        .unwrap()
        .try_get("", "now")?)
}
async fn scope_row(
    db: &impl ConnectionTrait,
    id: Uuid,
    lock: bool,
) -> ControlLedgerResult<Option<OutputControlRecord>> {
    Ok(row(
        db,
        &format!(
            "{SCOPE}WHERE s.response_id=$1 {}",
            if lock { "FOR UPDATE OF s" } else { "" }
        ),
        vec![id.into()],
    )
    .await?)
}
async fn required_scope(
    db: &impl ConnectionTrait,
    id: Uuid,
    lock: bool,
) -> ControlLedgerResult<OutputControlRecord> {
    scope_row(db, id, lock)
        .await?
        .ok_or_else(|| DeliveryError::NotFound.into())
}
async fn command_row(
    db: &impl ConnectionTrait,
    id: Uuid,
) -> ControlLedgerResult<Option<ControlCommandRecord>> {
    Ok(row(
        db,
        &format!("{COMMAND}WHERE c.command_id=$1"),
        vec![id.into()],
    )
    .await?)
}
async fn command_rows(
    db: &impl ConnectionTrait,
    id: Uuid,
) -> ControlLedgerResult<Vec<ControlCommandRecord>> {
    Ok(rows(
        db,
        &format!("{COMMAND}WHERE c.response_id=$1 ORDER BY c.sequence LIMIT 32"),
        vec![id.into()],
    )
    .await?)
}
async fn scope_deliveries(
    db: &impl ConnectionTrait,
    binding: &OutputBinding,
) -> ControlLedgerResult<Vec<DeliveryRecord>> {
    Ok(delivery_many(
        db,
        "WHERE d.response_id=$1 AND d.media_kind='audio' ORDER BY d.delivery_id FOR UPDATE OF d",
        vec![binding.response_id.into()],
    )
    .await?)
}
fn validate_deliveries(
    deliveries: &[DeliveryRecord],
    registration: &OutputRegistration,
) -> ControlLedgerResult<()> {
    for delivery in deliveries {
        if delivery
            .identity
            .is_none_or(|identity| identity.scope != registration.binding.scope)
            || delivery.owner_id != registration.binding.server_instance_id
            || delivery.cancel_requested
            || matches!(
                delivery.status,
                DeliveryStatus::Cancelled | DeliveryStatus::Failed | DeliveryStatus::Unknown
            )
            || delivery
                .progress
                .as_ref()
                .and_then(|p| p.backend.as_ref())
                .is_some_and(|backend| *backend != registration.backend)
        {
            return Err(revoked(
                "original audio owner, scope, backend or cancellation changed",
            ));
        }
    }
    Ok(())
}
async fn lock_original_command(
    tx: &DatabaseTransaction,
    command: &BoundControlCommand,
) -> ControlLedgerResult<(OutputControlRecord, ControlCommandRecord)> {
    // The order matches new admission without taking any revoked authority lock.
    let response = required_response(tx, command.binding.response_id).await?;
    owned_response(tx, response.key.user_id, response.response_id).await?;
    let scope = required_scope(tx, response.response_id, true).await?;
    same_binding(&scope, response.key.user_id, &command.binding)?;
    if scope.registration.backend != command.expected_backend {
        return Err(ControlLedgerError::Conflict);
    }
    let old = command_row(tx, command.command.command_id)
        .await?
        .ok_or(DeliveryError::NotFound)?;
    same_command(&old, command)?;
    Ok((scope, old))
}
async fn retire_scope(tx: &DatabaseTransaction, id: Uuid, reason: &str) -> ControlLedgerResult<()> {
    tx.execute(stmt("UPDATE output_control_scopes SET status='retired',retirement_reason=$2,updated_at=clock_timestamp() WHERE response_id=$1 AND status='active'",vec![id.into(),reason.into()])).await?;
    tx.execute(stmt("UPDATE output_control_commands SET status='unknown',unknown_reason=COALESCE(unknown_reason,$2),updated_at=clock_timestamp() WHERE response_id=$1 AND status='requested'",vec![id.into(),reason.into()])).await?;
    audit(tx, id, None, "control_retired", json!({"reason":reason})).await?;
    Ok(())
}
fn validate_snapshot_advance(
    old: &BoundControlReceipt,
    next: &BoundControlReceipt,
) -> ControlLedgerResult<()> {
    let a = &old.receipt.snapshot;
    let b = &next.receipt.snapshot;
    if b.sequence < a.sequence
        || (a.phase == ControlPhase::Retired
            && (b.phase != ControlPhase::Retired || a.retired != b.retired))
        || (a.hold_id.is_some()
            && a.hold_id == b.hold_id
            && a.remaining_ms
                .zip(b.remaining_ms)
                .is_some_and(|(a, b)| b > a))
        || (a.sequence == b.sequence
            && a.phase == ControlPhase::Held
            && b.phase == ControlPhase::Pausing)
    {
        return Err(invalid(
            "new observation regresses the original control snapshot",
        ));
    }
    Ok(())
}
