# Expression runtime and Body delivery

Expression planning turns an authorized response into speech style and bounded
motion intent. Context owns personal evidence and policy authority; Body owns
actuator command identity and execution facts. This document describes the
current contracts. It does not enable hardware or change deployment policy.

## Canonical interpretation and planning

The production flow is:

```text
original source and immutable privacy / relationship epochs
  -> Context generation lease and versioned policy
  -> canonical AppraisalInput
  -> approved interpreter drafts
  -> supervised AppraisalWriter database commit
  -> validated affect / relationship projection
  -> plan_with_context
  -> canonical Response
  -> bounded Body delivery
```

`observe_context_user`, `observe_context_recall`, `observe_committed_response`
and `observe_execution_completion` obtain canonical inputs from Context. The
writer validates source authority, policy version, interpreter identity and
input/result digests before publishing an `AppraisalReceipt`. Drafts are not
committed facts. `appraisal_records` and `affect_projections` hold the canonical
records; the runtime has no private relationship cache.

`plan_with_context` validates the generation lease before and after planning,
checks its original owner and channel, and consumes its frozen affect and memory
projection. The separate `plan` helper uses a nonpersonal baseline for local
planning/evaluation. It does not create output authority.

Planning preserves semantic intent, regulation, speech style and a bounded
motion timeline. Built-in policy still applies action allowlists, negotiated
capabilities, intensity/duration limits, frequency limits and minimal-expression
constraints. Registered planner and safety extensions participate before the
final built-in validation. A missing motion capability leaves speech available.

Runtime policy comes from existing configuration and the persisted Context
policy. Removing obsolete interfaces does not relax policy or enable output.

## Extension contract

Rust integrations can register:

- `AppraisalEnginePlugin`: interprets a canonical input into drafts. Private
  interpretation requires a stable implementation pin approved by persisted
  policy; registration alone grants no authority.
- `ExpressionPlannerPlugin`: transforms semantic plans before safety checks.
- `ExpressionSafetyPolicyPlugin`: clamps or removes planned actions and records
  the reasons for its changes.

External expression plugins use the JSON stdin/stdout ABI defined in
`src/expression/abi.rs`. Current capabilities are `appraisal_engine`,
`intent_planner`, `plan_transformer` and `safety_policy`. Appraisal responses
contain event drafts, never replacement affect snapshots. Canonical external
appraisal additionally requires implementation approval, supervised cancellation
and disabled raw stderr tracing. A loaded manifest alone does not satisfy those
gates; a dispatcher without a provenance pin cannot contribute private drafts.

The mutable affect reducer interface, `reduce_affect` request, snapshot response
and source-free observation APIs have been removed. Unknown capability fields
are rejected instead of silently retaining a second extension contract.

## Body command ownership

`ExpressionOrchestrator` belongs to one runtime instance. It serializes phases
per channel and owner, preserves the original response/plan/scope, coalesces
compatible timeline actions and negotiates `expression-command-v1` support.
Execution requires a nonempty canonical Response ID. Missing authority returns
a local `skipped` result without contacting an actuator adapter.

The Embedded channel's `execute_expression`, `observe_expression` and
`cancel_expression` actions are internal adapters to signed Body v1. They are
not a separate device RPC protocol. The old `alia.expression.EmbeddedExpression`
service, its JSON execute/cancel methods and its protobuf have been removed.

Body delivery requires:

1. A configured trusted device ID and public-key pin matching the connected
   device, a durable Body delivery store, and open local admission.
2. Revalidation of the original Response and Context/source authority before
   durable claim and before dispatch.
3. A signed device control lease identified by device boot, server instance and
   generation; each command adds a monotonically increasing sequence.
4. The original command identity for observation and cancellation. The command
   contains its owner, plan, immutable source correlation, scope and actions.

Public device status and `online` are reachability information, not output
permission. The device checks current binding, bearer scope and signed Body
payloads. Admission occurs under the binding lock; binding changes revoke old
output authority. Preparation is bounded and followed by another authorization
check before writer admission.

One retained native writer owns calibrated startup centering, expression frames,
face tracking and backend cleanup. Local `motion.calibration_confirmed` is
required before startup motion; remote commands cannot provide this confirmation.
Stop cancels future targets and waits for entered native work without adding a
center or disable command. The old soft-start and shutdown-reset settings are
rejected. Channel limits, angle validation and speed limits between submitted
software targets remain; the initial physical position is unknown. Animation preparation
validates file location, size, frame shape and the complete expanded plan before
native output. The command signature covers the filename and parameters; it is
not a signature of the file's bytes.

## Evidence and recovery

Body results distinguish `accepted`, `started`, `submitted`, `cancelled`,
`failed`, `unknown` and `rejected`. Local negotiation/admission may return
`skipped`. Every device-derived result must carry the original Body command and
validated progress; the orchestrator checks owner, plan, source ID, scope and
action receipts against that evidence.

`submitted` means the servo frames reached the reported host-write or I2C
register boundary. It does not mean a motor reached a position. Current drivers
have no position feedback, so `position_confirmed_actions` stays unknown.
Partial and uncertain native writes retain their observed prefixes and channels.

There is no `completion_acknowledged` Boolean or `executed_actions` compatibility
path. Writer retirement requires the original Body progress to have a terminal
state and `writer_quiescent`. A terminal `unknown` can establish writer
quiescence while leaving the physical outcome unknown; neither fact substitutes
for the other. A result with no Body evidence cannot retire a writer.

Intent and dispatch facts are persisted before Submit. After crossing the
possible-admission boundary, the owner observes or cancels the original command
and does not blindly retry Submit. A lost ACK, connection failure or expired
observation can remain Unknown. Later checkpoints do not rewrite that uncertainty
as proof of physical completion. An Embedded process restart changes boot
identity and rejects old control; independent control-board restart detection
and actual physical feedback remain hardware work.

## Diagnostics and relationship operations

`expression_traces_from_events` groups `tts_provider_adjustments` and
`sentence_expression_llm` diagnostic events. It does not reconstruct the removed
plan/input trace, flat intent payloads, unbound affect snapshots or source-free
appraisal events. The production pipeline does not persist raw personal
expression snapshots into conversation events. The trace helper is not an
authoritative relationship replay or delivery ledger.

Authenticated relationship routes use Context authority:

- `GET /api/v1/expression/relationship`
- `DELETE /api/v1/expression/relationship`, with operation ID and expected epoch
- `POST /api/v1/expression/relationship/prune`, with a bounded limit
- `POST /api/v1/expression/relationship/rebuild`, with its explicit operation
  identity and bounded continuation

The delivery and appraisal stores are the source of execution/provenance facts.
Historical database migrations remain a journal; removal of runtime compatibility
does not reopen the old appraisal table or invent provenance for old records.

## Cleanup checklist

- [x] Remove Expression RPC/protobuf and scope-free MotionRuntime rejection shells.
- [x] Remove mutable reducer and source-free appraisal compatibility APIs.
- [x] Remove legacy trace reconstruction and completion Boolean fallbacks.
- [x] Require canonical Response identity before expression dispatch.
- [x] Preserve Body Unknown, original ownership, calibrated limits and one writer.
- [x] Update regression fixtures to use BodyProgress and explicit native boundaries.
- Coordinated workspace results are recorded in the [cleanup checklist](../../../docs/refactor/checklists/cleanup-2026-10-06.md).
- [ ] Validate control-board restart, safe physical poses and measured feedback on
  explicitly identified hardware in a later authorized hardware phase.
