# Output lifecycle / independent review handoff — 2026-09-23

Worktree: `/Users/ke-qiao/Developer/project-alia/run/worktrees/alia-duplex-integration`, branch `codex/duplex-integration`. Initial paused source checkpoint: `ae75a4ee92b1ea2e8c11a1d20c488ba62125d81f`. This document supplements the 2026-09-22 handoffs and does not replace their historical evidence.

The latest directed tests actually passed: Pipeline 21, DeliveryService 12, OutputLifecycle 11, all with zero failures and zero ignored. They are **not a single frozen-source acceptance**. Core `output_control.rs` changed hash between the Pipeline and lifecycle stages; C attributes that change to rustfmt after adding the retirement types, but no earlier byte snapshot is available to independently prove formatting-only equivalence. Root subsequently bounded retained failure reports. The final frozen source must cover both changes.

## Owned files and completed work

- `packages/alia-server/tests/output_lifecycle.rs`: the original six tests plus five real PostgreSQL regressions for queued Bind → retire, cancellation during Register ACK, semantic expiry during Register ACK, service.cancel_response during Register ACK, and a forged candidate that must not change another request's Response. The identity mismatch must be reported by shutdown after resource join.
- `packages/alia-server/tests/delivery_service.rs`: the original twelve cases preserve their cancellation/Unknown/ACK semantics; its fake adapter now implements typed retirement explicitly.
- `packages/alia-server/src/channels/pipeline/integration_tests.rs`: the new prepared-before-Register-ACK error regression reaches the real notifier, preserves the original Context/Response/request, persists cancellation, creates no extra Context/Response/text/audio, and fails on any replacement TTS invocation. The fake adapter also implements typed retirement.
- `packages/alia-server/src/channels/pipeline/error_notice.rs`: its guard now uses the trusted prepared `turn.response_id` before device registration ACK. No other Pipeline production source is owned by B.

The owned file hashes captured at this handoff are in `owned-source.sha256`. Root subsequently ran rustfmt (skip_children) on the three fixture files without a reported logic change; that manifest therefore predates Root’s final formatting and must not be mistaken for the final frozen source manifest. Root owns Pipeline delivery/turn, DeliveryService/output, ChannelAdapter, and EmbeddedChannel. C owns Core/Proto retirement and Embedded production. No shared-tree commit was made by B.

## Reproduction and verification evidence

All paths below are relative to this worktree.

- `run/validation-20260923/stack-analysis/README.md`: exact `lldb target create`/disassemble commands and before/after prologues. The original five poll frames totaled 2,003,856 bytes; after boxing the concrete Future before constructing the async wrapper, they totaled 632,096 bytes. Measured and later runtime binary hashes are recorded separately. No debugger process was launched and no increased `RUST_MIN_STACK` was used.
- `run/validation-20260923/lifecycle-result.json`: three actual old-production Bind cancellation failures followed by the completed 11-test lifecycle result, all command attempts, source hashes, and PG cleanup. The separate sandbox-denied PG attempt is infrastructure evidence, not a behavioral red.
- `run/validation-20260923/retirement-lifecycle-result.json`: typed retirement fixture migration, guard red → green, all current command lines and source/binary manifests. Old guard body failed with 3 Contexts instead of the original 2; cleanup succeeded, so this is an actual behavioral red. Pipeline 21 passed in 8.13s; DeliveryService 12 in 11.34s; OutputLifecycle 11 in 6.38s.
- `docs/refactor/checklists/duplex-lifecycle.md`: concise staged status and remaining final acceptance.

The three fake adapters validate original scope/binding/backend and await their own explicit software stop barrier. Their registered software-only models return `Settled` with their original backend and no invented playback progress. A never-registered/no-media model returns `NoNativeOwner`. They do not claim physical speaker silence or real ALSA playback.

## Independent final retirement review

Read-only review covered the Server supervisor, EmbeddedChannel cache, Core receipt validation, Embedded retirement/activation, playback gRPC authentication, and stop/join paths. No additional concrete blocking issue was found. The relevant invariants are:

- Bind candidate ownership is ordered with local retirement under the gate; database cancellation independently checks canonical immutable identity before changing a Response.
- An unacknowledged registration retains the candidate binding without inventing the expected backend. The peer must match a retained original registration or prove that no native owner existed.
- Server cache removal targets the exact scope; a late Register ACK cannot refill a removed or replaced admission token.
- Embedded retirement either retires the exact active scope or reads a retained original scope. It does not authorize or fence a replacement scope. Retained capacity evicts only quiescent records; unknown/evicted scopes remain errors.
- Entered native writes stay owned until quiescent. The Server fence deadline records failure but continues awaiting the same future. Authentication is checked again before the peer returns a receipt. Prior retirement failures remain visible to shutdown.

This was source inspection only, with no additional tests or production edits. Root/C's pending frozen combination validation remains necessary.

## Resource state and next work

B has no active exec, build, container, or PostgreSQL window. Last PG verification found exactly the original three `pipeline_delivery_*` baseline schemas, zero `delivery_test_*` schemas and zero other client connections. No extra DROP was needed. PG ownership has returned to Root coordination.

Task DB is only `127.0.0.1:32768/alia_refactor_delivery_final` in `alia-refactor-test-db`; user port 5433 was not touched. Root coordinates all subsequent PG windows.

The next bounded assignment is Linux ARM64 verification preparation and, **only after Root supplies the final frozen snapshot**, execution. See `run/validation-20260923/linux-preparation/README.md`. Current preparation does not create/start containers, connect to PG, or build. Root's macOS workspace validation can run in parallel with Linux non-PG stages; PG stages need a separate coordinated window.

## V2 continuation: runtime fixture and Linux execution

Root V1 full workspace exposed a third fixture mismatch: the isolated precise-cancel-panic runtime case called voice media without an original OutputProducer, so it failed before native entry. Root granted B ownership of `packages/alia-server/tests/server_runtime/delivery.rs`. The fixture now prepares a canonical voice intent, creates/binds the original producer, and provides exact typed software retirement that waits for the blocking native call. All previous panic/native future/dependency assertions remain; after releasing the native call it also waits for typed scope retirement to finish before proving the precise-cancel quarantine still owns the media future. No production guard changed.

Mac exact 1 passed (4.63s), complete ServerRuntime 43 passed (57.01s), no failed/ignored. Source hashes were stable; PG schema lists matched the original baseline and other clients were zero. Evidence: `run/validation-20260923/runtime-panic-fixture/RESULT.json`. The PG window was handed to A for five ignored cases.

Linux V1 (manifest `4e6eadd8…`) completed all non-PG stages: Core 27/Proto 4 passed, Server default and both no-default all-targets checks passed, Server affected tests linked. Embedded full no-default lib had 163 passed/2 failed because two old binding fixtures reused a retained retired scope. V1 source stayed identical; its only task container was removed after resource/inspect capture. Logs, source, and private COW cache remain.

Root froze V2: 605 files, manifest SHA256 `1939a948191002882ba70e2578ad87d266d428b2ba423ccfa31aa178a34998d4`. Exactly two fixture files differ from V1: Embedded binding_admission and Server runtime delivery. B verified all V2 files and is running the complete non-PG matrix in a new dedicated container with V2 mounted read-only. It reuses only the private task cache; historical caches remain untouched. Full ServerRuntime has been added to the Linux PG matrix. Current V2 evidence: `run/validation-20260923/linux-final-v2/evidence`; PG execution waits for A/Root handoff.

## Final B state

Linux V2 is complete: **325 passed / 0 failed / 0 ignored**. Core27, Proto4, full Embedded no-default lib165, Server Pipeline21, Voice23, Delivery12, paired6, output_control13, output_lifecycle11 and ServerRuntime43. Server default and both no-default all-targets checks passed. All 605 snapshot files and the committed worktree match V2 SHA1939a948… after execution; source commit is `581068795fb1dbf5449ed9a0f0b73665c1797a7a`. Current B-owned hashes are recorded separately in `owned-source-v2.sha256`; earlier manifests remain historical.

Full Linux evidence and exact per-stage commands: `run/validation-20260923/linux-final-v2/evidence/result-summary.json`; readable summary: `run/validation-20260923/linux-final-v2/README.md`. All PG schemas matched baseline, zero other clients; no DROP. Both B-created V1/V2 validation containers have been removed after evidence capture. Private COW cache, historical caches, source snapshots, logs, models, and task PG are preserved. B has no active exec/build/test/PG window. Root received the PG window for final macOS V2 workspace validation. No further B tests are authorized or needed for this bounded assignment.

This Linux result covers the specified software matrix; it does not claim full Linux workspace/default Sherpa/vision or physical Pi 5 acoustic verification. Root final macOS workspace result and final integration acceptance remain Root-owned.
