# Memory Capture And Provider ABI

Alia treats memory as a provider-neutral service. Pipelines, tools, and prompt
assembly talk to `MemoryBackend`; provider details stay inside adapters such as
`mem0`.

Design boundary: Alia decides whether something should become memory; providers
store, search, and report their capabilities. This keeps privacy, sensitivity,
relationship modeling, and prompt-injection defenses in the agent runtime
instead of delegating them to a backend-specific inference loop.

## Capture Modes

- `none`: memory stays available for direct API management, but the assistant
  does not capture conversation memories.
- `explicit_tool_only`: no automatic capture; the assistant may store or forget
  memory only through explicit user-requested tools such as `memory_store` and
  `memory_forget`.
- `llm_selected`: recommended default for a personal assistant. A dedicated
  `memory_capture_llm`, when configured, extracts only durable facts,
  preferences, goals, habits, and relationship context before writing memory.
  If no dedicated model is configured, the minor LLM is used.
- `provider_infer_all`: sends each completed turn to the memory provider for
  provider-native inference. This is opt-in because it gives the provider broad
  authority over what gets remembered.

Recommended personal-agent setup:

```toml
[policies.memory]
enabled = true
capture_mode = "llm_selected"
sensitive_filter_enabled = true
retrieval_layers = ["core", "preference", "relationship"]
max_injected_memory_chars = 1200

[modules]
# Optional. Omit to fall back to minor_llm.
# memory_capture_llm = "ollama"

[providers.mem0]
infer = false
conversation_infer = false
```

Sensitive chat-derived memories are blocked before storage. Credentials,
government or financial identifiers, health details, religious views, and
political views are only accepted through direct user-managed memory entry.

With this setup, mem0 receives selected `MemoryNode`s as single user-message
adds with `infer = false`. Mem0's v3 add API supports one or more messages and
documents `infer = false` as raw storage of the provided text, so this path does
not require sending a full conversation transcript to the provider. Set
`infer = true` or `conversation_infer = true` only for deployments that
intentionally opt into mem0-native extraction; `custom_instructions` then become
provider-side extraction guidance, not Alia's primary memory policy.

## Provider ABI

Every memory backend exposes `memory_capabilities()` with `abi_version = 1`.
Capabilities describe what a backend can actually do, without leaking concrete
provider APIs into the rest of the server:

- store/list/get/retrieve/update/delete/clear-user
- conversation ingest and provider-native inference
- metadata support
- search filters, layer filters, and similarity threshold support
- graph and relationship edge support
- async write behavior and size limits

Runtime clients can inspect `/api/v1/memories/capabilities` to adapt UI and
plugin behavior. For example, a backend without graph support should hide graph
relationship views, while a backend without provider inference should not offer
`provider_infer_all`.

Runtime clients can inspect `/api/v1/memories/privacy` for available capture
modes and can use `/api/v1/memories/audit` to show the user's current memory
controls, provider capabilities, and stored memories in one backend-facing
response. The API rejects `provider_infer_all` when the active backend does not
declare provider-native inference support.

Future memory plugins should implement the same `MemoryBackend` contract and
return accurate capabilities. Plugin-specific details belong in
`provider_specific`, not in pipeline or prompt code.

Alia-side scheduled memory prompts are configured under
`[policies.insights]`: `memory_extraction_prompt`,
`memory_consolidation_prompt`, `reflection_prompt`, and
`habit_detection_prompt`. Keep these prompts in English because the tasks expect
structured JSON and are regression-tested as internal system jobs.
