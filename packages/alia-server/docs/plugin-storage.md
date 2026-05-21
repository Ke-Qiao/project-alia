# Plugin Storage Design

Future LLM tool plugins need durable state, but they should not own the server
database schema or receive raw database connections. The server stays the
persistence authority and gives compiled-in tool plugins a namespaced storage
capability.

Built-in server components are not plugins. Core systems such as embedded
channels, memory, expression, scheduler, sensors, and rules should keep using
their own repositories, migrations, and domain tables.

## Goals

- Keep the server core independent from plugin-owned schema.
- Prevent plugin SQL/schema drift from breaking server migrations.
- Let plugins store user-scoped state, channel-scoped state, cache entries, and
  small structured documents.
- Prepare storage semantics for future plugin lifecycle without implementing
  dynamic plugin install/start/stop yet.

## Domain Model

| Concept | Identity | Notes |
| --- | --- | --- |
| Plugin | `plugin_name` | Stable compiled-in tool plugin name; owns storage collections. |
| Collection | `(plugin_name, collection)` | Plugin-declared logical bucket, for example `settings`, `cache`, `devices`. |
| Record | `(plugin_name, scope, owner_id, collection, key)` | A JSON document with version and timestamps. |
| Blob | `(plugin_name, blob_id)` | Optional future large binary/object reference, not inline JSON. |

`scope` should be one of:

- `global`: one plugin-wide record namespace.
- `user`: records owned by a user.
- `channel`: records owned by a channel/device.
- `conversation`: records tied to a conversation/session.

## Storage Table

First pass should use a generic JSONB document table, not plugin-created SQL
tables:

```sql
CREATE TABLE plugin_storage_records (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    plugin_name VARCHAR(100) NOT NULL,
    collection VARCHAR(100) NOT NULL,
    key VARCHAR(200) NOT NULL,
    scope VARCHAR(30) NOT NULL,
    owner_id VARCHAR(200) NOT NULL DEFAULT '',
    value JSONB NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    version BIGINT NOT NULL DEFAULT 1,
    ttl_expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK ((scope = 'global' AND owner_id = '') OR (scope <> 'global' AND owner_id <> '')),
    UNIQUE (plugin_name, scope, owner_id, collection, key)
);
```

`owner_id` is stored as an empty string for `global` scope so PostgreSQL unique
constraints cannot be bypassed by multiple `NULL` values. Runtime structs still
present global records as having no owner.

Recommended indexes:

```sql
CREATE INDEX idx_plugin_storage_owner
    ON plugin_storage_records(plugin_name, scope, owner_id, collection);

CREATE INDEX idx_plugin_storage_ttl
    ON plugin_storage_records(ttl_expires_at)
    WHERE ttl_expires_at IS NOT NULL;
```

Use JSONB GIN indexes only after a real plugin needs query-heavy document
search. The default API should prefer `get`, `put`, `delete`, and prefix/list
by collection/key.

## Runtime API

Plugins should receive a scoped handle:

```rust
pub trait PluginStorage: Send + Sync {
    async fn get(&self, collection: &str, key: &str, scope: StorageScope)
        -> anyhow::Result<Option<PluginRecord>>;

    async fn put(&self, collection: &str, key: &str, scope: StorageScope, value: serde_json::Value)
        -> anyhow::Result<PluginRecord>;

    async fn compare_and_put(
        &self,
        collection: &str,
        key: &str,
        scope: StorageScope,
        expected_version: i64,
        value: serde_json::Value,
    ) -> anyhow::Result<PluginRecord>;

    async fn delete(&self, collection: &str, key: &str, scope: StorageScope)
        -> anyhow::Result<bool>;

    async fn list(&self, collection: &str, scope: StorageScope, prefix: Option<&str>, limit: u32)
        -> anyhow::Result<Vec<PluginRecord>>;
}
```

The handle is constructed with `plugin_name`; plugins cannot choose or spoof
another plugin namespace.

`alia-server` exposes this through `PluginStorageService`. Compiled-in tool
plugins receive a `PluginToolContext` containing only their scoped storage handle
and the shared tool registry. `ModuleContext` carries the service so this stays
independent from embedded channel support.

## Static Declaration Contract

First-pass compiled-in plugins should declare storage use in Rust code. The
shape intentionally mirrors a future manifest, but there is no dynamic plugin
installer in this phase:

```rust
PluginStorageManifest {
    collections: vec![
        PluginStorageCollection {
            name: "settings".into(),
            scope: vec![PluginStorageScopeKind::User],
            schema_version: 1,
            retention: PluginStorageRetention::RetainOnUninstall,
            ttl_sec: None,
            max_record_bytes: None,
        },
    ],
}
```

The server validates collection names, allowed scopes, retention policy, and
record limits before handing storage to the plugin.

## Security And Boundaries

- Do not store secrets in `plugin_storage_records`; use a server-managed secret
  store and keep only secret references in plugin records.
- Do not expose arbitrary SQL to plugins.
- Do not let plugin storage rows masquerade as skills/tools/sensors. Runtime
  artifacts remain in `registry_artifacts`; storage is plugin-private data.
- Enforce size limits per record and per plugin.
- Record writes should be attributed in audit/debug output when they happen
  during tool execution.
- Embedded-side plugin management is not part of this server plugin-storage
  phase.

## Future Uninstall Behavior

When dynamic plugins exist, plugin uninstall should:

1. Stop plugin execution.
2. Unregister plugin tools, skills, sensors, and other artifacts.
3. Apply each collection retention policy:
   - `delete_on_uninstall`: delete rows.
   - `retain_on_uninstall`: keep rows for reinstall/export.
   - `export_required`: block uninstall until exported or explicitly purged.
4. Emit an audit event with deleted/retained counts.

## Suggested Phases

1. Add `plugin_storage_records` and a `PluginStorage` trait. Done.
2. Inject a scoped storage handle into compiled-in tool plugin contexts. Done.
3. Add static declaration validation for declared collections and scopes. Done.
4. Add API/admin visibility for plugin storage usage and purge/export. Done.
5. Add dynamic plugin lifecycle only when the project actually needs it.
6. Add optional blob storage and JSONB query indexes only after concrete plugin
   requirements appear.
