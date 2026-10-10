from pathlib import Path
base = Path('packages/alia-server/src/modules')
old_path = base/'memory/context/companion.rs'
old = old_path.read_text()
inv_path = base/'memory/context/companion/invitation.rs'
inv = inv_path.read_text()

# Canonical Context proof stays encapsulated; only the domain projection leaves.
authority = old[old.index('pub(crate) struct CompanionAuthority'):old.index('pub(crate) struct CompanionSource')]
authority = authority.replace('CompanionAuthority', 'AppraisalReadAuthority').replace('lock_companion_authority', 'lock_appraisal_authority')
load = old[old.index('pub(crate) async fn load_companion_source'):old.index('    let mut roots =')]
load = load.replace('load_companion_source', 'load_user_appraisal_source').replace('CompanionAuthority', 'AppraisalReadAuthority').replace('CompanionSource', 'UserAppraisalSource')
load = load.replace('Companion source is unavailable', 'appraisal source is unavailable').replace('the first Companion intention requires a canonical user-message appraisal', 'source requires a canonical user-message appraisal').replace('Companion user source was re-timed', 'appraisal user source was re-timed')
load = load.replace('    let reference = AppraisalReference {\n        appraisal_id,\n        revision: row.try_get("", "revision")?,\n        result_digest: row.try_get("", "result_digest")?,\n    };', '    let revision = row.try_get("", "revision")?;\n    let result_digest: String = row.try_get("", "result_digest")?;')
load = load.replace('revision: reference.revision,', 'revision,').replace('result_digest: reference.result_digest.clone(),', 'result_digest: result_digest.clone(),')
load = load.replace('    let source_event_id = source.source_event_id.ok_or_else(revoked)?;\n    let conversation_id = source.conversation_id.ok_or_else(revoked)?;\n', '    source.source_event_id.ok_or_else(revoked)?;\n    source.conversation_id.ok_or_else(revoked)?;\n')
header = '''//! Canonical user-appraisal evidence read under the Context authority locks.
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction};
use uuid::Uuid;

use super::{repository::*, *};

'''
source = header + authority + '''pub(crate) struct UserAppraisalSource {
    pub lease: ContextLease,
    pub appraisal_id: Uuid,
    pub revision: i64,
    pub result_digest: String,
    pub effective_at: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
}

''' + load + '''    Ok(UserAppraisalSource {
        lease,
        appraisal_id,
        revision,
        result_digest,
        effective_at,
        valid_until: until,
    })
}
'''
(base/'memory/context/appraisal_source.rs').write_text(source)

roots = old[old.index('    let mut roots ='):old.index('    match fact {')]
roots = roots.replace('&lease.dependencies', 'lease.dependencies()')
finish = old[old.index('    roots.sort_unstable();'):]
finish = finish.replace('context_id: lease.id,', 'context_id: lease.id(),').replace('            effective_at,', '            effective_at: appraisal.effective_at,').replace('            valid_until: until,', '            valid_until: appraisal.valid_until,')
comp_source = '''//! Companion observations and erasure references projected from canonical evidence.
use sea_orm::DatabaseTransaction;
use uuid::Uuid;

use super::types::{AppraisalReference, CanonicalObservation};
use crate::modules::memory::context::{
    ContextError, ContextEvidence, ContextLease, ContextResult,
    appraisal_source::{AppraisalReadAuthority, load_user_appraisal_source},
};

pub(crate) struct CompanionSource {
    pub observation: CanonicalObservation,
    /// Original parent identity with every flattened canonical dependency.
    pub lease: ContextLease,
    /// Opaque references for transactional erasure, never copied source text.
    pub erasure_roots: Vec<(&'static str, Uuid)>,
}

pub(crate) async fn load_companion_source(
    tx: &DatabaseTransaction,
    owner: Uuid,
    appraisal_id: Uuid,
    authority: &AppraisalReadAuthority,
) -> ContextResult<CompanionSource> {
    let appraisal = load_user_appraisal_source(tx, owner, appraisal_id, authority).await?;
    let lease = appraisal.lease;
    let source = lease.source();
    let revoked = || ContextError::Revoked("Companion source is unavailable".into());
    let source_event_id = source.source_event_id.ok_or_else(revoked)?;
    let conversation_id = source.conversation_id.ok_or_else(revoked)?;
    let reference = AppraisalReference {
        appraisal_id: appraisal.appraisal_id,
        revision: appraisal.revision,
        result_digest: appraisal.result_digest,
    };
''' + roots + finish
(base/'companion/source.rs').write_text(comp_source)

# Companion owns private grant integrity, policy admission, and the lock re-read.
grant = inv[inv.index('fn revoked()'):inv.index('async fn invitation_lease(')]
policy = inv[inv.index('    let row = tx\n', inv.index('async fn invitation_lease(')):inv.index('    ensure_live(tx, Some(locked.valid_until))')]
policy = policy.replace('locked_digest != initial_digest', 'locked_digest != initial.digest()?').replace('locked != initial', 'locked != *initial')
comp_inv = '''//! Companion-owned immutable grants and current live policy admission.
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, Statement};
use serde_json::Value;
use uuid::Uuid;

use super::{
    decision::canonical_digest,
    live::types::{INVITATION_OUTPUT_KEY, LiveAdmissionGrant},
    types::{CompanionMode, CompanionPolicy, PolicySnapshot as CompanionPolicySnapshot},
};
use crate::modules::memory::context::{ContextError, ContextResult};

''' + grant + '''/// The caller must first lock and validate the original canonical source closure.
/// The returned grant remains bound to this transaction; it is not an output permit.
pub(crate) async fn lock_invitation_grant(
    tx: &DatabaseTransaction,
    owner: Uuid,
    admission_id: Uuid,
    initial: &LiveAdmissionGrant,
) -> ContextResult<LiveAdmissionGrant> {
''' + policy + '''    Ok(locked)
}
'''
comp_inv = comp_inv.replace('digest(&value)?', 'canonical_digest(&value)?')
comp_inv = comp_inv.replace('stmt(query, vec![admission_id.into(), owner.into()])', 'Statement::from_sql_and_values(DatabaseBackend::Postgres, query, vec![admission_id.into(), owner.into()])')
comp_inv = comp_inv.replace('.query_one(stmt(\n', '.query_one(Statement::from_sql_and_values(\n            DatabaseBackend::Postgres,\n')
(base/'companion/invitation.rs').write_text(comp_inv)

ctx_inv = inv[inv.index('async fn invitation_lease('):]
ctx_inv = ctx_inv.replace('let (initial, initial_digest) = grant(tx, owner, admission_id, false).await?;', 'let initial = read_invitation_grant(tx, owner, admission_id).await?;')
ctx_inv = ctx_inv.replace('lock_companion_authority', 'lock_appraisal_authority')
start = ctx_inv.index('    let row = tx\n')
end = ctx_inv.index('    ensure_live(tx, Some(locked.valid_until))')
ctx_inv = ctx_inv[:start] + '    let locked = lock_invitation_grant(tx, owner, admission_id, &initial).await?;\n' + ctx_inv[end:]
ctx_header = '''//! Context-owned invitation evidence and immutable lease issuance.
//! The original source closure is locked before a live grant or its policy.
use chrono::{DateTime, Utc};
use sea_orm::DatabaseTransaction;
use serde_json::json;
use uuid::Uuid;

use super::{appraisal_source::lock_appraisal_authority, repository::*, *};
use crate::modules::companion::{
    invitation::{lock_invitation_grant, read_invitation_grant},
    live::types::{INVITATION_KIND, LiveAdmissionGrant},
    source::load_companion_source,
};

fn revoked() -> anyhow::Error {
    ContextError::Revoked("Companion invitation authority is unavailable or changed".into()).into()
}

'''
(base/'memory/context/invitation.rs').write_text(ctx_header + ctx_inv)

old_tests = base/'memory/context/companion/invitation/tests.rs'
tests = old_tests.read_text()
tests = tests.replace('        types::CandidateKind,', '        types::{CandidateKind, CompanionMode, CompanionPolicy},')
tests = tests.replace('use sea_orm::TransactionTrait;', 'use sea_orm::{ConnectionTrait, TransactionTrait};\nuse serde_json::Value;')
tests = tests.replace('let (admission, _) = grant(&tx, f.owner, permit.admission_id(), false)', 'let admission = read_invitation_grant(&tx, f.owner, permit.admission_id())')
tests = tests.replace('delivery.mark_generated_in_transaction(', 'DeliveryRepository::mark_generated_in_transaction(').replace('delivery.prepare_in_transaction(', 'DeliveryRepository::prepare_in_transaction(')
new_tests = base/'memory/context/invitation/tests.rs'
new_tests.parent.mkdir(parents=True, exist_ok=True)
new_tests.write_text(tests)
old_tests.unlink()
old_tests.parent.rmdir()
inv_path.unlink()
inv_path.parent.rmdir()
old_path.unlink()

path = base/'memory/context.rs'
text = path.read_text().replace('pub(crate) mod companion;', 'pub(crate) mod appraisal_source;\npub(crate) mod invitation;')
path.write_text(text)
path = base/'memory/context/repository.rs'
text = path.read_text().replace('super::companion::validate_invitation_reference', 'super::invitation::validate_invitation_reference')
path.write_text(text)
path = base/'companion/mod.rs'
text = path.read_text().replace('pub(crate) mod live;', 'pub(crate) mod invitation;\npub(crate) mod live;\npub(crate) mod source;')
path.write_text(text)
path = base/'delivery/repository/control.rs'
text = path.read_text().replace('crate::modules::memory::context::companion::read_invitation_grant', 'crate::modules::companion::invitation::read_invitation_grant')
path.write_text(text)
