# Automation

Automation is split into two trigger families that share the same condition and
action system.

The server owns the current runtime implementation. See
[alia-server runtime systems](../../packages/alia-server/docs/runtime-systems.md)
for the package-local view.

## Rules

Rules are event-driven:

```text
ServerEvent + event_pattern + conditions -> action
```

Use rules for cases like:

- a sensor reading enters a range
- a device goes online or offline
- a plugin emits a specific event
- a process changes state
- a tool call succeeds or fails

Rules are managed through `/api/v1/rules` and persisted in PostgreSQL.

## Scheduler

Scheduler tasks are time or polling driven:

- `cron`: recurring calendar schedule
- `once`: one-shot run at an ISO timestamp
- `interval`: repeated run every N seconds
- `condition`: poll conditions every `poll_interval_sec`

Scheduler also supports event subscriptions, which are useful when the user
wants event-triggered behavior to live beside their scheduled tasks. Event
subscriptions and rules use the same condition and action model; rules are the
clearer product-facing abstraction for long-lived automation rules.

## Conditions

Condition expressions can be:

- an array, interpreted as AND
- `{"type":"all","conditions":[...]}`
- `{"type":"any","conditions":[...]}`
- `{"type":"not","condition":{...}}`
- built-in conditions such as `sensor`, `time_range`, `event_field`, and
  `event_pattern`
- plugin-defined condition types registered into the shared condition registry

Unknown condition types fail closed.

## Actions

Built-in action types include:

- `log`: write an automation log entry
- `emit_event`: publish a plugin-style event onto the server event bus
- `tool_call`: execute a registered tool
- `insights`: no-op action used by scheduled insights tasks, which respond to
  the scheduler's `TaskTriggered` event

Plugins can register additional action executors through the shared action
registry.
