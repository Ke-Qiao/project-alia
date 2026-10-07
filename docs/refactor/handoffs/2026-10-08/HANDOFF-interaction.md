# Interaction handoff — 2026-10-08

## Scope and ownership

User authorized clean directory/dependency refactoring from the current `codex/architecture-refactor` baseline (`adb28c1`). This subagent owned the former `channels/pipeline.rs` and its implementation/test children, excluding `memory_capture.rs` (owned by Memory). Root owns all Cargo/PostgreSQL validation, external consumers, evidence and commits. No Cargo, database, service, network, hardware or Git write operations were performed by this subagent.

## Checklist and implementation progress

Implementation steps: **7/7 complete**. Root acceptance is complete; these numbers do not imply the broader architecture refactor is complete.

- [x] Trace text and recognized-voice ingress, bounded request admission, Context materialization, cache, route/model/tool execution, Delivery, response persistence and memory capture.
- [x] Delete the `channels::pipeline` namespace and move interaction use cases into `modules::interaction`; no compatibility module or alias remains.
- [x] Replace use-case `ModuleRegistry` dependencies with the concrete `InteractionServices` composition containing the 17 actual required services. Bootstrap constructs this explicitly. Interaction cannot access STT, scheduler, runtime cleanup, companion or extension registry through this composition.
- [x] Extract policy, context, prompt, cache, persistence, expression preparation, LLM execution and bounded runtime responsibilities. `interaction/mod.rs` contains declarations and the public runtime/services/policy exports; production child modules use explicit imports.
- [x] Replace the single large `process_text` body with ordered preparation, cache lookup, route execution and completion phases. Exact and semantic cache answers share one completion implementation; skill, minor, escalated-major and major routes share model/tool-selection/sentence-context execution.
- [x] Migrate existing integration tests to Interaction and adapt their fixture to the concrete service composition. Remove the old STT mock/registry field: STT is no longer a dependency accessible to the production use case. Move fixture-only deterministic IDs out of production Context code.
- [x] Integrate Memory agent's actual PostgreSQL exact/semantic cache regression via `integration_tests/cache.rs`; add its module declaration. This exercises real DbCache, canonical Response/Delivery/history, original-source Context lineage, zero LLM calls on a hit, and rejection after source deletion.
- [x] Root: default/no-default compile checks and appropriate regression matrix.
- [x] Root: formatting/architecture-boundary checks, source freeze/evidence, normal commits and final checkpoint.

## Resulting responsibility map

- `runtime.rs`: reliable voice receiver, best-effort ChannelEvent queue, bounded task supervision, text/sensor/status dispatch. Its owner still joins all request tasks before shutting down dependencies.
- `generation.rs`: `process_text` sequences `prepare_turn → lookup_cache → route_response → finish_generation / complete_answer`; internal request/prepared/result records carry only state shared by these actual phases.
- `context.rs` and `turn.rs`: original source admission, Context materialization/validation/cache evidence merge, frozen lease state, cancellation, generation/session identity and pause-aware waiting.
- `cache.rs`: safe-direct eligibility, semantic candidate selection/reference filtering and cache-write policy.
- `prompt.rs`: model message assembly and runtime/Memory prompt rendering.
- `llm.rs` and `tools.rs`: model response modes and actual tool loop/selection/protocol logic. Tool side effects continue through the existing Execution services.
- `expression.rs`, `dialogue_policy.rs`, `streaming.rs`: interpretation/projection, response/sentence planning, bounded speech generation and owned output coordination.
- `delivery.rs`: turn-bound use-case calls into the canonical Delivery service. It is not a replacement Delivery repository and owns no direct device I/O.
- `persistence.rs`: response commit, cache write projection and explicit invocation of `modules::memory::capture` with `CaptureTurn`; Memory owns capture policy/write implementation.
- `services.rs`: explicit production service composition. `policy.rs`: runtime policy snapshot/update logic.

The original `channels/pipeline.rs` was 3,424 lines. `interaction/mod.rs` is about 31 lines and `process_text` about 79 lines including signature/request assembly. Generation's total phase implementation remains substantial; file/line reduction is not claimed as evidence that every module or provider has reached its final architecture.

## Behavior and evidence preserved

- The outer generation future remains heap allocated (`BoxFuture`); route execution is also boxed. Existing cancellation wrappers continue to check cancellation before polling and wait across output holds.
- Text and voice retain their original request, account, input source timestamps/privacy epochs, original pinned Channel instance, response identity and generation scope.
- Canonical Context must materialize before model/tools; cache reuse merges its exact immutable evidence before response preparation. Preparing a response freezes that Context even if remote acknowledgement is unknown.
- Exact and semantic hit completion still records the original trace details, including semantic similarity/kind, prepares a unique Response, optionally persists, then plans expression from the current frozen Context. Cached text is not synthesized/generated twice.
- Minor escalation still disables minor streaming when an escalation prompt exists; skill preferred tools remain ranking hints, not restrictions.
- Interrupted tool requests still drain dispatched side effects and preserve user-only context; no abort-all shortcut was introduced.
- Response commit failure still suppresses memory capture and `ConversationTurnCompleted` publication. Source provenance and cache admission checks remain.
- Temporary hold, explicit pause, exact cancellation, Ready/original output binding, physical evidence and default Shadow policy are unchanged.
- Existing persisted audit actor/event strings containing `pipeline` are retained as record semantics. Removed Rust type/function names no longer require the old namespace.

## External API changes

- Runtime/public composition: `modules::interaction::{spawn_interaction, InteractionServices, InteractionPolicies, SharedInteractionPolicies}`.
- Testable helpers: `interaction::{cache,prompt,persistence,llm,streaming,tools}`.
- `PendingTextPersistence` and `cache_entry_for_pending`: `interaction::persistence`.
- `CacheWrite`, `CacheContext`, cache eligibility/write helpers: `interaction::cache`.
- `RuntimePromptContext` and rendering: `interaction::prompt`.
- `escalation_reason`: `interaction::llm`.
- Internal error/task/input types are `InteractionError*`, `InteractionTasks`, `InteractionInput`; tool invocation helper is `execute_turn_tool_call`.
- Memory capture uses explicit `&ContextService`, `&dyn LlmAdapter`, `&MemoryService`, `&CaptureTurn`, `&MemorySection`; `CaptureTurn.session_id` is `Uuid`.

## Validation status at handoff

Subagent only ran Rustfmt syntax/format checks on its owned Rust files. Root's first library preflight found stale Memory imports and three unnecessary mutable trace bindings; these were fixed. Root's all-target check was still running when this handoff was written. No test pass counts are claimed here before root confirmation. The cache regression was implemented by the Memory agent and has not been independently executed by this subagent.

## Deferred and next steps

This round does not migrate every remaining provider (conversation/cache/persona/skills still expose their existing contracts), redesign every expression or streaming algorithm, add interfaces for hypothetical deployments, or add a new process/dependency/platform. Physical AEC, Pi playback/motion acceptance, frontend and broader platform validation retain their existing deferred status. Next action is L09 fixed-core ownership; see the final root checkpoint.

## Cancellation audit correction after initial handoff

Independent review found that the new post-cache cancellation guard returned before recording the already-admitted user source's terminal interruption. The prior exact-only cache path reached the router cancellation branch and therefore retained this audit. This was an accidental refactor regression, not a policy change.

Root first ran the new deterministic PostgreSQL cancellation test against the unfixed guard: **0 passed / 1 failed**, at the expected missing-terminal-audit assertion (`0` versus `1`). The test blocks the selected exact or semantic cache lookup until cancellation and checks the saved original user source, no assistant/Response/model/output, and the original interruption stage.

The shared cache cancellation exit now invokes `persist_interrupted_input_context` and records the normal `turn_interrupted` trace before returning. Existing `cache_lookup` or `semantic_cache_candidates` stages are retained; `after_cache_lookup` is set only when cancellation happened during a cache trace await and no stage was yet recorded. Source identity and receive time remain unchanged. This also makes semantic lookup cancellation obey the same terminal-audit invariant. Root verified the repaired final source; actual counts and scope are recorded in the final acceptance below.

## Root 已确认的整合结果

源码 `978af9c`（实现 `08c54be` 加测试夹具修正）、最终 V4 输入 630 项已逐项对应提交。macOS 默认工作区 **1053 passed / 0 failed / 5 ignored**；真实 PTY **3 passed / 0 failed**；Server 无默认 feature all-targets、格式与架构边界自测/扫描均通过。两项新增缓存回归已包含在默认通过计数中。Linux 最终源码验证已完成，详见下方 Root 最终验收。此前 subagent 的未验证状态仅表示当时分工交接时点。

## Root 最终验收

本轮 **8/8**。最终源码 `978af9c942da42aa3cd98182cde3dafa6a6e33be`，V4 `ccfaa7981fc7062dfeb43e568e52af038915064a2187352e0891980afc6fcbd3`，630/630 输入匹配。macOS workspace **1053/0/5**、真实 PTY **3/0**、Server 无默认全部目标通过；Linux ARM64 Server 全包 **771/0/5**、Server 无默认全部目标和默认 release/native link 通过。架构守卫、自测、格式与差异检查通过。

实际命令与逐目标计数见 [最终验证表](../../layering-2026-10-08.md#最终验证与证据)、[证据索引](evidence-index.json)。两项真实模型未重跑；不声称 Linux 全工作区或硬件验收。V3 Linux 原失败与定向复核保持原样，V4 仅修测试夹具。三个 subagent 已交回；两个 Linux holder 和独立 PG 已停止，用户 5433 未访问。没有待接回的 Cargo/PG 作业。下一步从 [L09（0/4）](../../checklists/layering-next.md) 开始。
