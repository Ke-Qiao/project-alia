# CP05 Repository / admission implementation contract

2026-09-28：本合同已按当前实现更新。Root 已冻结本轮 V1：630 个输入，manifest SHA-256 `58c76e82adf71fed96fa6b113f51185412dd3bd0619a3db747835f3d81accd4a`，文件 `run/validation-20260928/final-source-manifest-v1.json`，并报告正常提交 `a79cb1f`。Root 的冻结验证为 macOS workspace 1036 passed / 0 failed / 5 ignored，另行实际通过 PTY 3 和 Easy-Turn 2，合计 1041 个不同测试；Linux ARM64 Server / Embedded no-default all-targets 编译通过，没有新增 Linux 运行测试。

本合同从 2026-09-26 的设计审阅转为实现记录。当前采用单个原 pending 行事务，不存在另行提交的 attempt claim。Companion V3 定向证据为 38 passed / 0 failed / 0 ignored；该运行早于 V1 冻结，不重复计入全量不同测试数。真实物理 commit ACK 丢失尚未注入，本文不宣布 CP07 完成。完整范围和状态由 Root 在 [恢复入口](resume-checkpoint-2026-09-28.md)、[checklist](checklists/companion.md) 中汇总。

## 1. Feasibility and ownership boundary

The atomic write is feasible with existing APIs: canonical Context authority and evidence reads, a new Companion admission row, `context::repository::persist`, and `DeliveryRepository::prepare_in_transaction` can all share one `DatabaseTransaction`. `ContextService::validate_in_transaction` already accepts that transaction. Appraisal validation reads its original parent and does not require the derived lease's request ID to equal the original parent request ID.

The implemented `CompanionInvitation` purpose uses exactly one `CompanionAdmission { id, intent_digest }` dependency and only `ResponseCommit | OutputAdmission`. Existing Generation and AppraisalRebuild capabilities do not expand. Admission evidence follows original source evidence in lock ordering.

The issuer's private `invitation_lease` path loads the complete original lease in the same transaction after canonical source validation. It preserves all parent dependencies rather than reconstructing roots from the observation's Appraisal reference. The derived Context changes the request identity for the new initiative while retaining the original source facts and parent Appraisal identity.

## 2. Concrete Rust boundary

All APIs below are crate-internal and implemented in `modules/companion/live`. They are not HTTP request DTOs. The excerpt omits ordinary getters and return aliases but uses current names.

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
    candidate: Candidate,
    admission_id: Uuid,
    initiative_request_id: Uuid,
    context_id: Uuid,
    server_instance_id: Uuid,
    opportunity: VoiceOpportunity,
    expected_scope: VoiceScope,
    admit_before: DateTime<Utc>,
    valid_until: DateTime<Utc>,
    digest: String,
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
    Deferred(Vec<DecisionReason>), // No grant or budget charge.
    Retired,
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

pub(crate) enum LiveError {
    InvalidInput,
    Conflict,
    Forbidden,
    Unavailable(String),
    CommitUnknown { candidate_id: Uuid, admission_id: Uuid },
}

impl LiveRepository {
    fn new(db: DatabaseConnection, instance_id: Uuid) -> Self;

    async fn discover_pending(
        &self, limit: u16,
    ) -> Result<Vec<PendingInvitation>, LiveError>; // 1..=32

    async fn discover_pending_excluding(
        &self, limit: u16, excluded: &[Uuid],
    ) -> Result<Vec<PendingInvitation>, LiveError>; // excluded <=1024

    // Freezes the actor-issued opportunity once. Never accepts a serializable
    // OpportunitySnapshot / Simulation or client-supplied source authority.
    async fn prepare_intent(
        &self, candidate_id: Uuid, opportunity: VoiceOpportunity,
    ) -> Result<LiveAdmissionIntent, LiveError>;

    async fn admit(
        &self, intent: &LiveAdmissionIntent,
    ) -> Result<AdmissionResult, LiveError>;

    async fn receipt(
        &self, owner: Uuid, admission_id: Uuid,
    ) -> Result<AdmissionReceipt, LiveError>;

    async fn retire_candidate(
        &self, candidate_id: Uuid, reason: RetirementReason,
    ) -> Result<(), LiveError>;

    async fn retire_admission(
        &self, owner: Uuid, admission_id: Uuid, reason: RetirementReason,
    ) -> Result<(), LiveError>;

    async fn retire_batch(&self, limit: u16) -> Result<u64, LiveError>;
}

// Only Context creates the lease; callers supply no free roots or timestamps.
pub(crate) async fn issue_invitation_in_transaction(
    tx: &DatabaseTransaction, owner: Uuid, admission_id: Uuid,
) -> ContextResult<ContextLease>;
```

`prepare_intent` only reads/cross-checks the durable pending row and the trusted actor snapshot. It creates no grant and does not claim actor ownership. `LiveRepository` receives the actual `VoiceInitiativeHandle::instance_id()`. An in-flight managed task retains the exact returned intent through retries; a second scan cannot obtain a replacement opportunity for that same attempt. After process restart, `retire_batch` retires pending/admitted rows belonging to the previous instance rather than reconstructing a lost in-memory intent.

Discovery excludes the caller's retained candidate IDs before the final SQL LIMIT. The service's inspected rounds and one active attempt per channel prevent a blocked first batch from hiding subsequent candidates. These are bounded in-memory scheduling facts; there is no `next_check_at` column or durable replacement opportunity.

IDs use versioned domains plus the stable candidate ID. `expected_scope.generation_epoch = opportunity.scope.generation_epoch.checked_add(1)`; the actor remains the only allocator. SQL unsigned values must use validated typed JSON or NUMERIC(20,0), not unchecked conversion of `u64` to signed BIGINT.

The implementation freezes `admit_before` to the minimum of the original candidate deadline, opportunity deadline, last capture + 2 seconds and the next UTC day boundary, conservatively truncated to SQL microseconds. It is also checked against the Allowed decision's validity. Output `valid_until` is exactly the original candidate expiry. Durable admission and the actor's final first-reserve check must occur strictly before admit_before. A final DB-time check follows all Context/Response writes; a deferred constraint trigger checks the same strict boundary at COMMIT. Once admitted, the original producer may continue/Hold/resume only before the fixed output deadline and with current source/policy validation. No retry extends either timestamp. Transaction as_of and budget facts are read from SQL, never supplied by the caller.

## 3. Fresh, Existing, and commit uncertainty

Root's selected proof requires **READ COMMITTED**, not REPEATABLE READ:

1. Lock Context policy/account/privacy/relationship, complete canonical roots, Companion policy, then the original pending candidate `FOR UPDATE`. All writers that could admit this candidate use this row and lock order.
2. Hold that candidate row lock through the complete budget + admission + Context + Response write and commit.
3. On a retry, acquire the same row lock first. Only after it is acquired, issue a new SELECT for the original admission identity. Do not reuse an earlier absence result, a replica read, or the transaction-start snapshot.
4. The lock proves the prior admission transaction no longer holds its serialization point. The new READ COMMITTED statement sees its committed writes. If the admission exists, return Existing with no permit. A retired/erased admission remains Existing/Retired, never a new candidate.
5. If admission is absent and the original pending row is still active, belongs to the same instance, has the original source/policy/candidate intent, and has not expired, the previous transaction did not commit the atomic admission write. Only then may the same immutable `LiveAdmissionIntent` retry.
6. A successful new insert followed by an acknowledged successful commit is the only Fresh constructor. Any error from `commit()` is CommitUnknown; do not return the pre-commit constructed permit. Timeout/cancellation that drops the future never transfers a permit to another task.

This proof depends on `candidate_id UNIQUE` in admissions, fixed response key uniqueness, no partial commits, no path deleting an admission after charging, and every writer holding the same candidate lock. It does not prove physical output or an actor/device ACK. It also does not justify changing the opportunity, scope, admission, request, Context or output key when retrying.

A temporary lack of opportunity before `prepare_intent` is allowed to leave pending unchanged. Once a specific admission attempt is frozen, retry that opportunity only. If it expires, retire the candidate; do not take a new snapshot to replace it. Restart retires prior-instance pending/admissions without replaying output. The managed runner retires an Existing receipt's planned grant/Response because no original producer was handed to that attempt; the charge remains. This distinguishes ordinary pre-attempt idle polling from uncertain commit reconciliation.

## 4. Database schema and invariants

Implemented in `m20260926_000015_companion_live.sql`, after the already-verified shadow migration. The shadow migration was not rewritten.

### Shared source identity

The immutable `companion_source_claims` table enforces the cross-mode boundary:

| Column | Constraint |
| --- | --- |
| owner_id, source_event_id, kind, generator_version | Composite primary key; kind/version fixed for this first intention |
| origin_mode | `shadow` or `enabled`; immutable |
| policy_revision | Positive, frozen at first claim |

Backfill from distinct existing `companion_processed_sources` with origin `shadow`, including sources that were examined but limited out of candidate generation. Both future shadow and live source admission use this table under the existing owner privacy lock. A live insert cannot change a shadow claim. Existing shadow candidate and processed-source tombstones must also be checked during migration/transition so an erased candidate is never upgraded.

This is source dedup, not a second separately committed attempt claim. Insert it in the same transaction that first creates the shadow receipt or live pending candidate. Policy changes cannot update origin_mode. A pre-enabled canonical source should additionally fail the enabled policy activation cutoff using its original source time; explicit rebuild never substitutes its new execution time.

### `companion_live_pending`

| Column group | Implemented fields and constraints |
| --- | --- |
| Permanent identity | candidate_id PK; owner_id; source_event_id; kind; generator_version; source-claim FK/composite uniqueness |
| Owner lifetime | server_instance_id; state `pending/admitted/retired`; bounded retirement reason |
| Private frozen input | nullable candidate JSON <=32KiB, candidate_digest, appraisal_id; Context/policy/epoch facts are inside the candidate JSON; the three nullable columns are cleared together on retirement |
| Original time | created_at from canonical original source, expires_at unchanged; no scanner/refetch timestamp grants more lifetime |
| Admission lookup | `companion_admissions.candidate_id` is unique; pending has no separate admission_id or next_check_at column |

All candidate/source/policy identity is immutable. Only pending→admitted/retired is permitted; no retired→pending. Source/Appraisal deletion cannot cascade away the permanent identity. Private capacity is 4 pending per owner and 128 per current schema. Count and insertion share a capacity advisory lock using `hashtextextended(current_schema(), fixed_domain)` after source locks. Independent schemas do not contend, while different owners in the same schema still serialize.

### `companion_admissions`

| Column group | Implemented fields and constraints |
| --- | --- |
| Permanent identity | admission_id PK; candidate_id UNIQUE referencing the permanent pending row without CASCADE; owner_id; initiative_request_id; fixed output_key; UNIQUE(owner_id,initiative_request_id,output_key) |
| Permanent charge | charged_at from DB clock, budget_date UTC, budget_revision >0; UNIQUE(owner_id,budget_revision); never decremented or removed by source/policy cleanup |
| Read-only state | active/retired, enumerated retirement reason; one-way transition |
| Private grant | nullable typed grant JSON <=64KiB and intent_digest; includes candidate/decision/policy versions, canonical source header, exact actor snapshot, expected scope, exact fixed envelope and deadline |
| Context | immutable non-null context_id, `REFERENCES context_leases(context_id) DEFERRABLE INITIALLY DEFERRED`; the existing lease and this opaque link remain after retirement |
| Original opportunity | unique(server_instance_id,opportunity_id) across all admissions, retained as opaque replay fence after erasure |
| Original deadlines | immutable admit_before and valid_until columns; `charged_at < admit_before <= valid_until`; deferred COMMIT check; no separate private_until column or renewal |

Check that active implies complete private grant, digest and live context reference; erased/retired-private inputs cannot be refilled. A before-update trigger rejects immutable-field changes. The fixed envelope uses `deny_unknown_fields`, canonical digest and bounded bytes. Its kind and response output key are both `companion_invitation`; literal comes only from `CandidateKind::FollowUpInvitation.text()`.

Admissions do not cascade from source, Appraisal, Context, policy or relationship rows. `retire_companion_live` retires pending → admission → Response, clears candidate/grant/digests and the root index, and marks the canonical Response cancel_requested. Permanent identity, opportunity replay fence, charge columns, immutable Context lease and its non-null FK remain. Later Context validation fails against the retired admission/current source. No Context mutation or charge deletion is used to implement revocation.

Private admission limits are 128/owner and 1024/current schema; only private grants count toward this bound, not historical opaque charges. One `companion_live_sources` index links candidate IDs to Appraisal, Affect, message, memory, graph, cache, execution and response roots; it is deleted on retirement. The migration installs those source triggers plus graph-node, policy, privacy and relationship invalidation. This implementation inventory is not a claim that every trigger family has a new live fault test; section 7 identifies the actual tested subset.

`retire_candidate` / `retire_admission` lock by original instance and opaque identity without requiring a currently active account or newly valid source authority. This permits cleanup after account revocation and after an unknown commit that actually left a grant/Response. They never reacquire source locks in reverse order. Batch expiry/old-instance cleanup is limited to 1..=32 rows using SKIP LOCKED.

## 5. Budget survives erasure and policy changes

Budget calculations must read all charged admissions, including retired/erased/failed/unknown results. They must not filter by current policy revision, candidate state, available private snapshot or delivery success.

- `admitted_today = COUNT(*) WHERE owner_id=? AND budget_date=(DB_now AT TIME ZONE 'UTC')::date`.
- `last_admitted_at` is the most recent durable charge across dates and policy revisions. Index `(owner_id, charged_at DESC)`.
- Revision is the prior maximum per-owner budget_revision plus one, under the existing owner privacy lock; use checked arithmetic. Index/unique `(owner_id,budget_revision)` gives a cheap latest revision without scanning all historical rows.
- UTC date and charged_at are created from one explicit transaction DB time and constrained to agree. A failed transaction writes neither admission nor charge. A reserve or output failure after commit never refunds.
- Day rollover permits a new daily count but does not erase cooldown, old dedup, or awaiting-response history.
- Awaiting response compares the new candidate's already-validated canonical user message with the latest charge: the conversation must match and its original source_received_at must be strictly later than charged_at. The ledger keeps the opaque conversation ID for this comparison; it does not keep message content. Presence, elapsed time and silence cannot satisfy it. A source that fails current canonical validation cannot supply a new candidate.

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
| `DatabaseConversationBackend::commit_response` | Existing atomic assistant history + mark_generated path after admission; enforce exact final literal in Delivery first |
| `DispatchPermit` pattern | No Clone/Deserialize and no lookup constructor; use same ownership discipline for LiveAdmissionPermit |

The new migration must extend `context_leases_purpose_check`. Delivery must constrain exact prepared envelope, stored response revalidation, final_content, media literal/scope/key/count, and text chunks/End (or explicitly reject text for this audio-only first intent). Existing general Generation contexts must not be allowed to impersonate `kind=companion_invitation` just because they can commit normal responses.

Cancellation writes follow source→admission→Response order. Current policy CAS obtains owner authority and Companion policy before its erasure trigger; ordinary owner admission is serialized by privacy. Direct source mutation holds source first, so admission validation must never lock grant before canonical roots. Delivery validation after Response locking must not discover any new unlocked root: the transaction must already hold its complete original closure.

## 7. Actual fault evidence and remaining boundaries

The following maps the storage/source requirements in live-design section 13 to the actual 7 live Repository, 9 shadow Repository and 5 invitation Context tests. All are in the V3 component run and Root's subsequent frozen workspace run. Related runner/mailbox cases are labeled separately. Test names and earlier failures are retained in [repository handoff](handoffs/2026-09-28/repository.md).

| Requirement | Actual evidence | Limit of that evidence |
| --- | --- | --- |
| Same original transaction, committed outcome | Production `stage_for_test` writes grant + Context + Response without producing a permit; another connection is observed blocked via pg_blocking_pids; commit then yields only Existing with one charge/Response | The blocker holds authority/source/pending locks. The observer proves blocking on the original transaction, not which individual lock the retry first waited on |
| Atomic rollback | Injected Response INSERT trigger failure leaves no admission/derived Context; staged production rollback allows the same immutable intent to return one Fresh; subsequent call returns Existing | Grant INSERT and Context persist do not each have an independently injected failure case |
| Final commit deadline | A real staged transaction is held until admit_before; the deferred COMMIT constraint rejects it and no grant/Context/Response survives | This is a known SQL rejection, not a lost database commit acknowledgement |
| Last daily budget / no refund | Two real `admit` futures via tokio::join! produce one Fresh and one DailyBudgetExhausted; retirement, new policy and new source retain the charge/cooldown; DELETE is rejected | This budget test does not force a pg_blocking_pids barrier at the last slot. UTC midnight and awaiting_response are pure-policy tests, not actual midnight crossing or concurrent-response PG tests |
| Original source time, shadow separation, old instance | Live tests retain original expiry, refuse all five old shadow claims including limited, and retire old-instance pending. Shadow tests cover stable identity/rebuild, stale-source retirement, competing workers and restart | No replacement opportunity/output is reconstructed; this is database/runtime state evidence, not a process killed during network COMMIT |
| Complete parent roots and strict purpose | Context tests compare every parent dependency, preserve original Appraisal/request identity, reject missing/changed evidence, and allow only ResponseCommit/OutputAdmission | The tested Context fixture's roots do not mean every possible source family has an independently mutated live example |
| Current-source revocation and cleanup | Live/Context cases mutate message, policy, privacy and relationship, check private erasure and retained charge, reject later Context/Delivery gates and permit finish_unknown cleanup. Account revocation still permits original-instance retirement | Graph/Cache/Memory/Appraisal trigger-family coverage in the shadow suite is not automatically live trigger coverage. Not every live family or lock interleaving was separately fault-injected |
| Opaque Existing, failed reserve | Runner tests verify Existing causes no Reserve/TTS; rejected reserve, dropped oneshot reply and shutdown retain one charge, retire grant/Response, and produce no replacement opportunity/media | Dropped reserve reply is a controlled mailbox fault without a produced OutputProducer; it is neither database COMMIT ACK loss nor a physical device/network failure |
| Bounded private state | Live owner pending capacity, exclusion-before-LIMIT, same-schema different-owner capacity lock and private erasure are tested; shadow private retention/capacity and opaque tombstones are tested | Constants 128 schema pending / 1024 schema grants and all historical ledger growth are not a new full-capacity soak test. Permanent opaque identity/charges intentionally survive private GC |
| Two deadlines | Real COMMIT expiry case plus Context tests show old short opportunity cannot issue another lease, while existing output validation remains valid until original candidate expiry; boundary equality is rejected | Context tests do not prove actual Hold/resume audio. Root's separate actor/software-paired matrix owns that evidence; none of it substitutes for Pi acoustic validation |

**Commit ACK uncertainty is not a completed physical fault injection.** There is no proxy/network cut after PostgreSQL COMMIT and before its acknowledgement in these tests. The production path conservatively maps commit errors to CommitUnknown and only constructs Fresh after successful commit return. The tested transaction barriers establish serialization and commit/rollback outcomes without manufacturing a permit, but do not exercise an actual lost COMMIT response. A future injected ACK-loss boundary must be recorded precisely and must reuse the same immutable intent.

The current suite found no additional concrete production defect during this documentation review. The incomplete rows above remain explicit evidence limits; they must not be converted into blanket claims that every budget interleaving, write site, source family or physical fault was exercised.

- [x] Current live API/schema, cleanup ownership and two-deadline contract recorded.
- [x] Actual component tests and frozen aggregate results separated; no duplicate test counting.
- [ ] Physical database commit ACK-loss injection, if claimed in a later fault matrix, must have its own recorded transport boundary and result.
- [ ] Stronger last-budget barrier, real UTC rollover/concurrent response, per-write-site failure and every-live-source-family claims need additional focused evidence before those stronger claims are made.
- [ ] CP07 completion is determined by Root's full checklist and acceptance record, not this storage document.
