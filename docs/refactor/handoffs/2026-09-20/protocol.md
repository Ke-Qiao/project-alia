> 归档副本。原工作树：`/Users/ke-qiao/Developer/project-alia/run/worktrees/alia-duplex-control-protocol`；固定提交：`2619a28fad25dd425d718f0ba9e50a7d55c2fe3b`。文内相对证据路径相对于原工作树的交接目录，未改写原始交接内容。

# Duplex package A — persistent handoff

This is a checkpoint requested because account quota is low. Do not infer complete end-to-end duplex recovery from this package. Server integration is not finished. No new implementation task should start until the parent resumes it.

## Ownership and Git

- Worktree: `/Users/ke-qiao/Developer/project-alia/run/worktrees/alia-duplex-control-protocol`
- Branch: `codex/duplex-control-protocol`
- Base: `213d5ede62fc714c028f0f009730ef060f424bf8` (Stage1).
- Scope: Core outer output-control types; Proto playback v3 / voice v2; Embedded registration/control RPC, capture barrier, Held idle and associated tests; one phase document.
- No Server source was edited. `crates/alia-core/src/turn.rs` was not edited; parent independently owns its new `accept_evidence()` method in `run/worktrees/alia-duplex-runtime`.
- Parent authorized a WIP phase commit at shutdown. Its SHA and final Git status are in `commit.txt` and `git-status.txt` after creation. Do not merge/push from this agent.
- Runtime evidence is ignored by Git under `run/`; preserve this directory and sibling `run/duplex-control-validation/` when moving the branch.

## Exact implemented interface

Shared source: `crates/alia-core/src/output_control.rs` and `crates/proto/src/output_control.rs`.

```rust
OutputBinding { scope: VoiceScope, response_id: Uuid, context_id: Uuid, server_instance_id: Uuid }
OutputRegistration { binding: OutputBinding, backend: SubmissionBackend, maximum_hold_ms: u64 }
BoundControlCommand { binding: OutputBinding, command: ControlCommand, expected_backend: SubmissionBackend }
BoundControlCommand::{validate(), canonical_bytes(), intent_digest()}
BoundControlReceipt {
    observation_sequence: u64,
    command: BoundControlCommand, intent_digest: String,
    receipt: ControlReceipt, backend: SubmissionBackend,
    mode: ControlMode::SoftwareGate, native_writes_quiescent: bool,
    progress: Vec<PlaybackProgress>,
}
BoundControlReceipt::validate(&expected_command)
```

The digest is SHA-256 of a fixed typed serde_json representation prefixed with `alia.playback.bound-control.v3\0`. No generic canonical JSON library or handwritten field codec. There are no arbitrary maps/floats in this intent. Both server agents were informed of this API, then of the added nonzero monotonic `observation_sequence`. The counter and snapshot sample share the original PlaybackControl mutex; observation never applies a new command or renews a hold. Immutable original outcome and current snapshot have separate semantics.

`alia_proto::output_control` public codecs:

- encode/decode scope, backend, binding, registration, command, control_request, snapshot, progress, control_receipt.
- `decode_control_receipt(&wire, &expected)` checks exact intent/backend/identity plus bounded checkpoints and current state consistency.
- Unknown enums, nil/noncanonical UUIDs, missing identities, bad digest, zero observation sequence, unimplemented presentation proof fail closed.

Wire additions:

- package `alia.playback.v3`; `RegisterOutput`, `Control`, `ObserveControl` unary RPCs.
- `RegisterOutputRequest { binding, contract_version }` / receipt `{binding,backend,maximum_hold_ms,contract_version,software_hold}`.
- `ControlRequest { command, intent_digest }` / `ControlResponse` mirrors BoundControlReceipt (`observation_sequence` is field 8).
- `PlaybackStart.output_binding` (field 5), version must 3, exactly pre-registered. No implicit registration.
- package `alia.voice.v2`; Open/Ready must negotiate software_hold and max_hold_ms.
- `TurnEvidence.output_decision` field 7: Unspecified=0, Unchanged=1, PauseRequested=2, ResumeRequested=3, Cancelled=4.
- Same-epoch evidence never changes the control gate. Valid admitted Complete+Unchanged increases generation; Cancelled increases generation without admission. Pause/ResumeRequested require no admission and the same generation. Parent requested this v2 change explicitly during implementation.

Core `MAX_HOLD_MS` is now 120000. Open advertises it, Ready must be 1..=local maximum and software_hold=true. Server default60s and temporary semantic3s are parent-owned policy work. Default60s was a declared assumption after waiting for the user's duration preference, not an explicit user selection.

## Embedded implementation map

- `module/playback_session.rs`: registration/capture/max capability retained by ActiveSession, begin_bound exact registration check, revoke_all under one Session lock, precise cancellation split into sync admission and observation.
- `module/playback_session/capture.rs`: cloneable local actual capture owner, bounded through/latest-speech watermarks, before-try_send admission under the same Session lock as Resume; failed upload or gaps retire. Ready-before-upload and future generations inherit genuine retained watermarks.
- `module/playback_session/control.rs`: register_output, apply_bound_control, observe_bound_control, fixed original owner/backend, optional native quiescence wait. Missing original owner/command is typed Unknown; conflicting outer identity/intent is Conflict.
- `grpc/playback_service.rs`: authenticated independent unary RPCs; Binding read → Session → backend lock order; no native wait under binding/session locks; reauthentication after successful wait. Unknown→NotFound, identity conflict→AlreadyExists; other RPC failures do not become Core Rejected. Start requires exact binding. Held freezes only remaining 5s media idle; 120s original observation deadline/auth/progress/disconnect stay active.
- `module/builder.rs`: binding revocation now synchronously retires PlaybackSession as well as Motion, including registration without a Push.
- `module/speech/duplex.rs`: actual Session.enqueue writes capture barrier before try_send, Guard atomically installs capture owner at Ready, protocol v2 capabilities, strict decision/epoch matrix; same-epoch ResumeRequested is never permission to open the gate.
- Tests: existing playback fixture was upgraded to real signed binding/tokens and exact registration. New `grpc/playback_service/tests/control.rs` owns new RPC tests. Voice tests use a held Ready semaphore and real queued capture before accepting Ready.

`owned-paths.txt` is the complete 20-file source/dependency list; `docs/refactor/duplex-control-protocol.md` is the phase documentation. Cargo.lock only adds four existing dependency edges (Core serde_json/sha2, Proto alia-core/uuid); no dependency version changed.

## Validation and exact evidence

Environment: macOS26.6.2 ARM64, rustc/cargo1.94.1; precise output in `environment.json`.

All commands ran in this worktree with:

```sh
CARGO_TARGET_DIR=/Users/ke-qiao/Developer/project-alia/target
SHERPA_ONNX_ARCHIVE_DIR=/Users/ke-qiao/Developer/project-alia/target/sherpa-onnx-prebuilt
```

The final runner is `../duplex-control-validation/validate.py`. `../duplex-control-validation/results.json` stores exact argv, exit code and wall time; raw target logs are adjacent.

| Command | Exit | Actual result | Wall time |
| --- | --- | --- | --- |
| `cargo test -p alia-core -p alia-proto --offline --locked` | 0 | Core27 + Proto3, docs0 | 6.707s |
| `cargo check -p alia-embedded --no-default-features --all-targets --offline --locked` | 0 | Complete no-default target check | 12.145s |
| `cargo check -p alia-embedded --all-targets --offline --locked` | 0 | Complete default target check | 13.670s |
| `cargo test -p alia-embedded --no-default-features --lib --offline --locked -- --nocapture` | 0 | 145 passed,0failed,0ignored | 27.155s |
| `cargo test -p alia-embedded --lib --offline --locked -- --nocapture` | 0 | 145 passed,0failed,0ignored | 41.709s |

Default tests actually link Sherpa v1.13.1 arm64 static libs. Local archive SHA-256: `ca7baf61f331ea6b848e3630e566647302c26960c7303932bc33125074871eeb`. No native link bypass (`DOCS_RS`) or unpinned network dependency install. No PG or Docker used by A.

Focused checks before the final matrix: playback RPC18/18 with real signed binding; voice11/11 (including late new v2 decision matrix). Their raw outputs are retained as `run-protocol-controls-first.log` and `run-protocol-voice-v2-first.log` under validation. Final library totals include these tests.

Earlier resolved failures, preserved for provenance:

1. `run-protocol-core-check.log` was overwritten by the green Core26/Proto0 follow-up after an initial missing parenthesis around a match cast. Original session87407 exited101; no remaining failure.
2. `run-protocol-embedded-check.log` exited101 because old test fixtures missed output_binding, Ready capability fields, and Session capture fields. Corrected fixtures compiled in compile-v2 and passed all later tests.
3. A final rustfmt check only requested alphabetic sorting of two `pub mod` declarations in Core lib.rs. The before-format tested manifest is preserved. Sorted it, then reran Core/Proto to `core-proto-after-format.log`; no Embedded behavior changed and no extra full Embedded rerun was needed for declaration order.

Expected panic text in unrelated existing Motion fault-injection tests is intentional; the final library exit codes and result lines are green.

## Source integrity

- `tested-source-manifest.json` pins all owned source/dependency bytes used by the final full Embedded matrix.
- `final-source-manifest.json` pins final code after the sole Core module declaration ordering change.
- Final manifest SHA-256: `f8d344a9f172e929738b3021b199b542406269c0a0996325510b62233cddf4c9`.
- Final Cargo.lock SHA-256: `cb4058f1accf88cc6ce4d05a51d58749d2cc96290240d7fa9c8f694d10a9d02b`.
- Per-file digest and size are in both manifests. The phase document is added after verification, outside the code manifest.
- No `turn.rs` change or Server/other-agent overlay belongs to this branch.

## Remaining work and next safe continuation

1. Parent must integrate A with B's persistent output-control ledger and C's Server runtime/voice/pipeline/TTS work. The standalone A branch intentionally has new Proto that the old Server baseline does not yet compile against; no workspace-green claim is made.
2. Server must register canonical Response/Context before Start, preserve the original backend and intent for retry/Observe, gate every later control against original context/policy/epoch roots, and update current device snapshot only for greater observation_sequence. Never turn transport Unknown into Rejected.
3. Server must retain original dispatch owner and bounded TTS/media source while held; validate 60s configurable default against120s engineering and original observation deadline; implement ExplicitContinue vs FreshBackchannel decision provenance. No new feature flag or compatibility stack.
4. Re-run combined default workspace and real Server/Embedded loopback after integration. This package has no PG/server ledger tests, LinuxARM64 rerun, real Pi/audio device/AEC/physical pause or audible continuity proof.
5. A read-only lifecycle concern was noticed immediately before quota shutdown and intentionally NOT expanded: `SessionGuard.accept(Ready)` activation itself currently relies on the surrounding 250ms binding-change monitor; unlike register/control/Start, Ready does not hold the Binding read lock while activating. Before end-to-end production acceptance, review a binding change racing late Ready and decide whether to pass the original binding snapshot into exchange and admit each Ready/evidence under an exact-current-binding read guard. No claim is made that this race has a failing test or has been fixed. Existing binding-change and synchronous output-revocation tests are green.
6. Public local Stage1 helpers remain available for local backend tests; production network paths use only registered bound controls. Do not wire an unbound helper as a shortcut when integrating Server.

## Processes and clean stop

Final wrapper session45995 completed exit0. Core/Proto post-format session45949 completed exit0. Earlier local exec sessions87407/49798/17519/96708/62500/55871/97294 were all explicitly collected and completed (initial101 failures explained above);13310 and35095 also completed. No ongoing shell/test session or task-created container remains. No Docker/PG service was touched. Local loopback servers are joined by their fixtures.

Checkpoint created: `2619a28fad25dd425d718f0ba9e50a7d55c2fe3b`. Final `git status --short --branch`: `## codex/duplex-control-protocol` (clean tracked/untracked state; local run evidence intentionally ignored). No active exec session or task-created container remains.
