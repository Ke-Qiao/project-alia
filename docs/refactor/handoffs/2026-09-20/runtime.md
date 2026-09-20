> 归档副本。原工作树：`/Users/ke-qiao/Developer/project-alia/run/worktrees/alia-duplex-runtime`；固定提交：`e94f66eb6968706053cceab7c9d371c24f37e116`。文内相对证据路径相对于原工作树的交接目录，未改写原始交接内容。

# Server duplex runtime checkpoint — 2026-09-20

User explicitly requested pausing for quota. Do not keep working until the user resumes.

Branch `codex/duplex-runtime`, WIP commit `e94f66e`, base `b995098`. Main remains separately verified. This is **not buildable standalone and has not been compiled or runtime tested**. `rustfmt --edition 2024 --config skip_children=true` for all six owned files and `git diff --check` succeeded; these are syntax/format checks only.

## Owned draft

- `crates/alia-core/src/turn.rs`: add `accept_evidence` to consume fresh model evidence without implicitly advancing generation; old `observe` behavior preserved.
- `channels/adapter.rs`: typed `register_output`, `control_output`, `observe_output_control` methods (default unsupported).
- `channels/embedded/channel.rs`: bounded local canonical registration map, typed gRPC methods, old-scope map removal on fences; Start requires registered binding. Watch late Register/Fence and replacement races during review.
- `channels/embedded/playback.rs`: Push/produce accept OutputBinding and put it in playback v3 Start.
- `modules/delivery/service/output.rs`: new draft local gate/producer/media owner and serialized durable control actor; **not declared/wired from service.rs**, references fields that do not yet exist. Intent: continuous capture must not await these SQL/RPC operations; original media futures remain owned; no replay permit on unknown ACK.
- `modules/delivery/service/output/gate.rs`: deterministic temporary/explicit hold policy and three authored, **unexecuted** unit tests.

## Dependencies and saved overlays

Core/Proto/Cargo.lock snapshots from agent A were copied solely for development. They are not part of `e94f66e`; exact copies and hashes are in `shared-overlay/` and `RESULT.json`. The snapshot predates A final `TurnEvidence.output_decision` addition. Do not publish or use it as the latest A implementation. Integrate A final commit first, not the old copies. Agent B owns repository::control and migration 000013; C owns typed AudioBuffer/ResponseAudioBudget and TTS callers.

## First resumption tasks

1. Read the main `docs/refactor/resume-checkpoint-2026-09-20.md` and all three agent handoffs; inspect Git status before any merge. Preserve exact WIP and evidence.
2. Integrate A/B/C in an isolated integration worktree. A owns Core/Proto/Embedded; root only owns Core turn.rs. B temporary Core overlays must not be committed. C and root overlap Server channels/adapter.rs, embedded/playback.rs and possibly channel.rs: merge relevant hunks, never replace entire files.
3. Wire `mod output`, exports, `outputs`, `output_sender` and receiver into DeliveryService start/run/shutdown and the existing JoinSet ownership. Add `anyhow::Context` where required; the new output.rs has not undergone type checking.
4. Add original request_id and optional production ownership to RecognizedVoiceInput/VoiceTurn. Mint at live voice admission, retain through queue/LLM/TTS and scope native tail; call `bind` only after canonical Response prepare, before first TTS/Start. Hold the same ResponseAudioBudget across all sentences/media owners. Cover errors/drop of queued input.
5. In VoiceSessionService pass the actual DeliveryService and configurable hold budget (default 60000 ms, hard 120000 ms); v2 requires paired software_hold capability. Update Open/Ready/fixtures to v2. Consume fresh evidence before deciding generation. Suspected speech => local gate + async Pause; Wait => confirm explicit; fresh Backchannel only resumes AcousticCandidate; exact explicit Continue resumes ExplicitWait; true new turn/Stop => cancel + generation change. All commands run outside capture/auth actor blocking path. No model label alone opens a device gate.
6. Wire every accepted/drained capture frame to local freshness tracking; device A has the authoritative same-lock before-enqueue barrier. Same-epoch evidence carries PAUSE_REQUESTED/RESUME_REQUESTED, not an Applied claim; only Control can alter the gate. Add new live-model expectations, replacing old WAIT-cancels semantics only with full paired protocol.
7. Connect gate to TTS and audio stream production without dropping/restarting futures or holding PG transactions while paused. Register media ownership before waiting so native tails survive producer completion. Complete initial pending-producer path and original Context admission.
8. Review unfinished actor carefully: unregistered Bind currently awaits DB/RPC without the same bounded/cancellable select; local deadline extensions after semantic Confirm must wake/recompute the *temporary semantic* timer but never extend original hard deadline; unknown Control outcomes currently try Observe once then retire (conservative, no replay) and need explicit integration tests. Late facts must still be recorded after local retirement. Avoid invoking stale generation fences on replacements.
9. Validate complete old Context/root/epoch gates on Resume, backend identity and SQL-first intent, registration failure/unknown commit, new speech vs in-flight Resume, capacity/exact original deadlines, no double Response/history/TTS, caller drop and graceful shutdown, queue full control priority, expiry/clear/cancel races. Use actual isolated PostgreSQL. No claims of physical silence or Presented/AEC.
10. Only after coherent default/no-default compilation, targeted regressions and full workspace validation should this work be merged into main. No push is authorized/requested.

## Resources at pause

No root build/test exec remains running. No root Docker container was created in this subpackage. Shared task PG, model service, Cargo caches and all worktrees are deliberately retained. Do not touch user alia-postgres:5433.
