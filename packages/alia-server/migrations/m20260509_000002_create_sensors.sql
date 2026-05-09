-- Sensor definitions (registered sensors)
CREATE TABLE IF NOT EXISTS sensor_definitions (
    sensor_id       TEXT PRIMARY KEY,
    display_name    TEXT NOT NULL,
    source          TEXT NOT NULL,           -- 'embedded', 'plugin', 'api'
    unit            TEXT NOT NULL DEFAULT '',
    value_type      TEXT NOT NULL DEFAULT 'float',  -- 'float', 'int', 'bool', 'string'
    metadata        JSONB NOT NULL DEFAULT '{}',
    registered_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at    TIMESTAMPTZ
);

-- Time-series sensor readings (partitioned-friendly schema)
CREATE TABLE IF NOT EXISTS sensor_readings (
    id          BIGSERIAL,
    sensor_id   TEXT NOT NULL REFERENCES sensor_definitions(sensor_id) ON DELETE CASCADE,
    value       JSONB NOT NULL,
    quality     REAL NOT NULL DEFAULT 1.0,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id)
);

-- Index for time-range queries per sensor
CREATE INDEX idx_sensor_readings_sensor_time
    ON sensor_readings (sensor_id, recorded_at DESC);

-- Index for cleanup (retention policy)
CREATE INDEX idx_sensor_readings_recorded_at
    ON sensor_readings (recorded_at);

-- Index for source-based listing
CREATE INDEX idx_sensor_definitions_source
    ON sensor_definitions (source);
