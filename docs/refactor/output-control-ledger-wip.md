# Output control ledger — WIP, 2026-09-20

This isolated package is based on `8c3f5450fcedfe250cba7936de6fcdf8076c7040`.
It depends on the concurrent Core output-control contract; that temporary Core
overlay is deliberately excluded from the ledger commit. Do not deploy or merge
this package as a completed feature. Runtime control and transport integration
remain owned by the parent task.

The implementation adds migration `000013` and `delivery::repository::control`
methods on the existing `DeliveryRepository`. A prepared canonical Response
registers one immutable voice scope, Context, server instance and native backend.
The registration deadline is fixed to the earlier of Response creation plus
120 seconds and the observation deadlines of audio deliveries already present.
Registration may precede the first audio reservation. The default hold is
60 seconds and the Core contract caps it at 120 seconds.

New Pause, explicit-wait confirmation and Resume intents pass the original
Context validator before Response, control and Delivery locks. A positively
acknowledged first transaction is the only source of a non-cloneable
`ControlPermit`. Duplicate identity, lookup and recovery yield facts only.
An uncertain commit reports the original binding and command ID; it never grants
a new send. Process recovery retires only expired scopes and does not steal a
different live instance's output.

Two tables retain at most 32 commands and 65 receipt slots: each command's first
and latest receipt plus the scope's latest receipt. Original outcomes are fixed;
only higher observation sequences can advance the current snapshot. A late
unknown command can acquire its original outcome without rewinding that
snapshot. Repeated Observe does not append audit events. Locally retired scopes
stay retired even when later device facts describe an earlier running output.
Media progress in control receipts is evidence only and never changes canonical
Delivery progress, assistant history or PCM dispatch.

Validation completed with the temporary Core-only overlay:

- `cargo check -p alia-server --all-targets --locked`: exit 0.
- `cargo test -p alia-server --test output_control --no-run --locked`: exit 0;
  the independent target contains 9 PostgreSQL tests, none executed yet.
- `git diff --check`: exit 0.

The initial full Core+Proto overlay check failed (exit 101) because Server
transport fixtures and constructors have not adopted the concurrent protocol
fields/methods. No production transport files were changed to hide that mismatch.

Before integration, run the 9 tests against the task's isolated PostgreSQL with
random-schema cleanup. They cover concurrent admission, immutable binding,
hold/capacity bounds, retained receipt semantics, actual SQL authority barriers,
recovery, audit rollback and an actual PostgreSQL COMMIT acknowledgement drop.
The complete command logs, source hashes, overlay files and handoff are preserved
under `run/duplex-control-ledger-handoff/` in this worktree.

Pending review must include audio observation deadlines created after initial
registration (the current admission path does not yet reject a later Delivery
deadline that has expired), and whether a device snapshot's command watermark
must be bounded by the ledger's admitted sequence. The current Context/SQL
locking and revocation tests are compiled assertions, not verified results.
No PostgreSQL schema, task database or container was created or modified by this
package's validation run.
