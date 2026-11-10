# OpenAPI coverage

Updated 2026-11-11. One operation is one explicitly registered HTTP method and path. The table matches [router mounts](../src/api/mod.rs) and handler `#[utoipa::path]` declarations registered in [`ApiDoc.paths`](../src/openapi.rs). It excludes Axum's implicit HEAD handling and routes outside `src/api/`, such as WebSocket, Scalar, and setup routes. The health check is mounted at `/health`; the other API modules are mounted under `/api/v1`.

47 of 143 explicitly registered HTTP operations are declared, leaving 96 for later batches. All eight planned additions are included: logout, `PATCH /api/v1/users/me`, and the six conversation operations. Expression traces required two schemas outside `api/`, within the five-type limit.

| API file | Declared operations | Undeclared methods and paths | Notes |
| --- | ---: | --- | --- |
| `auth.rs` | 4 | — | All registered operations declared. |
| `channels.rs` | 0 | `GET /api/v1/channels`<br>`POST /api/v1/channels/{channel_id}/restart` | Pending later contract batches. |
| `companion.rs` | 4 | — | All registered operations declared. |
| `conversations.rs` | 6 | — | All six operations added; traces require only two schemas outside api/. Payloads remain arbitrary JSON. |
| `deliveries.rs` | 6 | — | All registered operations declared. |
| `devices.rs` | 0 | `GET /api/v1/device`<br>`GET /api/v1/device/status`<br>`POST /api/v1/device/bind`<br>`POST /api/v1/device/unbind`<br>`GET /api/v1/device/settings`<br>`PATCH /api/v1/device/settings`<br>`GET /api/v1/device/settings/direct-responses`<br>`POST /api/v1/device/settings/direct-responses`<br>`DELETE /api/v1/device/settings/direct-responses/{id}` | Pending later contract batches. |
| `effects.rs` | 4 | — | All registered operations declared. |
| `embodiment.rs` | 2 | — | All registered operations declared. |
| `expression.rs` | 0 | `GET /api/v1/expression/relationship`<br>`DELETE /api/v1/expression/relationship`<br>`POST /api/v1/expression/relationship/prune`<br>`POST /api/v1/expression/relationship/rebuild` | Pending later contract batches. |
| `health.rs` | 1 | — | `GET /health` is mounted and declared at the root. |
| `insights.rs` | 0 | `GET /api/v1/insights/status`<br>`POST /api/v1/insights/toggle`<br>`GET /api/v1/insights/logs`<br>`GET /api/v1/insights/reflections`<br>`POST /api/v1/insights/trigger`<br>`GET /api/v1/insights/conditions`<br>`PUT /api/v1/insights/conditions` | Pending later contract batches. |
| `memories.rs` | 0 | `GET /api/v1/memories`<br>`POST /api/v1/memories`<br>`DELETE /api/v1/memories`<br>`GET /api/v1/memories/privacy`<br>`PATCH /api/v1/memories/privacy`<br>`GET /api/v1/memories/capabilities`<br>`GET /api/v1/memories/audit`<br>`GET /api/v1/memories/export`<br>`POST /api/v1/memories/consolidate`<br>`POST /api/v1/memories/search/context`<br>`DELETE /api/v1/memories/hidden`<br>`DELETE /api/v1/memories/topic/{topic}`<br>`DELETE /api/v1/memories/kind/{kind}`<br>`GET /api/v1/memories/graph`<br>`POST /api/v1/memories/search`<br>`GET /api/v1/memories/{id}/explain`<br>`GET /api/v1/memories/{id}/suppressed`<br>`GET /api/v1/memories/{id}`<br>`PATCH /api/v1/memories/{id}`<br>`DELETE /api/v1/memories/{id}` | Pending later contract batches. |
| `mod.rs` | 0 | — | Router composition only; mounts API routes under `/api/v1`. |
| `motion_deliveries.rs` | 2 | — | All registered operations declared. |
| `personas.rs` | 0 | `GET /api/v1/personas`<br>`POST /api/v1/personas`<br>`GET /api/v1/personas/me`<br>`PATCH /api/v1/personas/me`<br>`GET /api/v1/personas/{id}`<br>`PUT /api/v1/personas/{id}`<br>`DELETE /api/v1/personas/{id}` | Pending later contract batches. |
| `plugins.rs` | 0 | `GET /api/v1/plugins`<br>`GET /api/v1/plugins/{name}/storage`<br>`DELETE /api/v1/plugins/{name}/storage`<br>`GET /api/v1/plugins/{name}/storage/export`<br>`GET /api/v1/plugins/{name}/storage/me`<br>`DELETE /api/v1/plugins/{name}/storage/me`<br>`GET /api/v1/plugins/{name}/storage/me/export` | Pending later contract batches. |
| `rules.rs` | 0 | `GET /api/v1/rules`<br>`POST /api/v1/rules`<br>`GET /api/v1/rules/{id}`<br>`PUT /api/v1/rules/{id}`<br>`DELETE /api/v1/rules/{id}`<br>`POST /api/v1/rules/{id}/pause`<br>`POST /api/v1/rules/{id}/resume` | Pending later contract batches. |
| `scheduler.rs` | 0 | `GET /api/v1/scheduler/tasks`<br>`POST /api/v1/scheduler/tasks`<br>`GET /api/v1/scheduler/tasks/{id}`<br>`PUT /api/v1/scheduler/tasks/{id}`<br>`DELETE /api/v1/scheduler/tasks/{id}`<br>`POST /api/v1/scheduler/tasks/{id}/pause`<br>`POST /api/v1/scheduler/tasks/{id}/resume`<br>`GET /api/v1/scheduler/subscriptions`<br>`POST /api/v1/scheduler/subscriptions`<br>`DELETE /api/v1/scheduler/subscriptions/{id}` | Pending later contract batches. |
| `sensors.rs` | 8 | — | Existing declared operations still need a later audit of operation-specific errors. |
| `settings.rs` | 0 | `GET /api/v1/settings`<br>`PATCH /api/v1/settings` | Pending later contract batches. |
| `skills.rs` | 2 | `POST /api/v1/skills/match`<br>`POST /api/v1/skills`<br>`PUT /api/v1/skills/{name}`<br>`DELETE /api/v1/skills/{name}` | Existing declared operations still need a later audit of operation-specific errors. |
| `tools.rs` | 1 | `GET /api/v1/tools/mcp/services`<br>`POST /api/v1/tools/mcp/services/{id}/restart`<br>`POST /api/v1/tools/mcp/services/{id}/stop`<br>`GET /api/v1/tools`<br>`GET /api/v1/tools/openai`<br>`GET /api/v1/tools/{name}` | Pending later contract batches. |
| `users.rs` | 6 | — | Existing declared operations still need a later audit of operation-specific errors. |
| `voice.rs` | 1 | `POST /api/v1/voice/chat`<br>`POST /api/v1/voice/chat/{channel_id}`<br>`GET /api/v1/voice/capabilities`<br>`POST /api/v1/voice/transcribe`<br>`POST /api/v1/voice/synthesize` | Pending later contract batches. |
| `wasm_plugins.rs` | 0 | `GET /api/v1/plugins/wasm`<br>`POST /api/v1/plugins/wasm`<br>`PUT /api/v1/plugins/wasm/{name}`<br>`DELETE /api/v1/plugins/wasm/{name}`<br>`POST /api/v1/plugins/wasm/{name}/enable`<br>`POST /api/v1/plugins/wasm/{name}/revoke` | Pending later contract batches. |

## Behavior recorded while adding contracts

- Conversation details, deletion, messages, events and expression traces require ownership or an administrator role. Missing conversations return 404; another user's existing conversation returns 403.
- Conversation query limits accept zero. Lists default to 100 and clamp to 500; messages/events/traces default to 1000 and clamp to 5000. Trace limits count source events, and the returned traces are grouped and ordered by turn UUID.
- In conversation operations, malformed path UUIDs and query values are rejected by Axum with plain-text 400 responses before the handler runs; they do not use `ErrorResponse`.
- Event payloads and expression diagnostics can carry any JSON value, including arrays and scalars. Their schemas do not require an object.
- Logout does not require bearer authentication; unknown or already revoked refresh tokens still return success. PATCH users/me reuses the user update rules, including administrator-only role/status updates.
- The shared authentication extractor returns 401 for credential/account failures, 503 when account verification times out, and 500 on database errors. A route's presence in this table does not certify that every response status has been documented.
- [`StandardAuthErrors`](../src/openapi.rs) fills shared 401 and 503 details on inline responses. It skips a `$ref` for the entire response; none of the current operations use a response-level `$ref`. References inside response body schemas are supported.
- Execution, delivery, and companion receipts retain their actual domain error types and retry identity fields. Motion errors reuse the matching delivery error schema without changing their wire values.

For behaviors that need a design decision before changing, see [API design follow-ups](api-design-follow-ups.md).
