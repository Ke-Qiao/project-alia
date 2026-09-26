# CP05 Repository / admission implementation contract draft

Prepared 2026-09-26 during V2 verification. Read-only design review; no CP05 source, migration, Cargo or PostgreSQL execution is represented here. Root selected the single pending-row transaction model below; no separately committed claim transaction is proposed.

## 1. Feasibility and ownership boundary

The atomic write is feasible with existing APIs: canonical Context authority and evidence reads, a new Companion admission row, `context::repository::persist`, and `DeliveryRepository::prepare_in_transaction` can all share one `DatabaseTransaction`. `ContextService::validate_in_transaction` already accepts that transaction. Appraisal validation reads its original parent and does not require the derived lease's request ID to equal the original parent request ID.

Use a new `CompanionInvitation` purpose, exactly one `CompanionAdmission { id, intent_digest }` dependency, and only `ResponseCommit | OutputAdmission`. No existing Generation or AppraisalRebuild capability changes. `evidence_key` must put the admission after all original source references, including `MemorySourceMessage` currently at ordinal 9.

The source helper currently returns only a `CanonicalObservation` and erasure roots. The issuer also needs the validated original lease with its complete dependencies. Add a private helper result carrying that lease, or have the issuer call the same canonical loader and `load` again in the same transaction. Do not reconstruct dependencies from the observation's single Appraisal reference.

## 2. Concrete Rust boundary

All APIs below are crate-internal. They are not HTTP request DTOs. Names may be adjusted together before implementation.

```rust
// Separate from the shadow report and its immutable candidate table.
pub(crate) struct PendingInvitation {
    pub candidate_id: Uuid,
    pub owner_id: Uuid,
    pub channel_id: String,
    pub expires_at: DateTime<Utc>,
    // Other frozen canonical fields are private, loaded from SQL.
}

// Immutable in a managed caller task across uncertain commit/retry.
// No Deserialize and no public field constructor; Clone is allowed for exact
// intent retry, never for obtaining another permit.
pub(crate) struct LiveAdmissionIntent {
    candidate_id: Uuid,
    admission_id: Uuid,
    initiative_request_id: Uuid,
    context_id: Uuid,
    server_instance_id: Uuid,
    opportunity: VoiceOpportunity,
    expected_scope: VoiceScope,
    intent_digest: String,
}

// No Clone, Deserialize, Default or SQL-query constructor.
pub(crate) struct LiveAdmissionPermit { /* private admission identity */ }

pub(crate) enum AdmissionResult {
    Fresh {
        permit: LiveAdmissionPermit,
        context: ContextBinding,
        response: ResponseRecord,
    },
    Existing(AdmissionReceipt),
    Deferred(DeferReason), // No grant or budget charge; bounded scheduler delay.
    Retired(RetirementReason),
}

// Opaque receipt only. Existing does not return private input/scope/content
// after erasure, and never contains a permit or actor-reserve constructor.
pub(crate) struct AdmissionReceipt {
    pub admission_id: Uuid,
    pub candidate_id: Uuid,
    pub initiative_request_id: Uuid,
    pub state: AdmissionState,
    pub charged_at: DateTime<Utc>,
    pub budget_date: NaiveDate,
}

pub(crate) enum LiveRepoError {
    InvalidInput,
    Conflict,
    Forbidden,
    Unavailable,
    CommitUnknown { candidate_id: Uuid, admission_id: Uuid },
}

impl CompanionRepository {
    async fn discover_live_pending(
        &self, instance_id: Uuid, limit: u16,
    ) -> Result<Vec<PendingInvitation>, LiveRepoError>; // <=32

    // Freezes the actor-issued opportunity once. Never accepts a serializable
    // OpportunitySnapshot / Simulation or client-supplied source authority.
    async fn prepare_live_intent(
        &self, candidate_id: Uuid, instance_id: Uuid,
        opportunity: VoiceOpportunity,
    ) -> Result<LiveAdmissionIntent, LiveRepoError>;

    async fn admit_invitation(
        &self, intent: &LiveAdmissionIntent,
    ) -> Result<AdmissionResult, LiveRepoError>;

    async fn admission_receipt(
        &self, owner: Uuid, admission_id: Uuid,
    ) -> Result<AdmissionReceipt, LiveRepoError>;

    async fn retire_live_batch(
        &self, instance_id: Uuid, limit: u16,
    ) -> Result<u64, LiveRepoError>; // <=32; no output reconstruction
}

// Only Context creates the lease; callers supply no free roots or timestamps.
pub(crate) async fn issue_invitation_in_transaction(
    tx: &DatabaseTransaction, owner: Uuid, admission_id: Uuid,
) -> ContextResult<ContextLease>;
```

`prepare_live_intent` only reads/cross-checks the durable pending row and the trusted actor snapshot. It creates no grant and does not claim actor ownership. The pending row belongs to one server instance. An in-flight managed task retains the exact returned intent through retries; a second scan must not obtain a replacement opportunity for that same in-flight task. After process restart, retire pending rows belonging to the previous instance rather than reconstructing a lost in-memory intent.

IDs use versioned domains plus the stable candidate ID. `expected_scope.generation_epoch = opportunity.scope.generation_epoch.checked_add(1)`; the actor remains the only allocator. SQL unsigned values must use validated typed JSON or NUMERIC(20,0), not unchecked conversion of `u64` to signed BIGINT.

The exact final deadline can be conservatively frozen from candidate/source and the original opportunity/capture deadline. A retry cannot extend it. Mutable transaction `as_of` and budget facts are re-read; the caller cannot provide them.

## 3. Fresh, Existing, and commit uncertainty

Root's selected proof requires **READ COMMITTED**, not REPEATABLE READ:

1. Lock Context policy/account/privacy/relationship, complete canonical roots, Companion policy, then the original pending candidate `FOR UPDATE`. All writers that could admit this candidate use this row and lock order.
2. Hold that candidate row lock through the complete budget + admission + Context + Response write and commit.
3. On a retry, acquire the same row lock first. Only after it is acquired, issue a new SELECT for the original admission identity. Do not reuse an earlier absence result, a replica read, or the transaction-start snapshot.
4. The lock proves the prior admission transaction no longer holds its serialization point. The new READ COMMITTED statement sees its committed writes. If the admission exists, return Existing with no permit. A retired/erased admission remains Existing/Retired, never a new candidate.
5. If admission is absent and the original pending row is still active, belongs to the same instance, has the original source/policy/candidate intent, and has not expired, the previous transaction did not commit the atomic admission write. Only then may the same immutable `LiveAdmissionIntent` retry.
6. A successful new insert followed by an acknowledged successful commit is the only Fresh constructor. Any error from `commit()` is CommitUnknown; do not return the pre-commit constructed permit. Timeout/cancellation that drops the future never transfers a permit to another task.

This proof depends on `candidate_id UNIQUE` in admissions, fixed response key uniqueness, no partial commits, no path deleting an admission after charging, and every writer holding the same candidate lock. It does not prove physical output or an actor/device ACK. It also does not justify changing the opportunity, scope, admission, request, Context or output key when retrying.

A temporary lack of opportunity before `prepare_live_intent` is allowed to leave pending unchanged. Once a specific admission attempt is frozen, retry that opportunity only. If it expires, retire the candidate; do not take a new snapshot to replace it. Restart retires prior-instance pending/admissions without replaying output. This distinguishes ordinary pre-attempt idle polling from uncertain commit reconciliation.

## 4. Database schema and invariants

Use a new migration after `m20260926_000014_companion.sql`; do not amend the already-verified shadow migration.

### Shared source identity

For an enforced cross-mode boundary, add a small immutable `companion_source_claims` table:

| Column | Constraint |
| --- | --- |
| owner_id, source_event_id, kind, generator_version | Composite primary key; kind/version fixed for this first intention |
| origin_mode | `shadow` or `enabled`; immutable |
| policy_revision | Positive, frozen at first claim |
| candidate_id | Stable opaque ID, optional for existing limited shadow worklists |

Backfill from distinct existing `companion_processed_sources` with origin `shadow`, including sources that were examined but limited out of candidate generation. Both future shadow and live source admission use this table under the existing owner privacy lock. A live insert cannot change a shadow claim. Existing shadow candidate and processed-source tombstones must also be checked during migration/transition so an erased candidate is never upgraded.

This is source dedup, not a second separately committed attempt claim. Insert it in the same transaction that first creates the shadow receipt or live pending candidate. Policy changes cannot update origin_mode. A pre-enabled canonical source should additionally fail the enabled policy activation cutoff using its original source time; explicit rebuild never substitutes its new execution time.

### `companion_live_pending`

| Column group | Proposed fields and constraints |
| --- | --- |
| Permanent identity | candidate_id PK; owner_id; source_event_id; kind; generator_version; source-claim FK/composite uniqueness |
| Owner lifetime | server_instance_id; state `pending/admitted/retired`; bounded retirement reason |
| Private frozen input | nullable candidate JSON <=32KiB, candidate_digest, appraisal_id, source_context_id, policy revision/digest, original epochs; all fields erased together on retirement |
| Original time | created_at from canonical original source, expires_at unchanged; no scanner/refetch timestamp grants more lifetime |
| Bounded work | next_check_at may only delay work, never extend expiry; admission_id nullable once admitted |

All candidate/source/policy identity is immutable. Only pending→admitted/retired and bounded scheduling facts may change. No retired→pending. Source/Appraisal columns must not have cascade-delete behavior that removes the permanent identity. Proposed private capacity: 4 pending per owner, 128 globally; use the existing short capacity-lock pattern after source locks, not unlocked COUNT + INSERT. Root may choose different constants before implementation, but they must be explicit and tested.

### `companion_admissions`

| Column group | Proposed fields and constraints |
| --- | --- |
| Permanent identity | admission_id PK; candidate_id UNIQUE referencing the permanent pending row without CASCADE; owner_id; initiative_request_id; fixed output_key; UNIQUE(owner_id,initiative_request_id,output_key) |
| Permanent charge | charged_at from DB clock, budget_date UTC, budget_revision >0; UNIQUE(owner_id,budget_revision); never decremented or removed by source/policy cleanup |
| Read-only state | active/retired, enumerated retirement reason; one-way transition |
| Private grant | nullable typed grant JSON <=64KiB and intent_digest; includes candidate/decision/policy versions, canonical source header, exact actor snapshot, expected scope, exact fixed envelope and deadline |
| Context | nullable context_id, `REFERENCES context_leases(context_id) DEFERRABLE INITIALLY DEFERRED`; active requires a context; erasure may clear its lookup link but never its charge |
| Original opportunity | unique(server_instance_id,opportunity_id) across all admissions, retained as opaque replay fence after erasure |
| Private lifetime | private_until capped by original source/candidate and stated retention; no renewal; private grant/digest cleared together |

Check that active implies complete private grant, digest and live context reference; erased/retired-private inputs cannot be refilled. A before-update trigger rejects immutable-field changes. The fixed envelope uses `deny_unknown_fields`, canonical digest and bounded bytes. Its kind and response output key are both `companion_invitation`; literal comes only from `CandidateKind::FollowUpInvitation.text()`.

Do not cascade admissions from source, Appraisal, Context, policy or relationship rows. Source/authority invalidation first retires admissions and erases their private fields; it keeps permanent identity, opportunity replay fence and charge columns. Existing Context leases themselves are immutable and referenced by Response: the implementation must explicitly decide whether their already-opaque provenance references remain revoked or a dedicated deletion/nullable-FK path removes the new derived lease. Do not attempt an UPDATE that violates `context_leases_immutable`, or accidentally delete charge history while solving this link.

Use private admission limits of 128/owner and 1024/global as a proposed starting point; count active/private storage, not opaque historical charges. Pending and admission source-root links have separate bounded tables and are deleted on erasure. Extend all current source invalidation roots, including graph-node projections and relevant cache columns; preserve the existing no-op/cache-hit exclusions.

## 5. Budget survives erasure and policy changes

Budget calculations must read all charged admissions, including retired/erased/failed/unknown results. They must not filter by current policy revision, candidate state, available private snapshot or delivery success.

- `admitted_today = COUNT(*) WHERE owner_id=? AND budget_date=(DB_now AT TIME ZONE 'UTC')::date`.
- `last_admitted_at` is the most recent durable charge across dates and policy revisions. Index `(owner_id, charged_at DESC)`.
- Revision is the prior maximum per-owner budget_revision plus one, under the existing owner privacy lock; use checked arithmetic. Index/unique `(owner_id,budget_revision)` gives a cheap latest revision without scanning all historical rows.
- UTC date and charged_at are created from one explicit transaction DB time and constrained to agree. A failed transaction writes neither admission nor charge. A reserve or output failure after commit never refunds.
- Day rollover permits a new daily count but does not erase cooldown, old dedup, or awaiting-response history.
- Awaiting response requires a new authorized canonical user message in the original conversation/channel after the charge. Keep only the opaque conversation identity needed to check this after private snapshot erasure; no content or free-form explanation. Presence, elapsed time and silence cannot satisfy it. If source authority needed for that comparison has been erased, fail closed rather than fabricating a response.

No separate mutable daily-counter table is necessary at these small daily limits. The permanent charge rows are the ledger. Owner serialization prevents two candidates consuming the last slot; SQL uniqueness catches implementation mistakes.

## 6. Reuse and required adjacent changes

| Existing implementation | Use / limitation |
| --- | --- |
| `lock_companion_authority` | Existing Context policy→active account→privacy→relationship order and explicit DB time |
| `load_companion_source` | Complete canonical user-message checks; preserve its original-time requirement |
| `context::repository::{load,merge,validate_evidence,persist}` | Private issuer builds new purpose with unchanged original source fields except initiative request; preserve complete roots |
| `ContextService::validate_in_transaction` | Every new permission gate rereads current source and live Companion policy/admission |
| `DeliveryRepository::prepare_in_transaction` | Existing immutable response-key advisory lock and intent conflict; same transaction as charge/grant |
| `DeliveryRepository::response_in_transaction` | Read by response ID; existing-by-key recovery can query the fixed key inside Companion or add one narrow Delivery helper, never call prepare to manufacture a permit |
| `DatabaseConversationProvider::commit_response` | Existing atomic assistant history + mark_generated path after admission; enforce exact final literal in Delivery first |
| `DispatchPermit` pattern | No Clone/Deserialize and no lookup constructor; use same ownership discipline for LiveAdmissionPermit |

The new migration must extend `context_leases_purpose_check`. Delivery must constrain exact prepared envelope, stored response revalidation, final_content, media literal/scope/key/count, and text chunks/End (or explicitly reject text for this audio-only first intent). Existing general Generation contexts must not be allowed to impersonate `kind=companion_invitation` just because they can commit normal responses.

Cancellation writes follow source→admission→Response order. Current policy CAS obtains owner authority and Companion policy before its erasure trigger; ordinary owner admission is serialized by privacy. Direct source mutation holds source first, so admission validation must never lock grant before canonical roots. Delivery validation after Response locking must not discover any new unlocked root: the transaction must already hold its complete original closure.

## 7. Tests that establish the selected proof

1. Same pending candidate: transaction A holds FOR UPDATE, transaction B starts retry and is observed waiting on that row. A commits grant+Context+Response; B acquires lock and returns Existing, no permit, exactly one charge/Response.
2. Same barrier, A rolls back after injected failures at grant insert, Context persist and Response prepare. B's new READ COMMITTED SELECT observes absence; exact original intent can return one Fresh. No half grant, lease or budget is left by A.
3. Client commit ACK loss is a separate integration fault from SQL-lock timeout. Tests 1/2 prove serialization and rollback/commit branching; they do not by themselves prove an actual dropped commit ACK occurred. If an ACK-loss proxy/test hook is used, record the injected boundary precisely.
4. Changed opportunity/scope/intent after a frozen attempt conflicts; expiry retires and never rebinds; old instance restart cannot produce Fresh.
5. Last daily slot with two different candidates: owner lock serializes, one Fresh only. Then policy CAS, source deletion, relationship/privacy clear and GC leave charge count/cooldown/dedup intact.
6. Old shadow/limited/erased source claims cannot create live pending after enabling; explicit rebuild cannot refresh source time or bypass source claim.
7. Erased replay/Existing returns opaque receipt only; no private input, content hash, scope snapshot or log reconstruction. All source-trigger families are covered.
8. Strict Context purpose forbids every non-output use; copied admission evidence on Generation is rejected. Parent Appraisal request remains unchanged while new response request succeeds.
9. All current-policy revocation races fail at later Delivery gates; cleanup and late physical facts remain recordable. No Existing/restart/unknown branch recreates output.

The fixed opportunity/decision deadline is currently only a few seconds. Before the runner package, explicitly test that initial TTS/registration can meet it and that any supported Hold/resume still respects the same deadline. A short deadline may intentionally retire an invitation; it is not permission to silently extend a lease.
