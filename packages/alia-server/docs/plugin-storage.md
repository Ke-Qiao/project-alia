# Plugin Storage

Updated: 2026-10-17, checked against `89d04a2`. This page describes the existing
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

Current routes provide plugin listing, usage, export and purge. At this baseline
[handlers](../src/api/plugins.rs) require `AuthUser`; full export/purge queries
operate by plugin name across owners. There is no additional administrator or
per-owner boundary at those handlers. This is a statically identified gap, not
a dynamically reproduced result or a permission fix shipped by this page.

The target administrator/user scope distinction is recorded in the plugin
contract. Its later implementation must test ordinary users, current admin
identity after waiting, cross-owner refusal and unchanged data on rejection.
Complete configuration, API keys and raw private payloads are not audit output.
Plugin records should contain secret references; no general secret broker is
claimed as already implemented here.

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

No production code, API, schema or permission is changed by this documentation
update. See the [implementation checklist](../../../docs/refactor/checklists/plugin-contract-2026-10-17.md)
for current design status and later regression requirements.
