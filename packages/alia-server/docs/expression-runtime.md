# Expression Runtime

`alia-server` includes an expression runtime for making assistant responses
feel more embodied without letting the LLM directly control hardware.

## v0.5 Scope

The current implementation separates short-term affect from slower social
relationship state, regulates internal state into a safe expressed stance, and
adds Rust extension points for future plugins/components:

- It observes user input at the start of a turn and appraises it into typed
  events such as `user_gratitude`, `user_frustration`, or `user_question`.
- User-input appraisal now runs through `AppraisalEnginePlugin`: the built-in
  rule engine remains the first pass, a built-in local classifier contributes
  sarcasm/support/safety/minimal-expression signals from language-scoped
  lexicons, and Rust components can register async classifier- or LLM-backed
  engines that return additional `AppraisalEvent`s.
- It observes the final assistant response and route outcome after generation,
  then appraises runtime events such as fallback responses, tool-assisted
  turns, cached turns, and long responses.
- It keeps short-term per `channel_id + user_id` appraisal events with
  confidence, scope, TTL, decay, deltas, and evidence.
- It also keeps a per `user_id` relationship stack for slower social state:
  `trust` and `familiarity`.
- It persists appraisal events to `expression_appraisal_events` and restores
  unexpired events at startup.
- It reduces non-expired weighted events into an `affect` snapshot
  (`valence`, `arousal`, `calmness`), `social` snapshot (`trust`,
  `familiarity`, `engagement`), and `resource` snapshot (`fatigue`,
  `battery_stress`, `thermal_stress`, `sleepiness`, `interaction_load`).
- It converts the internal snapshot into an `ExpressiveStance`, capping
  negative expression, high arousal, low-trust animation, fatigue, and apology
  intensity before planning speech or motion.
- It plans expression in two stages: an Intent Planner first chooses the
  device-independent `ExpressionIntent`, then a Realizer turns that intent into
  speech metadata and optional timeline actions.
- `ExpressionInput` includes explicit `dialogue_act`, `content_tone`,
  `conversation_context`, `constraints`, `user_state`, `device_context`, and
  `safety_context`, so expression is driven by dialogue policy and runtime
  constraints rather than response-text heuristics or affect alone.
- It adjusts style, intensity, motion scale, and prosody from the regulated
  stance rather than directly from raw internal state.
- It lets Rust components register affect reducers, expression planners, and
  expression safety policies through `ModuleContext.expression`.
- It records safety-policy mutations in `ExpressionPlan.safety_modifications`.
- It negotiates channel expression capabilities before planning motion, so the
  runtime does not assume a body supports `small_nod`, `look_at_user`, or other
  actions just because the channel has hardware control.
- It applies a built-in expression safety policy for motion allowlists,
  adapter support, intensity caps, duration caps, night-mode minimal expression,
  and adapter busy/thermal/battery-saver state.
- It passes speech metadata into TTS through `TtsRequest`.
- It orchestrates non-speech timeline actions by phase (`pre_speech`,
  `speech_start`, `post_speech`), coalesces duplicate same-phase actions,
  tracks active plans per channel/user, blocks lower-priority overlaps, and
  preempts interruptible lower-priority plans before sending executable phase
  slices to the active channel with `execute_expression`.
- If a channel has no expression adapter, the plan degrades to speech-only.

It does not yet include a bundled LLM-backed appraisal engine. External
expression plugin manifests can be loaded as ABI v1 JSON stdin/stdout
dispatchers, while in-process Rust interfaces remain available for core
components that want tighter integration.

## Flow

```text
User input
  -> built-in rule AppraisalEngine
  -> built-in local classifier AppraisalEngine
  -> optional external classifier/LLM AppraisalEngine plugins
  -> Turn/Session/User/Device/Global AppraisalEvent stacks
  -> Affect/Social/Resource reducer snapshot
  -> Regulation policy
  -> ExpressiveStance
Assistant response metadata
  -> Turn/Session AppraisalEvent stack
  -> Affect/Social/Resource reducer snapshot
  -> Regulation policy
  -> ExpressiveStance
  -> ExpressionRuntime input context
     (dialogue_act, content_tone, conversation_context, constraints,
      user_state, device_context, safety_context)
  -> Expression Intent Planner
  -> Expression Realizer
  -> Rust extension planners/safety policies
  -> built-in safety policy
  -> ExpressionPlan
  -> TTS request style/prosody
  -> optional phased channel execute_expression actions
```

## Configuration

Runtime policy lives in:

```text
config/server.d/policies/affect.toml
config/server.d/policies/regulation.toml
config/server.d/policies/expression.toml
```

Named expression styles live in:

```text
config/server.d/profiles/expression_styles.toml
```

The built-in local text classifier reads phrase lists from language-scoped
resource fragments:

```text
config/server.d/resources/appraisal_lexicons/<lang>.toml
```

Each file contributes one `[resources.appraisal_lexicons.<lang>]` table with
categories such as `positive_phrases`, `correction_or_failure`,
`sarcasm_markers`, `support_need`, `safety_salience`, and
`minimal_expression`. Each lexicon also carries
`[resources.appraisal_lexicons.<lang>.metadata]` with source, license, version,
redistribution, and overlay fields so third-party imports remain auditable.
Adding a language should only require a new fragment, not a Rust code change.

Dialogue policy phrase lists are built in by default because they choose the
response function for the current turn rather than durable affect. Deployments
may override `[resources.dialogue_policy]` in any config fragment when the
heuristic policy needs local tuning. These lists drive deterministic
`dialogue_act` and `content_tone` decisions before the expression planner runs.
They are intentionally swappable so a classifier or router can replace the
heuristic policy later without changing the expression runtime boundary.

Each expression style can point at a voice profile. The expression runtime does
not know the TTS backend's native controls; it only passes provider-neutral
speech metadata through `TtsRequest`:

- `style`: expression style name such as `gentle`, `calm`, or `cheerful`
- `intensity`: normalized expressive strength in `[0, 1]`
- `voice_profile`: opaque profile ID for the active TTS adapter
- `prosody`: normalized `rate`, `pitch_shift`, `energy`, and `pause_bias`

```toml
[profiles.expression_styles.gentle]
voice_profile = "default"
rate = 0.95
energy = 0.42
pause_bias = 0.12

[profiles.voice.default]
provider = "gpt_sovits"
model_name = "alia-default"
reference_voice = ""
style_prompt = ""
language = "zh"

[profiles.voice.default.provider_specific]
speed_factor = 1.0
fragment_interval = 0.3
```

TTS adapters decide how to realize those fields. A provider may map `style` to
a native voice style, a prompt preset, an embedding, or ignore it if unsupported.
When a provider clamps or drops part of the request, it should return
`TtsProviderAdjustment` entries so the pipeline can trace the degradation
without embedding provider-specific logic in the expression system.
Adapters also expose `TtsCapabilities` so planners/debug views can see whether
the active provider supports pitch, rate, energy, pause control, style
embeddings, reference voices, and streaming prosody.

The built-in GPT-SoVITS adapter is one implementation of this ABI. It maps
`rate` to `speed_factor`, lets `energy` and `intensity` shape `temperature`,
`top_p`, `top_k`, and `repetition_penalty`, and maps `pause_bias` to
`fragment_interval`. `pitch_shift` is intentionally disabled by default for
GPT-SoVITS because its available fallback rewrites the WAV sample-rate header,
changing duration as well as pitch. That degradation is reported as a provider
adjustment. Other TTS providers should implement native pitch/DSP controls
inside their own adapter rather than copying the GPT-SoVITS fallback.

The affect policy is intentionally small:

```toml
[policies.affect]
enabled = true
baseline_valence = 0.05
baseline_arousal = 0.35
baseline_calmness = 0.65
baseline_engagement = 0.55
baseline_fatigue = 0.05
baseline_battery_stress = 0.0
baseline_thermal_stress = 0.0
baseline_sleepiness = 0.05
baseline_interaction_load = 0.0
baseline_trust = 0.55
baseline_familiarity = 0.15
homeostasis_rate = 0.08
max_delta_per_turn = 0.18
fatigue_per_turn = 0.015
turn_appraisal_ttl_sec = 45
session_appraisal_ttl_sec = 900
relationship_appraisal_ttl_sec = 7776000
appraisal_half_life_sec = 180
relationship_appraisal_half_life_sec = 604800
max_active_appraisal_events = 64
min_appraisal_confidence = 0.6
relationship_persistence_enabled = true
prune_persisted_appraisals_on_startup = true
persist_appraisal_evidence = true
persist_relationship_evidence = false
max_persisted_evidence_chars = 160
```

## Persistence

The database stores appraisal events, not derived emotion scores.

```text
expression_appraisal_events
  id
  channel_id
  user_id
  route
  kind/source/target/scope
  confidence
  ttl_sec
  decay
  deltas
  evidence
  created_at
```

Why this shape:

- `AffectSnapshot`, `SocialState`, and `ExpressiveStance` are derived runtime
  views. They can change when reducers or policies change.
- `AppraisalEvent` is the durable fact: what happened, how confident we were,
  what it changed, and when it expires.
- On startup, the runtime loads events newer than the largest configured TTL
  and rebuilds in-memory session, user/relationship, device, and global stacks.
- On startup, expired persisted appraisal rows are pruned when
  `prune_persisted_appraisals_on_startup` is enabled.
- During turns, new events are appended to memory immediately and persisted
  asynchronously. If persistence fails, expression planning keeps working and
  logs a warning.
- User/relationship, device, and global events share the long-term retention
  boundary. User/relationship evidence is redacted by default. Persisted
  evidence summaries are capped by
  `max_persisted_evidence_chars`; raw user text is not stored in appraisal
  evidence.
- Appraisal scopes are `turn`, `session`, `user`, `relationship`, `device`,
  and `global`. `user`, `relationship`, and `global` events use `sticky`
  decay by default; `device` events use the long TTL but normal exponential
  decay so transient resource pressure still fades.

Regulation policy controls how much internal state may be expressed:

```toml
[policies.regulation]
enabled = true
max_negative_expression = 0.15
low_trust_threshold = 0.40
fatigue_threshold = 0.62
deescalated_max_intensity = 0.38
open_trust_threshold = 0.62
```

Internal affect observations are recorded into agent history, including newly
created appraisal events and active event counts. This is for debugging and
future UI inspection only. `ExpressionPlan` includes the regulated
`ExpressiveStance`, not raw hidden state, and raw affect is not injected into
the LLM prompt.

Long-term relationship state is exposed separately as a `RelationshipProfile`:
trust/familiarity are still reduced from appraisal events, while the profile
adds a compact relationship summary, a preference summary, bounded preference
signals, recent signal counts, and privacy controls indicating whether the
profile is user-visible, clearable, and whether raw relationship evidence is
stored. Users can inspect and clear their own profile through:

- `GET /api/v1/expression/relationship`
- `DELETE /api/v1/expression/relationship`

Operators can prune stale in-memory relationship profiles with:

- `POST /api/v1/expression/relationships/prune`

Pruning and profile size are configured by affect policy fields such as
`relationship_preferences_max_items`, `relationship_summary_max_signals`, and
`relationship_profile_prune_after_sec`.

## Extension Points

Server modules and future plugins can access the expression runtime through
`ModuleContext.expression` during initialization.

Supported extension points:

- `ExternalExpressionPlugin`: a stable JSON-shaped ABI bridge for external
  adapters. It supports async appraisal, affect reduction, intent planning,
  plan transformation, and safety-policy calls. Manifests can also be loaded
  dynamically from `resources.external_expression_plugins_dir`; the dispatcher
  writes an `ExpressionPluginRequest` JSON object to the configured command's
  stdin and reads an `ExpressionPluginResponse` from stdout under ABI v1.
- `AppraisalEnginePlugin`: interprets raw user input into `AppraisalEvent`s.
  Engines may call classifiers or LLMs, but they must not mutate affect state
  directly.
- `AffectReducerPlugin`: adjusts the reduced `AffectSnapshot` for a
  `channel_id + user_id`, for example adding sensor-derived fatigue or
  relationship signals.
- `ExpressionPlannerPlugin`: mutates an `ExpressionPlan` before safety checks,
  for example selecting a fallback or making final semantic adjustments after
  built-in intent planning and realization.
- `ExpressionSafetyPolicyPlugin`: clamps or removes unsafe parts of the plan
  and returns traceable `ExpressionSafetyModification` entries.

Boundaries:

- Reducers should not execute hardware actions or call LLMs.
- Appraisal engines are the place for expensive interpretation; they return
  evidence-bearing appraisal events and leave reduction to the affect store.
- Planners should remain device-independent.
- The built-in Intent Planner decides social function, style family, priority,
  and base intensity from dialogue act, content tone, user state, safety
  context, conversation context, and constraints.
- The built-in Realizer maps that intent to speech prosody and timeline actions
  using regulated stance and negotiated device capabilities.
- Safety policies are the last mutation step before the plan reaches a channel.

## Regression Cases

`expression::eval` contains named appraisal and planner regression cases for
sarcasm, compliments, frustration, user distress, safety warnings, low battery,
night mode, and explicit minimal-expression requests. These fixtures are meant
to stay small and stable so classifier, dialogue-policy, planner, safety, and
provider changes can be checked against product-critical expression behavior.
- Concrete device execution still belongs to channel/embedded adapters via
  `execute_expression`; the embedded channel now reports supported motion only
  when a running actuator/bridge expression plugin is available and dispatches
  timeline actions through that plugin.

## Trace And Replay

The pipeline records expression decisions into conversation events and exposes a
turn-level replay view:

```text
GET /api/v1/conversations/{session_id}/expression-traces
GET /api/v1/conversations/{session_id}/expression-traces?turn_id={turn_id}
```

Each `ExpressionTrace` stitches together:

- user and assistant appraisal events
- the reduced affect/social/resource snapshot
- the explicit `ExpressionInput`
- the realized intent and full `ExpressionPlan`
- safety modifications
- TTS provider adjustment traces

This is the first place to inspect when a turn feels off. It answers whether
the cause came from appraisal, reducer state, dialogue policy, safety policy,
device capability negotiation, or TTS provider limits without manually joining
raw agent-history rows.

External plugin ABI requests are tagged JSON values:

```json
{
  "type": "plan_intent",
  "input": { "dialogue_act": "explain", "content_tone": "technical" },
  "affect": { "dominant_causes": ["baseline"] },
  "intent": {
    "intent": "calm_explanation",
    "social_function": "explain",
    "style": "calm",
    "intensity": 0.34
  }
}
```

External plugins return `noop`, `appraisal_events`, `affect_snapshot`,
`intent`, `plan`, `safety_plan`, or `health`. The runtime runs external calls
with `policies.expression.external_plugin_timeout_ms` unless a manifest sets
`request_timeout_ms`; timeouts, unexpected responses, and plugin errors are
recoverable failures.

Dynamic plugin manifests are ABI-versioned and may opt into stricter runtime
behavior:

```toml
abi_version = 1
name = "expression-demo"
enabled = true
command = "/opt/alia/plugins/expression-demo"
args = []
request_timeout_ms = 1500
trace_stderr = true
max_stderr_bytes = 4096
cancel_on_timeout = true

[sandbox]
clear_env = true
env = { ALIA_PLUGIN_MODE = "expression" }
```

`clear_env = true` is the default so external plugins receive only explicitly
allowed environment variables. `trace_stderr` is intended for development and
health checks; stderr is truncated before logging.

A minimal Python appraisal plugin lives at:

```text
packages/alia-server/examples/external_appraisal_plugin.py
packages/alia-server/examples/external_appraisal_plugin.manifest.toml
```

It reads one JSON request from stdin and writes one JSON response to stdout. The
example is keyword-based, but the same process boundary can call an LLM
classifier and return `appraisal_events` without letting the model mutate
affect state directly.

## Channel Actions

Channels that support embodied output should implement:

- `expression_capabilities`: report supported expression channels/actions.
- `execute_expression`: execute or safely skip a serialized `ExpressionPlan`.

The embedded channel currently accepts `execute_expression` and returns a
partial/skipped result when no motion adapter is registered. Concrete motor,
face, light, or haptics control should remain in embedded-side components or
plugins.

Embedded plugins must explicitly advertise expression support in their
`manifest.toml`; a running actuator/bridge plugin is not enough:

```toml
[plugin.capabilities]
expression_actions = ["small_nod", "look_at_user"]
expression_state = ["ready"]
```

The server translates these into `expression.action.<name>` and
`expression.state.<state>` capability strings. If `ready` is absent, the adapter
state is treated as unverified and motion is blocked. `execute_expression`
subscribes to the embedded plugin event stream before dispatching and waits for
an `expression_execution`, `expression_ack`, or `expression` event with the same
`command_id`. If an acknowledgement arrives, the returned execution result is
marked `completion_acknowledged = true`; otherwise the channel returns
`timed_out` with `completion_acknowledged = false`. Results include both
legacy `executed_actions` / `skipped_actions` and structured
`action_receipts` with `accepted`, `started`, `completed`, `failed`, or
`timed_out` status so callers can distinguish command dispatch from verified
device execution.

Capability snapshots should include action support and runtime state:

```json
{
  "adapter": "head_motion_v1",
  "capabilities": {
    "small_nod": {
      "supported": true,
      "params": ["intensity", "duration_ms"],
      "safety_class": "low_risk",
      "max_intensity": 0.45,
      "max_duration_ms": 900,
      "max_frequency_per_min": 6
    }
  },
  "current_state": {
    "busy": false,
    "thermal_limit": false,
    "battery_saver": false,
    "motion_adapter_registered": true
  }
}
```

## Next Steps

Future versions should add:

- A bundled LLM-backed appraisal engine for cases that outgrow the local
  classifier.
- Persisted expression adapter capability snapshots and real frequency limiting
  across turns.
- Concrete embedded-side motion/face/light/haptics adapters that execute the
  negotiated action set and emit `command_id` execution acknowledgements.
- Stronger external plugin isolation beyond manifest-controlled environment
  sandboxing.
