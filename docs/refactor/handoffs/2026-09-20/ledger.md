> 归档副本。原工作树：`/Users/ke-qiao/Developer/project-alia/run/worktrees/alia-duplex-control-ledger`；固定提交：`76f5b040481d618f176fde7e8e5d5264c1cf470c`。文内相对证据路径相对于原工作树的交接目录，未改写原始交接内容。

# B Output Control Ledger — WIP handoff

Date: 2026-09-20. Parent requested bounded closeout before the PostgreSQL run.
Worktree: /Users/ke-qiao/Developer/project-alia/run/worktrees/alia-duplex-control-ledger
Branch: codex/duplex-control-ledger
Base: 8c3f5450fcedfe250cba7936de6fcdf8076c7040
Stage commit: recorded in FINAL.json after commit. No merge or push.

## Ownership and files

Owned source snapshots are under source/ with their original paths, and SHA256
values are in source-manifest.json. The package owns the new repository/control.rs,
repository.rs module declaration only, migration 000013, its migrations.rs registry
entry, tests/output_control.rs, and docs/refactor/output-control-ledger-wip.md.
No runtime actor, Pipeline, Voice, ChannelAdapter or service runtime was edited.

The compile-only Core overlay is under overlay/ with overlay.json. It comes from
../alia-duplex-control-protocol and is NOT part of the stage commit. Cargo.lock
only includes the Core serde_json/sha2 dependency change. Core lib.rs, delivery.rs,
Cargo.toml, playback_control.rs and output_control.rs remain deliberately dirty.
Initial Proto overlays were removed after exposing expected out-of-scope Server
constructor and fake-service migration requirements. They are not in this patch.
The original A worktree remains intact.

## Public API

All exports are in alia_server::modules::delivery::repository::control. Methods
are inherent on the existing DeliveryRepository:

- register_control(owner: Uuid, registration: OutputRegistration)
  -> ControlLedgerResult<OutputControlRecord>
- admit_control(owner: Uuid, server_instance_id: Uuid, command: &BoundControlCommand)
  -> ControlLedgerResult<ControlAdmission>
- query_control(owner: Uuid, binding: &OutputBinding)
  -> ControlLedgerResult<OutputControlView>
- record_control(receipt: &BoundControlReceipt)
  -> ControlLedgerResult<ControlRecordResult>
- unknown_control(command: &BoundControlCommand, reason: &str)
  -> ControlLedgerResult<ControlCommandRecord>
- retire_control(owner: Uuid, server_instance_id: Uuid, binding: &OutputBinding,
  reason: &str) -> ControlLedgerResult<OutputControlRecord>
- recover_expired_controls(limit: u32) -> ControlLedgerResult<usize>

DEFAULT_CONTROL_HOLD_MS = 60000. Core MAX_HOLD_MS = 120000; MAX_CONTROL_COMMANDS =32.
ControlAdmission = Fresh(ControlPermit) | Existing(ControlCommandRecord).
ControlPermit has only command() and deadline_at(), no Clone/Deserialize/public
constructor. Only acknowledged fresh SQL commit produces it. Existing/lookup and
CommitUnknown never authorize command replay.
ControlLedgerError::CommitUnknown carries optional original binding/command_id;
all mutation futures are bounded to 2s with 1800ms PostgreSQL lock/statement limits.
Errors before commit still return no permit; callers must never treat them as a
proved device Rejected. record_control and unknown_control are trusted internal
runtime APIs validating the stored immutable command, not public-user authority.

OutputControlRecord stores original registration, registration_digest, deadline,
Active/Retired status, reason, last_command_sequence, latest_receipt, timestamps.
OutputControlView contains scope plus bounded 32 command records. Command status
is Requested/Unknown/Receipted, with original unknown reason retained alongside
first_receipt (fixed outcome) and latest_receipt. First outcome and current device
snapshot are distinct. ControlRecordDisposition = Advanced/LateOutcome/Duplicate/
Stale. Already-retired local authority is never revived by device Running facts.
Two tables only, at most 65 receipt slots. Polling does not append delivery_events.

## Implemented intent, not yet PostgreSQL acceptance evidence

Registration validates canonical prepared Response's voice_scope/context/source,
original active authority and existing audio owner/backend. It may precede audio
reservation. Deadline is min(Response.created_at+120s, already-existing audio
observation deadlines). Original binding and registered hold maximum never refresh.
Pause hold IDs cannot be reused; Confirm/Resume inherit the first Pause deadline.
Sequence is contiguous, <=32. Original Context checks happen before Response,
control and Delivery locks. Query and late outcome/retire do not re-grant Context.
Late first outcomes are saved without replacing a newer current snapshot; retained
same-observation conflicts reject. Recovery only expires scopes; different live
instances are not deemed dead. No audio outbox, PCM resend, assistant-history write
or media progress mutation is introduced.

## Commands and observed results

Workdir is this worktree; shared target is /Users/ke-qiao/Developer/project-alia/target.
1. CARGO_TARGET_DIR=/Users/ke-qiao/Developer/project-alia/target cargo check -p alia-server --all-targets --locked
   Full initial Core+Proto overlay: exit 101. check-initial.log preserves missing
   fake Playback register_output/control/observe_control methods and constructors
   missing PlaybackStart.output_binding and SessionReady hold-capability fields.
2. Same exact command, Core-only overlay: exit 0, 42.43s. check-core-only.log.
3. CARGO_TARGET_DIR=/Users/ke-qiao/Developer/project-alia/target cargo test -p alia-server --test output_control --no-run --locked
   Exit 0, 46.02s. compile-tests.log. This compiles 9 tests; none were executed.
4. git diff --check: exit 0.
No default/non-default full test matrix, PostgreSQL migration application, runtime,
Linux, device or model validation was started in this package. No pass count is
claimed for the 9 tests. Earlier platform evidence belongs to a separate package.

## Compiled real-PG tests awaiting execution

1. prepared_binding_is_unique_and_first_admission_is_the_only_send_permit
2. original_response_scope_context_and_existing_audio_owner_are_required
3. confirm_and_resume_inherit_original_hold_deadline_and_commands_are_bounded
4. late_original_outcome_does_not_rewind_current_snapshot_or_grow_poll_audit
5. resume_waits_for_committing_revocation_and_never_refreshes_original_context
   (privacy/policy/relationship/root revision/private/expiry/cancel/inactive owner)
6. resume_authority_locks_are_held_until_the_intent_commit
7. expired_recovery_is_owner_independent_but_cannot_steal_a_live_instance
8. control_intent_and_first_outcome_roll_back_with_their_audit
9. actual_lost_commit_ack_returns_unknown_and_lookup_cannot_authorize_replay

They reuse delivery_support's bounded real-PG random schema/catch-unwind/cleanup.
The last test uses a real PostgreSQL wire proxy dropping COMMIT acknowledgement,
with explicit pool close and task join. This fixture has compiled only.

## Explicit remaining work and risks

- Coordinate the PG window and run this existing 9-test target against
  postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_delivery_final using
  require_escalated. Do not use port 5433 or alter task DB settings.
- Inspect and fix real results before claiming lock ordering, revocation races,
  migrations, deadline/capacity or lost-ACK behavior accepted.
- Admission currently validates existing Delivery owner/scope/backend/cancel state
  but does not reject an audio observation deadline that appeared after initial
  registration and is now expired. Review and conservatively cap original control
  authority to those actual deadlines without extending its registration lifetime.
- Receipt comparison prevents backward command watermarks/phase/hold budgets but
  does not currently reject a snapshot sequence beyond last_command_sequence.
  Review that bound against A's exact command/observation semantics.
- Relationship revocation case currently uses the shared plain canonical Memory
  fixture; verify that this fixture actually pins relationship evidence and adjust
  the test source legitimately if ordinary baseline Context survives such a clear.
- The mutation-barrier test uses SQL updates on canonical rows to prove waits;
  additionally review production clear/policy paths' shared lock order before merge.
- Registration collision on an already-bound VoiceScope with a different Response
  may surface NotFound after ON CONFLICT rather than the intended typed Conflict;
  fail-closed but error semantics should be tightened.
- Full Core+Proto integration still requires parent's transport/runtime changes.
  Do not mistake the Core-only compile for protocol-integrated Server acceptance.
- Parent owns runtime temporary 3s hold semantics and cancellation sequencing. No
  runtime owner task has been implemented in this SQL-only package.

## Resource/cleanup state

No PostgreSQL connection, schema, test database or Docker container was created by
this B validation. No DB cleanup was required or attempted. Prior baseline schemas,
alia-refactor-test-db, user alia-postgres:5433 and all caches/volumes were untouched.
Shell sessions 7577 (exit101), 46570 (exit0), 58480 (exit0) have all ended. No active
compile/test/proxy/container task remains. Parent can take the PG window immediately.
