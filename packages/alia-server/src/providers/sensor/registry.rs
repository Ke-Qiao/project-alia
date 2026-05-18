//! Sensor registry with PostgreSQL persistence and in-memory fallback.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use utoipa::{IntoParams, ToSchema};

use crate::registry::{ArtifactRef, ArtifactRegistry};

/// Metadata describing a registered sensor.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SensorDefinition {
    /// Unique sensor identifier: "{source}/{name}" (e.g., "embedded/temperature", "plugin.weather/humidity")
    pub sensor_id: String,
    /// Human-readable name
    pub display_name: String,
    /// Sensor source: "embedded", "plugin", "api"
    pub source: String,
    /// Unit of measurement (e.g., "°C", "%", "lux", "boolean")
    pub unit: String,
    /// Data type hint: "float", "int", "bool", "string"
    pub value_type: String,
    /// Optional metadata (location, device_id, etc.)
    pub metadata: serde_json::Value,
    /// When the sensor was first registered
    pub registered_at: DateTime<Utc>,
    /// Last time a reading was received
    pub last_seen_at: Option<DateTime<Utc>>,
}

/// A single sensor reading (time-series data point).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SensorReading {
    pub sensor_id: String,
    pub value: serde_json::Value,
    pub timestamp: DateTime<Utc>,
    /// Optional quality/confidence indicator (0.0 - 1.0)
    #[serde(default = "default_quality")]
    pub quality: f32,
}

fn default_quality() -> f32 {
    1.0
}

/// Query filter for time-series data.
#[derive(Debug, Clone, Default, Deserialize, IntoParams, ToSchema)]
pub struct SensorQuery {
    pub sensor_id: Option<String>,
    pub source: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
    /// If true, return only the latest reading per sensor
    #[serde(default)]
    pub latest_only: bool,
}

/// Aggregated sensor summary.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SensorSummary {
    pub sensor_id: String,
    pub display_name: String,
    pub unit: String,
    pub latest_value: Option<serde_json::Value>,
    pub latest_timestamp: Option<DateTime<Utc>>,
    pub reading_count: usize,
    pub online: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorReadMode {
    CacheOnly,
    BestEffort,
    Fresh,
}

impl SensorReadMode {
    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cache_only" | "cache-only" | "cache" | "latest" => Some(Self::CacheOnly),
            "best_effort" | "best-effort" | "best" | "auto" => Some(Self::BestEffort),
            "fresh" | "force_fresh" | "force-fresh" | "remote" => Some(Self::Fresh),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorStalePolicy {
    AllowStale,
    FailClosed,
}

impl SensorStalePolicy {
    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "allow_stale" | "allow-stale" | "allow" | "stale_ok" | "stale-ok" => {
                Some(Self::AllowStale)
            }
            "fail_closed" | "fail-closed" | "fresh_only" | "fresh-only" | "strict" => {
                Some(Self::FailClosed)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SensorFreshnessPolicy {
    pub mode: SensorReadMode,
    pub max_age: Duration,
    pub timeout: Duration,
    pub stale_policy: SensorStalePolicy,
}

impl SensorFreshnessPolicy {
    pub fn tool_default(offline_threshold: Duration) -> Self {
        Self {
            mode: SensorReadMode::BestEffort,
            max_age: offline_threshold,
            timeout: Duration::from_secs(2),
            stale_policy: SensorStalePolicy::AllowStale,
        }
    }

    pub fn scheduler_default() -> Self {
        Self {
            mode: SensorReadMode::Fresh,
            max_age: Duration::from_secs(5),
            timeout: Duration::from_secs(2),
            stale_policy: SensorStalePolicy::FailClosed,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SensorReadResult {
    pub definition: Option<SensorDefinition>,
    pub reading: Option<SensorReading>,
    pub fresh: bool,
    pub stale: bool,
    pub refreshed: bool,
    pub age_ms: Option<i64>,
    pub warning: Option<String>,
}

/// Pull-based reader for sensors that can be queried on demand.
///
/// Push-only sources can ignore this trait and keep using `ingest`. Embedded
/// devices and future plugins can register a provider so generic tools and
/// scheduler conditions get fresh values without knowing channel internals.
#[async_trait]
pub trait SensorReadProvider: Send + Sync {
    fn source(&self) -> &str;

    async fn read_sensor(
        &self,
        sensor_id: &str,
        definition: &SensorDefinition,
    ) -> anyhow::Result<SensorReading>;
}

/// Configuration for the sensor registry.
#[derive(Debug, Clone)]
pub struct SensorRegistryConfig {
    /// Maximum readings to keep in memory per sensor
    pub max_readings_per_sensor: usize,
    /// Auto-cleanup: discard readings older than this duration
    pub retention_duration: Duration,
    /// How often to run cleanup (seconds)
    pub cleanup_interval_sec: u64,
    /// Sensor is considered offline if no reading for this duration
    pub offline_threshold: Duration,
}

impl Default for SensorRegistryConfig {
    fn default() -> Self {
        Self {
            max_readings_per_sensor: 1000,
            retention_duration: Duration::from_secs(24 * 3600),
            cleanup_interval_sec: 300,
            offline_threshold: Duration::from_secs(120),
        }
    }
}

/// Sensor registry shared by APIs, tools, rules, and scheduler conditions.
pub struct SensorRegistry {
    config: SensorRegistryConfig,
    db: Arc<RwLock<Option<DatabaseConnection>>>,
    artifacts: Arc<RwLock<Option<Arc<ArtifactRegistry>>>>,
    /// Hot cache and fallback when no DB is attached.
    sensors: Arc<RwLock<HashMap<String, SensorDefinition>>>,
    readings: Arc<RwLock<HashMap<String, Vec<SensorReading>>>>,
    read_providers: Arc<RwLock<HashMap<String, Arc<dyn SensorReadProvider>>>>,
    read_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
}

impl SensorRegistry {
    pub fn new(config: SensorRegistryConfig) -> Self {
        Self {
            config,
            db: Arc::new(RwLock::new(None)),
            artifacts: Arc::new(RwLock::new(None)),
            sensors: Arc::new(RwLock::new(HashMap::new())),
            readings: Arc::new(RwLock::new(HashMap::new())),
            read_providers: Arc::new(RwLock::new(HashMap::new())),
            read_locks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Attach PostgreSQL persistence after migrations have run.
    pub async fn attach_db(&self, db: DatabaseConnection) -> anyhow::Result<()> {
        *self.db.write().await = Some(db);
        self.refresh_hot_cache().await?;
        Ok(())
    }

    /// Attach the runtime artifact registry so sensors participate in plugin
    /// ownership and dependency checks.
    pub async fn attach_artifacts(&self, artifacts: Arc<ArtifactRegistry>) {
        *self.artifacts.write().await = Some(artifacts);
    }

    async fn db(&self) -> Option<DatabaseConnection> {
        self.db.read().await.clone()
    }

    async fn artifacts(&self) -> Option<Arc<ArtifactRegistry>> {
        self.artifacts.read().await.clone()
    }

    pub fn offline_threshold(&self) -> Duration {
        self.config.offline_threshold
    }

    /// Register an on-demand reader for all sensors owned by `provider.source()`.
    pub async fn register_read_provider(
        &self,
        provider: Arc<dyn SensorReadProvider>,
    ) -> anyhow::Result<()> {
        let source = validate_owner_name(provider.source(), "sensor read provider source")?;
        self.read_providers
            .write()
            .await
            .insert(source.clone(), provider);
        tracing::info!(source = %source, "sensor read provider registered");
        Ok(())
    }

    pub async fn unregister_read_provider(&self, source: &str) {
        let source = source.trim();
        if self.read_providers.write().await.remove(source).is_some() {
            tracing::info!(source = %source, "sensor read provider unregistered");
        }
    }

    async fn refresh_hot_cache(&self) -> anyhow::Result<()> {
        let Some(db) = self.db().await else {
            return Ok(());
        };

        let definitions = load_sensor_definitions(&db).await?;
        {
            let mut sensors = self.sensors.write().await;
            sensors.clear();
            for def in definitions {
                sensors.insert(def.sensor_id.clone(), def);
            }
        }

        let latest = self
            .query(&SensorQuery {
                latest_only: true,
                ..SensorQuery::default()
            })
            .await?;
        {
            let mut readings = self.readings.write().await;
            readings.clear();
            for reading in latest {
                readings
                    .entry(reading.sensor_id.clone())
                    .or_default()
                    .push(reading);
            }
        }

        Ok(())
    }

    /// Register or update a sensor definition.
    pub async fn register_sensor(&self, mut def: SensorDefinition) -> anyhow::Result<()> {
        validate_sensor_id(&def.sensor_id)?;
        def.source = normalize_source(&def.source);
        normalize_definition_timestamps(&mut def);

        if let Some(db) = self.db().await {
            upsert_sensor_definition(&db, &def).await?;
        }

        if let Some(artifacts) = self.artifacts().await {
            artifacts
                .upsert_artifact(
                    &ArtifactRef::sensor(def.sensor_id.clone()),
                    &sensor_artifact_source(&def.source),
                )
                .await?;
        }

        self.remember_definition(def).await;
        Ok(())
    }

    /// Mark source-owned sensors that are no longer advertised as offline.
    ///
    /// This keeps definitions stable for tools/rules/frontend references while
    /// letting summaries correctly report that a remote sensor has disappeared.
    pub async fn mark_source_sensors_offline_except(
        &self,
        source: &str,
        active_sensor_ids: &HashSet<String>,
    ) -> anyhow::Result<usize> {
        let source = validate_owner_name(source, "source")?;
        let definitions = self.list_definitions_by_source(&source).await?;
        let mut updated = 0usize;

        for def in definitions {
            if active_sensor_ids.contains(&def.sensor_id) {
                continue;
            }
            self.set_sensor_last_seen(&def.sensor_id, None).await?;
            updated += 1;
        }

        Ok(updated)
    }

    /// Unregister a sensor and discard its readings.
    pub async fn unregister_sensor(&self, sensor_id: &str) -> anyhow::Result<bool> {
        self.unregister_sensor_inner(sensor_id, true).await
    }

    async fn unregister_sensor_inner(
        &self,
        sensor_id: &str,
        check_dependents: bool,
    ) -> anyhow::Result<bool> {
        validate_sensor_id(sensor_id)?;
        let artifact_ref = ArtifactRef::sensor(sensor_id);
        let artifacts = self.artifacts().await;

        if check_dependents && let Some(artifacts) = &artifacts {
            artifacts
                .ensure_no_external_dependents(std::slice::from_ref(&artifact_ref))
                .await?;
        }

        let db_removed = if let Some(db) = self.db().await {
            delete_sensor_definition(&db, sensor_id).await?
        } else {
            false
        };

        let memory_removed = self.sensors.write().await.remove(sensor_id).is_some();
        self.readings.write().await.remove(sensor_id);
        self.read_locks.write().await.remove(sensor_id);
        let removed = db_removed || memory_removed;
        if removed {
            if let Some(artifacts) = artifacts {
                artifacts.delete_artifact(&artifact_ref).await?;
            }
            tracing::info!(%sensor_id, "sensor unregistered");
        }
        Ok(removed)
    }

    /// Register a sensor owned by a plugin. The public sensor id is namespaced
    /// as `{plugin_name}:{sensor_id}` unless the caller already used that prefix.
    pub async fn register_plugin_sensor(
        &self,
        plugin_name: &str,
        mut def: SensorDefinition,
    ) -> anyhow::Result<String> {
        let plugin_name = validate_plugin_name(plugin_name)?;
        let sensor_id = namespace_sensor_id(&plugin_name, &def.sensor_id)?;
        def.sensor_id = sensor_id.clone();
        def.source = plugin_name;
        if def.display_name.trim().is_empty() {
            def.display_name = sensor_id.clone();
        }
        self.register_sensor(def).await?;
        Ok(sensor_id)
    }

    /// Ingest a reading for a plugin-owned sensor. Unknown sensors are
    /// auto-registered with plugin ownership.
    pub async fn ingest_plugin_reading(
        &self,
        plugin_name: &str,
        sensor_id: &str,
        value: serde_json::Value,
        quality: f32,
    ) -> anyhow::Result<String> {
        let plugin_name = validate_plugin_name(plugin_name)?;
        let sensor_id = namespace_sensor_id(&plugin_name, sensor_id)?;
        self.ingest_from_source(
            SensorReading {
                sensor_id: sensor_id.clone(),
                value,
                timestamp: Utc::now(),
                quality,
            },
            &plugin_name,
        )
        .await?;
        Ok(sensor_id)
    }

    /// Unregister all sensors owned by a plugin. When the artifact registry is
    /// attached this first checks that no external artifact still depends on
    /// those sensors.
    pub async fn unregister_plugin_sensors(&self, plugin_name: &str) -> anyhow::Result<usize> {
        let plugin_name = validate_plugin_name(plugin_name)?;

        if let Some(artifacts) = self.artifacts().await {
            let owned = artifacts
                .owned_artifacts("plugin", &plugin_name, Some(crate::registry::KIND_SENSOR))
                .await?;
            artifacts.ensure_no_external_dependents(&owned).await?;

            let mut removed = 0;
            for artifact in owned {
                if self.unregister_sensor_inner(&artifact.name, false).await? {
                    removed += 1;
                }
            }
            return Ok(removed);
        }

        let definitions = self.list_definitions_by_source(&plugin_name).await?;
        let mut removed = 0;
        for def in definitions {
            if self.unregister_sensor_inner(&def.sensor_id, false).await? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Ingest a new reading. Auto-registers unknown sensors.
    pub async fn ingest(&self, reading: SensorReading) -> anyhow::Result<()> {
        let source = infer_source(&reading.sensor_id);
        self.ingest_from_source(reading, &source).await
    }

    /// Ingest a new reading from a known source. This is the preferred entry
    /// point for channels/plugins because it preserves source ownership.
    pub async fn ingest_from_source(
        &self,
        mut reading: SensorReading,
        source: &str,
    ) -> anyhow::Result<()> {
        validate_sensor_id(&reading.sensor_id)?;
        reading.quality = reading.quality.clamp(0.0, 1.0);

        if self.get_sensor(&reading.sensor_id).await?.is_none() {
            self.register_sensor(SensorDefinition {
                sensor_id: reading.sensor_id.clone(),
                display_name: reading.sensor_id.clone(),
                source: source.trim().to_string(),
                unit: String::new(),
                value_type: infer_value_type(&reading.value).to_string(),
                metadata: serde_json::json!({}),
                registered_at: Utc::now(),
                last_seen_at: Some(reading.timestamp),
            })
            .await?;
        } else {
            self.touch_sensor(&reading.sensor_id, reading.timestamp)
                .await?;
        }

        if let Some(db) = self.db().await {
            insert_sensor_reading(&db, &reading).await?;
        }

        self.remember_reading(reading).await;
        Ok(())
    }

    /// Get the latest reading for a sensor.
    pub async fn latest(&self, sensor_id: &str) -> anyhow::Result<Option<SensorReading>> {
        validate_sensor_id(sensor_id)?;
        if let Some(db) = self.db().await {
            return latest_sensor_reading(&db, sensor_id).await;
        }

        let readings = self.readings.read().await;
        Ok(readings
            .get(sensor_id)
            .and_then(|series| series.last().cloned()))
    }

    /// Read a sensor with a cache/freshness policy.
    ///
    /// This is the shared path for tools, scheduler conditions, and future
    /// plugins. It first checks the latest persisted/cache value, then asks the
    /// source-owned read provider when the value is missing or too old.
    pub async fn read(
        &self,
        sensor_id: &str,
        policy: SensorFreshnessPolicy,
    ) -> anyhow::Result<SensorReadResult> {
        validate_sensor_id(sensor_id)?;

        let definition = self.get_sensor(sensor_id).await?;
        let latest = self.latest(sensor_id).await?;
        if matches!(policy.mode, SensorReadMode::CacheOnly) || reading_is_fresh(&latest, &policy) {
            return Ok(sensor_read_result(definition, latest, &policy, false, None));
        }

        let Some(definition_for_provider) = definition.clone() else {
            return Ok(sensor_read_result(
                None,
                latest,
                &policy,
                false,
                Some("sensor is not registered".to_string()),
            ));
        };

        let provider = self
            .read_providers
            .read()
            .await
            .get(&definition_for_provider.source)
            .cloned();
        let Some(provider) = provider else {
            return Ok(sensor_read_result(
                Some(definition_for_provider),
                latest,
                &policy,
                false,
                Some("no on-demand reader registered for sensor source".to_string()),
            ));
        };

        let lock = self.read_lock(sensor_id).await;
        let _guard = lock.lock().await;

        let definition = self.get_sensor(sensor_id).await?;
        let latest = self.latest(sensor_id).await?;
        if reading_is_fresh(&latest, &policy) {
            return Ok(sensor_read_result(definition, latest, &policy, false, None));
        }

        let Some(definition_for_provider) = definition.clone() else {
            return Ok(sensor_read_result(
                None,
                latest,
                &policy,
                false,
                Some("sensor is not registered".to_string()),
            ));
        };

        let read_result = tokio::time::timeout(
            policy.timeout,
            provider.read_sensor(sensor_id, &definition_for_provider),
        )
        .await;

        match read_result {
            Ok(Ok(mut reading)) => {
                if reading.sensor_id.trim().is_empty() {
                    reading.sensor_id = sensor_id.to_string();
                } else if reading.sensor_id != sensor_id {
                    tracing::warn!(
                        requested_sensor_id = %sensor_id,
                        returned_sensor_id = %reading.sensor_id,
                        "sensor read provider returned a different sensor id; normalizing"
                    );
                    reading.sensor_id = sensor_id.to_string();
                }
                reading.quality = reading.quality.clamp(0.0, 1.0);
                self.ingest_from_source(reading.clone(), &definition_for_provider.source)
                    .await?;
                Ok(sensor_read_result(
                    Some(definition_for_provider),
                    Some(reading),
                    &policy,
                    true,
                    None,
                ))
            }
            Ok(Err(error)) => Ok(sensor_read_result(
                definition,
                latest,
                &policy,
                false,
                Some(format!("fresh sensor read failed: {error}")),
            )),
            Err(_) => Ok(sensor_read_result(
                definition,
                latest,
                &policy,
                false,
                Some(format!(
                    "fresh sensor read timed out after {} ms",
                    policy.timeout.as_millis()
                )),
            )),
        }
    }

    /// Get the latest value as f64 (for condition evaluation).
    pub async fn latest_value_f64(&self, sensor_id: &str) -> anyhow::Result<Option<f64>> {
        Ok(self.latest(sensor_id).await?.and_then(|r| match &r.value {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            serde_json::Value::String(s) => s.parse().ok(),
            _ => None,
        }))
    }

    async fn read_lock(&self, sensor_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.read_locks.write().await;
        locks
            .entry(sensor_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Query readings with filters.
    pub async fn query(&self, q: &SensorQuery) -> anyhow::Result<Vec<SensorReading>> {
        if let Some(db) = self.db().await {
            return query_sensor_readings(&db, q).await;
        }

        Ok(self.query_memory(q).await)
    }

    /// List all registered sensors with summary.
    pub async fn list_sensors(&self) -> anyhow::Result<Vec<SensorSummary>> {
        if let Some(db) = self.db().await {
            return list_sensor_summaries(&db, self.config.offline_threshold).await;
        }

        Ok(self.list_sensors_memory().await)
    }

    /// List registered sensor definitions owned by a source/plugin.
    pub async fn list_definitions_by_source(
        &self,
        source: &str,
    ) -> anyhow::Result<Vec<SensorDefinition>> {
        let source = validate_owner_name(source, "source")?;
        if let Some(db) = self.db().await {
            return load_sensor_definitions_by_source(&db, &source).await;
        }

        Ok(self
            .sensors
            .read()
            .await
            .values()
            .filter(|def| def.source == source)
            .cloned()
            .collect())
    }

    /// Get a specific sensor definition.
    pub async fn get_sensor(&self, sensor_id: &str) -> anyhow::Result<Option<SensorDefinition>> {
        validate_sensor_id(sensor_id)?;
        if let Some(db) = self.db().await {
            return get_sensor_definition(&db, sensor_id).await;
        }

        Ok(self.sensors.read().await.get(sensor_id).cloned())
    }

    /// Run cleanup: discard readings older than retention_duration.
    pub async fn cleanup(&self) -> anyhow::Result<()> {
        if let Some(db) = self.db().await {
            cleanup_db_readings(&db, self.config.retention_duration).await?;
        }
        self.cleanup_memory().await;
        Ok(())
    }

    /// Spawn the auto-cleanup background task.
    pub fn spawn_cleanup_loop(&self, cancel_token: CancellationToken) {
        let readings = self.readings.clone();
        let db = self.db.clone();
        let retention = self.config.retention_duration;
        let interval_sec = self.config.cleanup_interval_sec;
        let max_per_sensor = self.config.max_readings_per_sensor;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_sec));

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    _ = interval.tick() => {
                        if let Some(db) = db.read().await.clone()
                            && let Err(error) = cleanup_db_readings(&db, retention).await
                        {
                            tracing::warn!(error = %error, "sensor DB cleanup failed");
                        }

                        let cutoff = Utc::now()
                            - chrono::Duration::from_std(retention)
                                .unwrap_or(chrono::Duration::hours(24));

                        let mut readings = readings.write().await;
                        for series in readings.values_mut() {
                            series.retain(|r| r.timestamp >= cutoff);
                            if series.len() > max_per_sensor {
                                let excess = series.len() - max_per_sensor;
                                series.drain(..excess);
                            }
                        }
                    }
                }
            }
        });
    }

    async fn touch_sensor(&self, sensor_id: &str, seen_at: DateTime<Utc>) -> anyhow::Result<()> {
        self.set_sensor_last_seen(sensor_id, Some(seen_at)).await
    }

    async fn set_sensor_last_seen(
        &self,
        sensor_id: &str,
        seen_at: Option<DateTime<Utc>>,
    ) -> anyhow::Result<()> {
        if let Some(db) = self.db().await {
            db.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE sensor_definitions SET last_seen_at = NULLIF($2, '')::timestamptz WHERE sensor_id = $1",
                vec![
                    sensor_id.to_string().into(),
                    seen_at
                        .map(|value| value.to_rfc3339())
                        .unwrap_or_default()
                        .into(),
                ],
            ))
            .await?;
        }

        let mut sensors = self.sensors.write().await;
        if let Some(def) = sensors.get_mut(sensor_id) {
            def.last_seen_at = seen_at;
        }
        Ok(())
    }

    async fn remember_definition(&self, def: SensorDefinition) {
        let mut sensors = self.sensors.write().await;
        if let Some(existing) = sensors.get_mut(&def.sensor_id) {
            let registered_at = existing.registered_at;
            *existing = def;
            existing.registered_at = registered_at;
        } else {
            tracing::info!(sensor_id = %def.sensor_id, source = %def.source, "sensor registered");
            sensors.insert(def.sensor_id.clone(), def);
        }
    }

    async fn remember_reading(&self, reading: SensorReading) {
        let mut readings = self.readings.write().await;
        let series = readings.entry(reading.sensor_id.clone()).or_default();
        series.push(reading);

        if series.len() > self.config.max_readings_per_sensor {
            let excess = series.len() - self.config.max_readings_per_sensor;
            series.drain(..excess);
        }
    }

    async fn query_memory(&self, q: &SensorQuery) -> Vec<SensorReading> {
        let readings = self.readings.read().await;
        let mut results = Vec::new();

        let sensor_ids: Vec<String> = if let Some(ref id) = q.sensor_id {
            vec![id.clone()]
        } else if let Some(ref source) = q.source {
            let sensors = self.sensors.read().await;
            sensors
                .values()
                .filter(|s| s.source == *source)
                .map(|s| s.sensor_id.clone())
                .collect()
        } else {
            readings.keys().cloned().collect()
        };

        for sid in &sensor_ids {
            if let Some(series) = readings.get(sid) {
                if q.latest_only {
                    if let Some(last) = series.last() {
                        results.push(last.clone());
                    }
                } else {
                    for r in series.iter().rev() {
                        if let Some(from) = q.from
                            && r.timestamp < from
                        {
                            break;
                        }
                        if let Some(to) = q.to
                            && r.timestamp > to
                        {
                            continue;
                        }
                        results.push(r.clone());
                        if let Some(limit) = q.limit
                            && results.len() >= limit
                        {
                            return results;
                        }
                    }
                }
            }
        }

        results
    }

    async fn list_sensors_memory(&self) -> Vec<SensorSummary> {
        let sensors = self.sensors.read().await;
        let readings = self.readings.read().await;
        let now = Utc::now();

        sensors
            .values()
            .map(|def| {
                let series = readings.get(&def.sensor_id);
                let latest = series.and_then(|s| s.last());
                let online = def.last_seen_at.is_some_and(|t| {
                    (now - t).to_std().unwrap_or(Duration::MAX) < self.config.offline_threshold
                });

                SensorSummary {
                    sensor_id: def.sensor_id.clone(),
                    display_name: def.display_name.clone(),
                    unit: def.unit.clone(),
                    latest_value: latest.map(|r| r.value.clone()),
                    latest_timestamp: latest.map(|r| r.timestamp),
                    reading_count: series.map_or(0, |s| s.len()),
                    online,
                }
            })
            .collect()
    }

    async fn cleanup_memory(&self) {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(self.config.retention_duration)
                .unwrap_or(chrono::Duration::hours(24));
        let mut readings = self.readings.write().await;
        let mut total_removed = 0usize;

        for series in readings.values_mut() {
            let before = series.len();
            series.retain(|r| r.timestamp >= cutoff);
            total_removed += before - series.len();
        }

        if total_removed > 0 {
            tracing::debug!(removed = total_removed, "sensor cleanup completed");
        }
    }
}

fn normalize_definition_timestamps(def: &mut SensorDefinition) {
    if def.registered_at.timestamp() <= 0 {
        def.registered_at = Utc::now();
    }
}

fn validate_sensor_id(sensor_id: &str) -> anyhow::Result<()> {
    let sensor_id = sensor_id.trim();
    if sensor_id.is_empty() || sensor_id.len() > 200 {
        anyhow::bail!("sensor_id must be between 1 and 200 characters");
    }
    Ok(())
}

fn validate_owner_name(value: &str, label: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 100 {
        anyhow::bail!("{label} must be between 1 and 100 characters");
    }
    Ok(value.to_string())
}

fn validate_plugin_name(plugin_name: &str) -> anyhow::Result<String> {
    let plugin_name = plugin_name
        .trim()
        .strip_prefix("plugin:")
        .unwrap_or(plugin_name.trim());
    validate_owner_name(plugin_name, "plugin_name")
}

fn normalize_source(source: &str) -> String {
    let source = source.trim();
    if source.is_empty() {
        "unknown".to_string()
    } else {
        source.to_string()
    }
}

fn namespace_sensor_id(source: &str, sensor_id: &str) -> anyhow::Result<String> {
    let source = validate_owner_name(source, "source")?;
    let sensor_id = sensor_id.trim();
    validate_sensor_id(sensor_id)?;
    if sensor_id_has_source_prefix(&source, sensor_id) {
        Ok(sensor_id.to_string())
    } else {
        Ok(format!("{source}:{sensor_id}"))
    }
}

fn sensor_id_has_source_prefix(source: &str, sensor_id: &str) -> bool {
    sensor_id
        .strip_prefix(source)
        .is_some_and(|rest| matches!(rest.chars().next(), Some(':' | '/' | '.')))
}

fn sensor_artifact_source(source: &str) -> String {
    let source = source.trim();
    if source.starts_with("component:")
        || source.starts_with("plugin:")
        || matches!(source, "api" | "builtin" | "file" | "manual")
    {
        source.to_string()
    } else if matches!(source, "embedded" | "unknown") {
        format!("component:{source}")
    } else {
        source.to_string()
    }
}

fn infer_source(sensor_id: &str) -> String {
    sensor_id
        .split(['/', ':', '.'])
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn infer_value_type(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(number) if number.is_i64() || number.is_u64() => "int",
        serde_json::Value::Number(_) => "float",
        serde_json::Value::String(_) => "string",
        _ => "json",
    }
}

fn reading_is_fresh(reading: &Option<SensorReading>, policy: &SensorFreshnessPolicy) -> bool {
    reading
        .as_ref()
        .and_then(|reading| reading_age(reading).ok())
        .is_some_and(|age| age <= policy.max_age)
}

fn sensor_read_result(
    definition: Option<SensorDefinition>,
    reading: Option<SensorReading>,
    policy: &SensorFreshnessPolicy,
    refreshed: bool,
    warning: Option<String>,
) -> SensorReadResult {
    let age = reading
        .as_ref()
        .and_then(|reading| reading_age(reading).ok());
    let fresh = age.is_some_and(|age| age <= policy.max_age);
    let stale = age.is_some_and(|age| age > policy.max_age);
    let age_ms = age.map(|age| age.as_millis().min(i64::MAX as u128) as i64);

    SensorReadResult {
        definition,
        reading,
        fresh,
        stale,
        refreshed,
        age_ms,
        warning,
    }
}

fn reading_age(reading: &SensorReading) -> anyhow::Result<Duration> {
    (Utc::now() - reading.timestamp)
        .to_std()
        .map_err(|_| anyhow::anyhow!("sensor reading timestamp is in the future"))
}

fn db_datetime_to_utc(value: DateTimeWithTimeZone) -> DateTime<Utc> {
    value.with_timezone(&Utc)
}

fn db_opt_datetime_to_utc(value: Option<DateTimeWithTimeZone>) -> Option<DateTime<Utc>> {
    value.map(|dt| dt.with_timezone(&Utc))
}

fn reading_from_row(row: &sea_orm::QueryResult) -> anyhow::Result<SensorReading> {
    let recorded_at: DateTimeWithTimeZone = row.try_get("", "recorded_at")?;
    Ok(SensorReading {
        sensor_id: row.try_get("", "sensor_id")?,
        value: row.try_get("", "value")?,
        quality: row.try_get("", "quality")?,
        timestamp: db_datetime_to_utc(recorded_at),
    })
}

fn definition_from_row(row: &sea_orm::QueryResult) -> anyhow::Result<SensorDefinition> {
    let registered_at: DateTimeWithTimeZone = row.try_get("", "registered_at")?;
    let last_seen_at: Option<DateTimeWithTimeZone> = row.try_get("", "last_seen_at")?;
    Ok(SensorDefinition {
        sensor_id: row.try_get("", "sensor_id")?,
        display_name: row.try_get("", "display_name")?,
        source: row.try_get("", "source")?,
        unit: row.try_get("", "unit")?,
        value_type: row.try_get("", "value_type")?,
        metadata: row.try_get("", "metadata")?,
        registered_at: db_datetime_to_utc(registered_at),
        last_seen_at: db_opt_datetime_to_utc(last_seen_at),
    })
}

async fn load_sensor_definitions(db: &DatabaseConnection) -> anyhow::Result<Vec<SensorDefinition>> {
    let rows = db
        .query_all(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT sensor_id, display_name, source, unit, value_type, metadata,
                   registered_at, last_seen_at
            FROM sensor_definitions
            ORDER BY sensor_id
            "#,
        ))
        .await?;
    rows.iter().map(definition_from_row).collect()
}

async fn load_sensor_definitions_by_source(
    db: &DatabaseConnection,
    source: &str,
) -> anyhow::Result<Vec<SensorDefinition>> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT sensor_id, display_name, source, unit, value_type, metadata,
                   registered_at, last_seen_at
            FROM sensor_definitions
            WHERE source = $1
            ORDER BY sensor_id
            "#,
            vec![source.to_string().into()],
        ))
        .await?;
    rows.iter().map(definition_from_row).collect()
}

async fn get_sensor_definition(
    db: &DatabaseConnection,
    sensor_id: &str,
) -> anyhow::Result<Option<SensorDefinition>> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT sensor_id, display_name, source, unit, value_type, metadata,
                   registered_at, last_seen_at
            FROM sensor_definitions
            WHERE sensor_id = $1
            "#,
            vec![sensor_id.to_string().into()],
        ))
        .await?;
    row.as_ref().map(definition_from_row).transpose()
}

async fn upsert_sensor_definition(
    db: &DatabaseConnection,
    def: &SensorDefinition,
) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"
        INSERT INTO sensor_definitions
            (sensor_id, display_name, source, unit, value_type, metadata,
             registered_at, last_seen_at)
        VALUES
            ($1, $2, $3, $4, $5, $6::jsonb, $7::timestamptz,
             NULLIF($8, '')::timestamptz)
        ON CONFLICT (sensor_id) DO UPDATE SET
            display_name = EXCLUDED.display_name,
            source = EXCLUDED.source,
            unit = EXCLUDED.unit,
            value_type = EXCLUDED.value_type,
            metadata = EXCLUDED.metadata,
            last_seen_at = COALESCE(EXCLUDED.last_seen_at, sensor_definitions.last_seen_at)
        "#,
        vec![
            def.sensor_id.clone().into(),
            def.display_name.clone().into(),
            def.source.clone().into(),
            def.unit.clone().into(),
            def.value_type.clone().into(),
            def.metadata.clone().into(),
            def.registered_at.to_rfc3339().into(),
            def.last_seen_at
                .map(|value| value.to_rfc3339())
                .unwrap_or_default()
                .into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn delete_sensor_definition(
    db: &DatabaseConnection,
    sensor_id: &str,
) -> anyhow::Result<bool> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM sensor_definitions WHERE sensor_id = $1",
            vec![sensor_id.to_string().into()],
        ))
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn insert_sensor_reading(
    db: &DatabaseConnection,
    reading: &SensorReading,
) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"
        INSERT INTO sensor_readings (sensor_id, value, quality, recorded_at)
        VALUES ($1, $2::jsonb, $3, $4::timestamptz)
        "#,
        vec![
            reading.sensor_id.clone().into(),
            reading.value.clone().into(),
            reading.quality.into(),
            reading.timestamp.to_rfc3339().into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn latest_sensor_reading(
    db: &DatabaseConnection,
    sensor_id: &str,
) -> anyhow::Result<Option<SensorReading>> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT sensor_id, value, quality, recorded_at
            FROM sensor_readings
            WHERE sensor_id = $1
            ORDER BY recorded_at DESC, id DESC
            LIMIT 1
            "#,
            vec![sensor_id.to_string().into()],
        ))
        .await?;
    row.as_ref().map(reading_from_row).transpose()
}

async fn sensor_ids_for_query(
    db: &DatabaseConnection,
    q: &SensorQuery,
) -> anyhow::Result<Vec<String>> {
    if let Some(sensor_id) = &q.sensor_id {
        return Ok(vec![sensor_id.clone()]);
    }

    let (sql, values) = if let Some(source) = &q.source {
        (
            "SELECT sensor_id FROM sensor_definitions WHERE source = $1 ORDER BY sensor_id",
            vec![source.clone().into()],
        )
    } else {
        (
            "SELECT sensor_id FROM sensor_definitions ORDER BY sensor_id",
            Vec::new(),
        )
    };

    let rows = db
        .query_all(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await?;
    rows.iter()
        .map(|row| row.try_get("", "sensor_id").map_err(Into::into))
        .collect()
}

async fn query_sensor_readings(
    db: &DatabaseConnection,
    q: &SensorQuery,
) -> anyhow::Result<Vec<SensorReading>> {
    let sensor_ids = sensor_ids_for_query(db, q).await?;
    let mut out = Vec::new();

    for sensor_id in sensor_ids {
        if q.latest_only {
            if let Some(reading) = latest_sensor_reading(db, &sensor_id).await? {
                out.push(reading);
            }
        } else {
            let remaining = q.limit.map(|limit| limit.saturating_sub(out.len()));
            if matches!(remaining, Some(0)) {
                break;
            }
            out.extend(query_sensor_readings_for_sensor(db, &sensor_id, q, remaining).await?);
        }

        if let Some(limit) = q.limit
            && out.len() >= limit
        {
            out.truncate(limit);
            break;
        }
    }

    Ok(out)
}

async fn query_sensor_readings_for_sensor(
    db: &DatabaseConnection,
    sensor_id: &str,
    q: &SensorQuery,
    limit_override: Option<usize>,
) -> anyhow::Result<Vec<SensorReading>> {
    let mut sql = String::from(
        r#"
        SELECT sensor_id, value, quality, recorded_at
        FROM sensor_readings
        WHERE sensor_id = $1
        "#,
    );
    let mut values = vec![sensor_id.to_string().into()];
    let mut idx = 2;

    if let Some(from) = q.from {
        sql.push_str(&format!(" AND recorded_at >= ${idx}::timestamptz"));
        values.push(from.to_rfc3339().into());
        idx += 1;
    }
    if let Some(to) = q.to {
        sql.push_str(&format!(" AND recorded_at <= ${idx}::timestamptz"));
        values.push(to.to_rfc3339().into());
    }

    sql.push_str(" ORDER BY recorded_at DESC, id DESC");
    if let Some(limit) = limit_override.or(q.limit) {
        sql.push_str(&format!(" LIMIT {}", limit.min(10_000)));
    }

    let rows = db
        .query_all(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await?;
    rows.iter().map(reading_from_row).collect()
}

async fn count_sensor_readings(db: &DatabaseConnection, sensor_id: &str) -> anyhow::Result<usize> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*)::bigint AS count FROM sensor_readings WHERE sensor_id = $1",
            vec![sensor_id.to_string().into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(0);
    };
    let count: i64 = row.try_get("", "count")?;
    Ok(count.max(0) as usize)
}

async fn list_sensor_summaries(
    db: &DatabaseConnection,
    offline_threshold: Duration,
) -> anyhow::Result<Vec<SensorSummary>> {
    let definitions = load_sensor_definitions(db).await?;
    let now = Utc::now();
    let mut out = Vec::with_capacity(definitions.len());

    for def in definitions {
        let latest = latest_sensor_reading(db, &def.sensor_id).await?;
        let reading_count = count_sensor_readings(db, &def.sensor_id).await?;
        let online = def.last_seen_at.is_some_and(|last_seen| {
            (now - last_seen).to_std().unwrap_or(Duration::MAX) < offline_threshold
        });

        out.push(SensorSummary {
            sensor_id: def.sensor_id,
            display_name: def.display_name,
            unit: def.unit,
            latest_value: latest.as_ref().map(|reading| reading.value.clone()),
            latest_timestamp: latest.map(|reading| reading.timestamp),
            reading_count,
            online,
        });
    }

    Ok(out)
}

async fn cleanup_db_readings(
    db: &DatabaseConnection,
    retention_duration: Duration,
) -> anyhow::Result<()> {
    let cutoff = Utc::now()
        - chrono::Duration::from_std(retention_duration).unwrap_or(chrono::Duration::hours(24));
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM sensor_readings WHERE recorded_at < $1::timestamptz",
        vec![cutoff.to_rfc3339().into()],
    ))
    .await?;
    Ok(())
}
