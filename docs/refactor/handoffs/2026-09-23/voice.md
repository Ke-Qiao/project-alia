# Voice resume handoff — 2026-09-23

Owned Rust source is stable and rustfmt/diff checks passed. Root owns integrated freeze, normal commit and final macOS/workspace/platform verification. This agent made no Git commit and currently owns no active exec, PostgreSQL work or model service.

## Current scoped outcome

- Actor 23/23 real execution, 0 failed/ignored, 3.26s: `actor-real-clock.log`. Four cases are pure/SessionLease; 19 use independent real PG schemas with normal migrations and Delivery start/recovery. These actor cases intentionally stop before canonical Response binding.
- Config 4/4 execution: `config.log`.
- Live 1/1 real Easy-Turn + gRPC + canonical PG source/Context/Response + exact control ledger execution, 0 failed/ignored, 1.85s: `live-repeat.log`. Model Complete/Backchannel/Wait 590.66/400.14/453.08ms. Four Applied control receipts preserve original binding/backend, WAIT retains original deadline, negotiated 30s caps configured 60s.
- 28 distinct scoped tests passed; do not add repeated attempts or direct HTTP diagnostics to that count. Root must validate all final integrated source together.
- First actor run 20 pass/3 fail is retained in `actor-run.log`. The three failures were virtual Tokio clock advancement across real periodic recovery SQL deadlines, observed during shutdown. Tests now use 100ms real revalidation intervals; production remains fixed 30s. No recovery bypass, no paused database runtime.
- First live run 0 pass/1 fail is retained in `live-run.log`: initial Complete exceeded the unchanged 8000ms fixture model deadline before any Response binding or pause assertion. Independent same-PCM HTTP diagnostics returned correct labels in 363–501ms; a subsequent unchanged-budget live run passed. Cold cache/load is only an inference, not a proven diagnosis.

## Changes since restored snapshot ae75a4e

Only `voice_session.rs`, `voice_stream_live.rs`, and voice checklist changed. Bootstrap/config/config tests remain byte-identical to the snapshot.

- Fixed fixture Core binding/registration validation and actual delivery_responses table.
- Wrapped actor PG fixtures with catch-unwind, task cancellation and normal Delivery shutdown before exact schema cleanup.
- Added capture-during-credential-revalidation regression; stale Continue must not release the original hold.
- Private run_session interval argument permits real short-cycle fixture scheduling. Production entry always passes AUTH_RECHECK=30s. Renamed credential test to repeated periodic revalidation; no claim of actual five-minute duration.
- Implemented exact OutputRetirementRequest/Receipt for both fixtures. Actor only admits known unregistered scopes with NoNativeOwner. Live validates original binding/backend and retires its software owner before Settled; empty progress means no native write/PCM was started in this fixture.
- Strengthened live real ledger identity, receipts, intent digest and budget checks.

## Exact evidence and cleanup

`RESULT.json` contains owned source SHA-256, commands, exit statuses, logs and explicit verification limits. Checklist: `docs/refactor/checklists/duplex-voice.md`. Owned five Rust files passed final `rustfmt --check --edition 2024`; `git diff --check` passed. All source remains stable for Root freeze.

`schemas-after-live-final.txt` exactly matches `schemas-before.txt` (the three historical pipeline_delivery schemas). `clients-after-live-final.txt` is 0. PG window passed to C, whose later checks are Root-managed. No services were started/stopped by this agent.

This is software fixture validation: no physical microphone/VAD, LLM/TTS, speaker presentation, Pi native playback or AEC claim. Easy-Turn revisions remain pinned; CPU float32 service at 8091 was ready/model_loaded.

## Root-requested follow-up after final workspace/no-default

Do not start before Root releases the final frozen source and PG window. Use shared target and `--offline --locked`, normal native dependencies, no DOCS_RS/RUST_MIN_STACK bypass.

1. True PTY (stdout and stdin must remain attached to the PTY; do not redirect cargo output):
   `CARGO_TARGET_DIR=/Users/ke-qiao/Developer/project-alia/target cargo test -p alia-server --lib --offline --locked -- --ignored --nocapture --test-threads=1`
2. Two model targets, official files and real task PG, with loopback permissions:
   `ALIA_TEST_DATABASE_URL=postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_delivery_final ALIA_EASY_TURN_PCM=/Users/ke-qiao/Developer/project-alia/run/easy-turn-stream-pcm/backchannel_real_1.pcm ALIA_EASY_TURN_PCM_DIR=/Users/ke-qiao/Developer/project-alia/run/easy-turn-stream-pcm ALIA_EASY_TURN_URL=http://127.0.0.1:8091 CARGO_TARGET_DIR=/Users/ke-qiao/Developer/project-alia/target cargo test -p alia-server --test easy_turn_live --test voice_stream_live --offline --locked -- --ignored --nocapture`

Capture tool-returned PTY output to an evidence file after execution rather than shell-redirection inside the PTY. Snapshot the final Root build-input manifest before/after these runs, and verify exact schema baseline and client cleanup again.

## Completed V2 frozen-source follow-up

The above follow-up executed after Root and B released the window. Manifest `run/validation-20260923/final-source-manifest-v2.json`, SHA-256 `1939a948191002882ba70e2578ad87d266d428b2ba423ccfa31aa178a34998d4`, had all 605 build inputs unchanged before execution, after PTY, and after models.

- True PTY: 3 passed, 0 failed/ignored, 0.34s. Cargo stdout was not redirected; tool-returned terminal output was saved afterward. Evidence: `run/validation-20260923/final-pty-model/pty-v2.log` and `pty-v2-result.json`.
- Real model targets: Easy-Turn 1 passed in 0.59s, then VoiceStream 1 passed in 1.71s; both 0 failed/ignored. Original fixture model budgets remain 30000ms and 8000ms. Evidence: `run/validation-20260923/final-models-v2.log` and `final-models-v2-result.json`.
- No additional diagnostic/prewarm request preceded these tests. First tested call was Easy-Turn Backchannel, 586.044ms inference; subsequent VoiceStream Complete/Backchannel/WAIT were 545.904/344.788/430.174ms. The service was not restarted and caches were not reset: this is ordered real-model evidence, not a cold-start claim. Earlier 8s timeout remains recorded.
- Task PG schema before/after matched exactly: original three historical pipeline schemas plus system/public, other clients 0. No explicit cleanup beyond fixture-owned cleanup was needed. Window released directly to `lifecycle_review` for Linux V2; Root notified. No active owned exec/PG work remains.

Final supplement with precise commands, hashes and resource state: `run/validation-20260923/final-pty-model/RESULT.json`. The 23 actor scoped evidence is preserved as its original run; this supplement does not relabel it as a V2 rerun or inflate unique test totals.

Root separately requested documentation updates in `docs/refactor/voice-protocol.md`, `docs/operations/local-development.md`, and `docs/operations/model-services.md`. These now describe paired voice v2 / playback v3, the two holds and unchanged 3s/60s/120s engineering limits, original-owner control/retirement and software versus hardware boundaries. Push ACK receipt sequence is positive/monotonic and may start at 2 or higher after independent Control/Observe; playback identity sequence starts at 1 and is separate. Documentation is excluded from the 605 frozen build inputs.
