-- Migration: Server-owned plugin storage.
--
-- Plugins get a scoped JSON document store owned by alia-server. They do not
-- receive raw database connections and do not create private SQL schemas.

CREATE TABLE IF NOT EXISTS plugin_storage_records (
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
    CHECK (plugin_name <> ''),
    CHECK (collection <> ''),
    CHECK (key <> ''),
    CHECK (scope IN ('global', 'user', 'channel', 'conversation')),
    CHECK ((scope = 'global' AND owner_id = '') OR (scope <> 'global' AND owner_id <> '')),
    CHECK (schema_version >= 1),
    CHECK (version >= 1),
    UNIQUE (plugin_name, scope, owner_id, collection, key)
);

CREATE INDEX IF NOT EXISTS idx_plugin_storage_owner
    ON plugin_storage_records(plugin_name, scope, owner_id, collection);

CREATE INDEX IF NOT EXISTS idx_plugin_storage_ttl
    ON plugin_storage_records(ttl_expires_at)
    WHERE ttl_expires_at IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_plugin_storage_plugin_collection
    ON plugin_storage_records(plugin_name, collection, key);
