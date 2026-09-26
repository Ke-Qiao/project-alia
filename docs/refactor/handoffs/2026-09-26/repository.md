# Companion Repository / canonical source handoff — 2026-09-26

Root 收口补记：V3 已完成，workspace 1005/0/5、5 项 ignored 单独通过、两平台 no-default 检查通过；详见 [统一摘要](validation-summary.json)。下文“运行中”保留为组件交接当时状态。

Project: `/Users/ke-qiao/Developer/project-alia`, main integration branch `codex/architecture-refactor`. This handoff covers the default-shadow Repository package and the read-only CP05 admission design review. It does not claim that live Companion output, the full R01 checklist, or the current workspace validation is complete.

At handoff, Root's V3 directed Companion run has actually passed: **9 real PostgreSQL Repository cases**, together with 11 pure decision cases and 6 runtime cases. Root's full workspace run is still in progress and has no final acceptance result in this handoff. Do not infer workspace success from the directed run or partial workspace log.

## Owned implementation

- `packages/alia-server/src/modules/companion/repository.rs` and `repository/tests.rs`: fixed shadow worklist/receipt, owner policy CAS, source discovery/retirement, authorized historical replay, bounded private retention, and 9 real PostgreSQL regressions.
- `packages/alia-server/src/modules/memory/context/companion.rs`, plus its minimal export in `context.rs`: canonical read-only source/authority admission. This helper never creates a ContextLease or borrows Generation/AppraisalRebuild authority.
- `packages/alia-server/migrations/m20260926_000014_companion.sql`, registered in `modules/platform/migrations.rs`: shadow tables, immutable identity, opaque processed/rejected tombstones, and transactional source/authority erasure.

Root owns service/API/Builder integration and runtime tests. The pure types/decision layer belongs to the policy agent. There is no independently committed CP05 implementation from this agent.

Repository entry points are `policy`, `replace_policy`, `evaluate`, `replay`, `discovery_pending`, and `prune_expired`. The internal `evaluate_with_opportunity` now accepts `Option<OpportunitySnapshot>`; HTTP callers cannot provide this argument. Typed errors distinguish invalid input, conflict, forbidden, erased and unavailable storage.

## Source, receipt and restart invariants

An explicit operation accepts 1–32 distinct canonical Appraisal IDs owned by the authenticated owner. Sorting fixes the operation intent; the same operation with different IDs or owner conflicts. It generates at most 4 candidates and records the remainder as limited observations. The source read, pure decision and immutable receipt all commit in one bounded SQL transaction, with a local 2-second statement timeout. No model await occurs inside the transaction.

The first intention admits only `AppraisalFact::UserMessage`. It validates the current Context policy, active account, privacy and relationship epochs, original parent lease, Appraisal revision/result digest, and the complete flattened dependency closure. Candidate time remains the canonical original `effective_at == source_received_at`. Scanning, rebuild and restart do not refresh it. The original user-message preparation already obtains its source from the SQL lease before assigning Appraisal effective_at.

Candidate identity does not depend on operation ID, policy revision or a rebuild's Appraisal ID. Processed-source pins and candidate dedup tombstones survive erasure. Canonical rows with irreversibly stale parents receive an opaque rejected-source pin, preventing the first stale batch from starving later eligible sources. No replacement epoch or content is retained by that retirement path.

Replay first rechecks current owner authority, policy/epochs and canonical source closure. It verifies the stored digest and repeats the pure decision over its frozen input; it does not reread a sensor or call a model. Invalid or erased private input is unavailable for replay. A new Repository instance can read the same authorized immutable receipt but cannot use it as output authority.

Repository and API still reject Enabled live admission. Shadow evaluation does not create Appraisal, Affect, Response, Execution or a new ContextLease. Simulation is explicit offline evidence and never grants Allowed. Restart preserves original identities and times, not an output replay queue.

## Erasure, storage and lock boundaries

Private operation snapshots are at most 128 KiB. Private completed operations are capped at 128 per owner and 1024 globally; the owner privacy lock serializes the owner quota, and a short global capacity lock serializes the final global count/insert. Private retention is capped by the earliest original source validity and `source_received_at + 24h`.

`prune_expired(1..=128)` erases a bounded batch using operation-ID order and `FOR UPDATE SKIP LOCKED`. It never reads source authority after taking operation locks. Root's worker runs bounded pruning before discovery. Erasure clears private operation snapshots/digests/retention timestamps, candidate input JSON and source links, while preserving opaque identity and dedup pins.

Privacy/relationship epoch changes, Companion policy revision changes and Context policy version changes erase affected private snapshots transactionally. Source invalidation covers Appraisal, Affect, user/derived messages, Memory, graph edges and projected graph-node fields, cache projections, Execution and Response ancestry. Cache hit counters and unrelated/no-op projection updates do not erase valid history. The graph-node trigger runs before deletion so it can still find linked edges before cascade removal.

Normal evaluation follows authority → Companion policy → canonical source → operation write. Historical reads deliberately do not lock the operation before validating its source, because source erasure follows source → operation. Destructive/replay paths must preserve that ordering. These shadow locks are not a claim that the future CP05 admission/Delivery graph is already implemented or checked.

## Actual verification evidence

Root's [V3 directed result](../../../../run/validation-20260926/companion-v3-result.json) records exit 0, no source drift, source manifest SHA-256 `8ea6d4f7cc63b82d22b8c584c8c0b426b11e04f952f64bd15b7776d352cc18a1`, 2 build jobs, default test threads and stack, and no `DOCS_RS`. The [V3 log](../../../../run/validation-20260926/companion-v3.log) records the 20-case lib group in 1.20 seconds and the 6-case runtime group in 14.63 seconds; the complete directed command took 68.03 seconds including compilation.

The 9 real Repository cases cover:

1. Competing workers, same-operation retry, duplicate-source suppression and a new Repository instance reading the original receipt without production ledger/model writes.
2. Injected candidate-insert failure rolls back the entire operation/candidate write; the original intent can retry.
3. Source revision and relationship clear physically erase private replay input while tombstones persist.
4. Owner-scoped privacy erasure, policy CAS and rejection of shadow-to-Enabled promotion.
5. Five canonical observations produce four candidates plus one limited observation; explicit Simulation remains ShadowWouldAllow and adds no ContextLease.
6. Stale parents retire without discovery starvation; source retiming is rejected.
7. Private capacity and bounded expired GC preserve identity tombstones.
8. Actual cache/graph SQL triggers ignore ordinary cache hits and erase changed cache/graph-node projections. This trigger-focused fixture adds explicit root pins; it is not evidence that the test built every graph ancestor through the full source pipeline.
9. Missing opportunity is frozen exactly at database report time; future Simulation and VerifiedLive snapshots are rejected without operation, candidate, processed or rejected-source writes; the valid frozen result replays identically.

Root adopted the prepared service regressions and V3 passed all 6 runtime cases: owner policy CAS/restart, HTTP durable retry/erasure/no side effects, canonical scan plus restart/stop/join, public-request drain, eight-request saturation/cancellation, and SQL-lock prewrite timeout followed by original-operation retry. The scanner restart test creates a second canonical source as a real progress witness before checking that the old receipt is unchanged; it does not infer correct restart from an arbitrary quiet sleep.

The HTTP timeout case blocks before writes and proves a 503 followed by safe original-intent retry. **It is not a commit-ACK-loss test.** No test here claims that an unknown physical output or lost commit ACK was replayed safely.

Historical [initial Repository evidence](../../../../run/validation-20260926/companion-repository-initial.result.json) contains the first 8-case PG pass and a separate sandbox-denied attempt. The permission-denied attempt is infrastructure evidence, not a behavioral failure. V3 supersedes that earlier 8-case count for the current source.

## V1 scan timeout investigation and clock correction

V1 full workspace reported a 10-second wait timeout in `companion_runtime_discovers_canonical_sources_and_joins_worker`. Its scanner had no structured progress or error-class observation, so that log does not establish whether discovery was empty, source validation rejected, or storage failed.

The exact V1 test binary was rerun once alone and twice concurrently against isolated task schemas. All three passed: 9.42 seconds alone and 9.47/9.46 seconds concurrently. Read-only SQL observation saw one Appraisal per schema, no rejected tombstone, and no receipt before the source approached the first five-second scan. Evidence is in the [reproduction result](../../../../run/validation-20260926/companion-scanner-repro-v1.result.json) and [opaque SQL observations](../../../../run/validation-20260926/companion-scanner-parallel-sql.log). This did not reproduce or explain the original V1 timeout.

The nanosecond hypothesis was checked against the actual path: `prepare_appraisal` validates/reloads the parent from SQL, and both compared timestamps are SQL TIMESTAMPTZ values. The base no-Memory Context has no short validity deadline; Appraisal default turn/session TTLs are 45/900 seconds. No five-second source-expiry path was found for this fixture.

A separate real logic issue was corrected: missing runtime opportunity had used host `Utc::now()`, while the decision's `as_of` used PostgreSQL time. A lagging DB clock could make an unavailable snapshot appear to be from the future. The internal interface now passes None and the Repository creates Unavailable at `authority.as_of`; supplied future Simulation/VerifiedLive snapshots still fail validation. The ninth PG case verifies this boundary. **The clock correction is not asserted to be the V1 failure's root cause.**

Root added `ScanProgress` counters and fixed stage/error-class diagnostics with no owner/source IDs, private evidence or raw SQL errors. They make future scanner failures observable without extending timeouts or adding arbitrary retries.

## CP05 next-package contract — design only

The [live design](../../companion-live-design.md) and [Repository contract draft](../../../../run/validation-20260926/cp05-repository-contract.md) remain unimplemented/unaccepted plans. The latter supplies concrete Rust interfaces, admission/pending/source-claim columns and constraints, reusable Context/Delivery APIs, and tests. Proposed extra tables/capacity constants are not already-migrated features.

Root explicitly selected a **single pending candidate row lock**, without a separately committed claim transaction:

1. Use READ COMMITTED. Every admission writer locks the same original pending row `FOR UPDATE` and holds it through the atomic budget + admission + dedicated Context + Response prepare transaction and commit.
2. An uncertain-commit retry must first acquire that same row lock, then issue a new statement reading the original admission. Never use an earlier absence result, replica read or REPEATABLE READ snapshot.
3. If the admission exists, return Existing with no permit. If absent after the lock is obtained, and the original pending row is still active with the same immutable intent, the prior transaction did not commit that atomic write. Only that branch may retry the same candidate, admission, request, Context, output key and frozen opportunity.
4. Only a new successful write with acknowledged successful commit returns Fresh. Any commit error returns CommitUnknown with the original identity. Expiry retires; it does not bind a new opportunity. Managed caller tasks retain the exact attempt intent; restart retires prior-instance pending instead of reconstructing lost authority.

This proof requires the candidate/admission and fixed Response-key unique constraints, all writers obeying the same lock, no partial transaction commits, and no deletion of a charged admission. Tests must control a real pending-row blocker and both commit/rollback outcomes. Those tests do not automatically count as a physically injected lost commit ACK; evidence must name the actual fault boundary.

Old shadow, erased and limited-worklist sources cannot acquire live authority after a policy change. The draft recommends shared immutable source claims backfilled from all processed-source pins, not just rows that received a Candidate. Admission privacy cleanup must not cascade-delete charge history: UTC day counts, cooldown, dedup and unknown/retired attempts remain charged across policy revisions and source erasure. Existing should return an opaque receipt rather than private frozen input after erasure.

Context can reuse `load`, `merge`, `validate_evidence`, `persist` and transaction validation, but needs the original complete lease closure, a new restricted purpose and an admission evidence validator sorted after source roots. Delivery's existing `prepare_in_transaction` is the atomic prepare hook. Exact envelope/final text/media scope and representation count must be enforced at all relevant Delivery gates before Enabled is opened. None of this handoff authorizes enabling live output before the coordinated implementation and validation.

## Resource and handoff state

This agent has no active exec, build, PostgreSQL window or validation container. The last agent-owned reproduction cleanup restored the original three `pipeline_delivery_*` baseline schemas, left no `server_runtime_*` schema and reported zero other client connections. Root owns subsequent V3 PG/Cargo execution and final cleanup evidence.

Only the task database `127.0.0.1:32768/alia_refactor_delivery_final` in `alia-refactor-test-db` was used by this agent; the user database on port 5433 was not touched. This handoff was written without source edits or Cargo/PG execution. Wait for Root to release the next implementation package; do not infer permission to modify the frozen source or restart testing from this document.
