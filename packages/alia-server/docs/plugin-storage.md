# Plugin Storage

Updated: 2026-10-17 (PN01). This page describes the existing
storage service and its limits. The cross-package
[plugin contract](../../../docs/architecture/plugin-contract.md) defines target
management, runtime trust and lifecycle rules; it does not make those future
rules active in the current endpoints.

## Current Ownership

The server owns schema, migrations and persistence. Built-in Memory,
Interaction, Execution, Delivery and Platform services use their domain
repositories; they are not plugin implementations of replaceable core slots.

Compiled-in tool plugins implement `CompiledToolPlugin`. Bootstrap creates a
`PluginToolContext` with namespaced `PluginStorage` and the Execution-backed
`ToolRegistry`. Dependencies are wired explicitly; `ModuleContext` was removed
in L10. These are trusted in-process Rust interfaces, not a sandbox ABI.

Source contracts:

- [Plugin context](../src/plugins/context.rs)
- [Storage service and handles](../src/plugins/storage.rs)
- [Collection declarations](../src/plugins/manifest.rs)
- [Execution registry](../src/modules/execution/registry.rs)
- [Existing storage tests](../tests/plugin_storage.rs)

## Data and Implemented Semantics

Records use `(plugin_name, scope, owner_id, collection, key)` identity in
`plugin_storage_records`. Global records use an empty owner ID so the SQL unique
constraint covers them; non-global records require an owner. User, channel and
conversation scopes are supported.

Implemented mechanisms include declared collections and scope types, schema
version, TTL, per-record size validation, record versions and compare-and-put.
The storage handle fixes the plugin namespace. Record data does not become a
registered tool or Skill; runtime artifact ownership stays in the core catalog.

The existing methods accept a `StorageScope` containing an owner ID. Validation
checks the scope type and identifier, not the current authenticated actor's
permission to access that owner. The same interface must not be exposed directly
to unreviewed code. A future host broker must derive namespace and authorized
user/channel/conversation scope from trusted invocation context. Per-plugin
aggregate quotas also need implementation and evidence; per-record validation
is not proof of an aggregate bound.

## Management API Boundary

The four existing management operations require a current active administrator:
`GET /api/v1/plugins`, `GET /api/v1/plugins/{name}/storage`,
`GET /api/v1/plugins/{name}/storage/export` and
`DELETE /api/v1/plugins/{name}/storage`. Their response structures and full-plugin
scope are unchanged; ordinary users receive `403 AUTH_FORBIDDEN`.

Personal data has separate routes:

| Route (under `/api/v1`) | Scope |
| --- | --- |
| `GET /plugins/{name}/storage/me` | Current user's usage, including expired-record counts |
| `GET /plugins/{name}/storage/me/export` | Current user's non-expired records |
| `DELETE /plugins/{name}/storage/me` | Current user's records, optionally filtered by `collection` |

The host binds personal queries to authenticated `auth.id` and `scope='user'`.
They do not include global, channel or conversation records, even when an owner
string happens to match. Personal reads accept no query selectors; DELETE
accepts only `collection`. Unknown query fields such as `owner_id` or `scope`
are rejected. No user ID supplied by a plugin or URL establishes authority.

Sensitive reads revalidate the current account after fetching data and before
returning it. DELETE performs its work in a transaction, then revalidates and
locks the active account (also requiring admin for management) before commit.
A failed final authorization rolls back the deletion. This ordering covers
revocation during storage lock waits; the account lock is held through commit.
Account lookup failures/timeouts fail closed using the existing two-second
verification budget. The routes do not impose a new total SQL/HTTP deadline.

These HTTP checks do not turn the trusted in-process `PluginStorage` interface
into an untrusted host API. Complete configuration, API keys and raw private
payloads are not audit output. No general secret broker is implemented here.
Regression evidence is in the [PN01 checkpoint](../../../docs/refactor/resume-checkpoint-pn01-2026-10-17.md).

## Retention and Lifecycle

`apply_uninstall_policy` implements collection retention:

- `retain_on_uninstall` is the default and preserves records.
- `delete_on_uninstall` removes that collection's records.
- `export_required` blocks normal removal while records remain. The trusted
  `force_export_required` argument bypasses that check but still counts and
  retains those records; explicit purge is a separate operation. The helper
  does not itself verify an external export or authorize a purge.

These helpers are not a complete Server dynamic uninstall transaction. Future
uninstall must close admission, retire tools and dependencies, finish original
runtime owners, then apply authorized retention and remove unreferenced assets.
A file deletion or process signal alone is not uninstall completion. Preserve
historical effect/audit records and report partial failures with their identity.

## Remaining Delivery

Server dynamic install/activate/upgrade/uninstall, an isolated host API,
aggregate quotas and a standalone third-party SDK are not yet delivered. Keep
the current storage/CAS/TTL/retention and Execution authority while implementing
those capabilities in bounded steps. Installation of unreviewed third-party
code is an explicit future requirement; neither source review nor a market
certificate should be universally required merely to receive an inactive
package. Runtime isolation and capability grants remain separate gates.

PN01 changes HTTP authorization and adds the personal routes above; storage
schema, CAS/TTL/retention and Execution approval remain unchanged. See the [implementation checklist](../../../docs/refactor/checklists/plugin-contract-2026-10-17.md)
for current design status and later regression requirements.
