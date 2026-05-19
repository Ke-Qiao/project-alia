# Memory Capture And Provider ABI

Alia treats memory as a provider-neutral service. Pipelines, tools, and prompt
assembly talk to `MemoryBackend`; provider details stay inside adapters such as
`mem0`.

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
```

Sensitive chat-derived memories are blocked before storage. Credentials,
government or financial identifiers, health details, religious views, and
political views are only accepted through direct user-managed memory entry.

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

Future memory plugins should implement the same `MemoryBackend` contract and
return accurate capabilities. Plugin-specific details belong in
`provider_specific`, not in pipeline or prompt code.
