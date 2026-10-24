# Skills and Tools

Skills and tools have different jobs. Keeping them separate makes routing,
prompt assembly, plugin ownership, and frontend inspection much easier.

## Boundary

| Concept | Role | Example |
| --- | --- | --- |
| Tool | Atomic execution unit. It has a name, description, JSON parameters, ownership, optional confirmation, and Rust/plugin executor code. | `fetch`, `get_current_time`, `sensor_read`, `memory_search` |
| Skill | Cognitive context. It helps route intent, contributes a domain prompt, and declares which tools are relevant. | smart home control, web research, memory-aware conversation |

A skill should not behave like a giant function call. It should prepare the LLM
context and available tools. The LLM then decides whether to call tools.

## Runtime Flow

1. Authorized user input enters Interaction.
2. Interaction routing and the skill registry decide whether a skill context is relevant.
3. The skill contributes a prompt template and a bounded set of tool names.
4. The tool registry resolves those names into tool definitions.
5. The LLM receives the prompt plus tool definitions.
6. Rig AgentRun processes native function/tool calls. Text resembling a tool
   invocation remains text; there is no text-based execution fallback.
7. Execution validates identity, policy, implementation and approval, records
   the effect, and returns its actual result or uncertainty to the LLM loop.
8. Final assistant text is delivered and persisted.

## Tool Registration

Every tool should be registered into the database catalog, including tools from:

- built-in Rust code
- server components
- embedded channel actions
- MCP services and WASM plugins

Code-backed tools still need database metadata so routing, frontend display,
permissions, confirmation policy, and dependency tracking can work consistently.

Registration and Skill selection do not authorize an effect. Package trust,
runtime capability grants and per-effect approval are independent, as defined
in the [plugin contract](../architecture/plugin-contract.md). External
tools must use the same Execution authority; tool descriptions and responses
remain source-tagged data.

Rust tool authors implement Rig `Tool` with one `Deserialize + JsonSchema`
argument type and register a `ToolEntry` with an implementation revision and
host policy. Dynamic schemas use the same bounded local validator. See
[MCP configuration](../operations/mcp-tools.md) and
[WASM packages](../operations/wasm-plugins.md) for external registrations.

## Public Fetch

`fetch` accepts `url`, optional `max_bytes` (default 65536, at most 1048576),
and optional `timeout_sec` (default 10, at most 30). It accepts public HTTP(S)
destinations and validates and pins DNS addresses for every redirect. URL
credentials, arbitrary headers, private networks, proxies and HTTPS downgrade
redirects are rejected. Existing callers must remove the old `headers` argument.

UTF-8 HTML becomes Markdown with relative links resolved against the final URL;
scripts, styles and embedded objects are removed. Text and JSON remain text.
The result includes the final URL, HTTP status, byte count, format, truncation
flag, and `untrusted: true`. Download limits do not enlarge Execution's 64 KiB
receipt limit: text may be shortened further to fit the full serialized receipt,
including JSON escaping. Unsupported binary/compressed responses fail explicitly.

## Skill Registration

Every skill should also be registered into the database catalog. Skills can come
from built-in definitions, resource files, APIs, or plugins. A skill declares
its tool bindings through an explicit relationship rather than a loose list
inside opaque metadata.

## Dependency Ownership

The registry tracks which owner created each artifact and what depends on what.
When a plugin or component is removed:

- artifacts with no remaining dependency can be deleted
- artifacts that are still depended on should block removal with a clear error
- shared tools should not be deleted just because one owner disappears

This dependency tree should be shared across built-in code and plugins.

## Tool Candidate Selection

The tool list can grow too large for the context window. The current direction
is layered filtering:

- database `routing_keywords` and `routing_text` provide the lexical candidate
  pool
- embeddings persisted in `routing_embedding` can narrow candidates quickly
- a lightweight minor LLM can filter the candidate pool before major LLM use
- insights can improve catalog keywords, examples, and routing text during idle
  time

The final LLM prompt should see only the tools that are likely relevant to the
current turn.
