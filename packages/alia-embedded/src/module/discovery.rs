use std::collections::HashMap;

#[cfg(target_os = "linux")]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(target_os = "linux")]
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::RwLock;

use super::context::ModuleContext;
use super::lifecycle::ModuleLifecycle;
use super::traits::{DeviceInfo, DiscoveryBackend, DiscoveryEndpoint, DiscoveryMetadata};
use crate::config::{DiscoveryConfig, IdentityConfig};

const DEFAULT_GRPC_PORT: u16 = 50051;
const BLE_SERVICE_UUID: &str = "3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f1";

pub struct MdnsDiscovery {
    config: DiscoveryConfig,
    identity: IdentityConfig,
    device_info: RwLock<DeviceInfo>,
    metadata: RwLock<DiscoveryMetadata>,
    daemon: RwLock<Option<mdns_sd::ServiceDaemon>>,
    service_fullname: RwLock<Option<String>>,
    ble: Option<BleAdvertiser>,
}

impl MdnsDiscovery {
    pub fn new(config: DiscoveryConfig, identity: IdentityConfig) -> Self {
        let device_info = default_device_info(&identity);
        let ble = config
            .ble_provision_enabled
            .then(|| BleAdvertiser::new(config.clone(), identity.clone()));

        Self {
            config,
            identity,
            device_info: RwLock::new(device_info),
            metadata: RwLock::new(default_metadata()),
            daemon: RwLock::new(None),
            service_fullname: RwLock::new(None),
            ble,
        }
    }

    async fn ensure_daemon(&self) -> Result<()> {
        if !self.config.mdns_enabled {
            return Ok(());
        }

        let mut daemon = self.daemon.write().await;
        if daemon.is_none() {
            *daemon = Some(mdns_sd::ServiceDaemon::new().context("creating mDNS daemon")?);
        }
        Ok(())
    }

    async fn reannounce(&self) -> Result<()> {
        if !self.config.mdns_enabled {
            return Ok(());
        }

        self.ensure_daemon().await?;

        let device_info = self.device_info.read().await.clone();
        let metadata = self.metadata.read().await.clone();
        let service_type = normalize_mdns_service_type(&self.config.mdns_service_name);
        let instance_name = self
            .config
            .mdns_instance_name
            .clone()
            .unwrap_or_else(|| self.identity.device_name.clone());
        let host_name = mdns_hostname(&self.identity.device_id);
        let properties = mdns_properties(&device_info, &metadata);

        let service_info = mdns_sd::ServiceInfo::new(
            &service_type,
            &instance_name,
            &host_name,
            "",
            device_info.port,
            properties,
        )
        .context("building mDNS service info")?
        .enable_addr_auto();

        let fullname = service_info.get_fullname().to_string();
        let daemon = self.daemon.read().await;
        daemon
            .as_ref()
            .context("mDNS daemon not initialized")?
            .register(service_info)
            .context("registering mDNS service")?;
        drop(daemon);

        *self.service_fullname.write().await = Some(fullname);
        tracing::info!(
            device_id = %device_info.device_id,
            status = %metadata.status,
            service_type = %service_type,
            "mDNS service announced"
        );
        Ok(())
    }

    async fn update_ble(&self) {
        if let Some(ble) = &self.ble {
            if let Err(error) = ble.reannounce().await {
                tracing::warn!(error = %error, "failed to refresh auxiliary BLE beacon");
            }
        }
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for MdnsDiscovery {
    fn name(&self) -> &'static str {
        "mdns_discovery"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> Result<()> {
        *self.device_info.write().await = default_device_info(&ctx.config.identity);
        Ok(())
    }

    async fn start(&self) -> Result<()> {
        self.reannounce().await?;
        self.update_ble().await;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.unregister().await
    }
}

#[tonic::async_trait]
impl DiscoveryBackend for MdnsDiscovery {
    async fn register_service(&self, info: &DeviceInfo) -> Result<()> {
        *self.device_info.write().await = info.clone();
        self.reannounce().await?;
        self.update_ble().await;
        Ok(())
    }

    async fn update_metadata(&self, metadata: &DiscoveryMetadata) -> Result<()> {
        *self.metadata.write().await = metadata.clone();
        self.reannounce().await?;
        self.update_ble().await;
        Ok(())
    }

    async fn unregister(&self) -> Result<()> {
        if let Some(ble) = &self.ble {
            ble.stop().await?;
        }

        let fullname = self.service_fullname.write().await.take();
        let daemon = self.daemon.write().await.take();

        if let Some(fullname) = fullname {
            if let Some(daemon) = daemon.as_ref() {
                let _ = daemon.unregister(&fullname);
            }
        }

        if let Some(daemon) = daemon {
            daemon.shutdown().context("shutting down mDNS daemon")?;
        }

        Ok(())
    }
}

pub struct BleDiscovery {
    advertiser: BleAdvertiser,
}

impl BleDiscovery {
    pub fn new(config: DiscoveryConfig, identity: IdentityConfig) -> Self {
        Self {
            advertiser: BleAdvertiser::new(config, identity),
        }
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for BleDiscovery {
    fn name(&self) -> &'static str {
        "ble_discovery"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> Result<()> {
        self.advertiser.init(ctx).await
    }

    async fn start(&self) -> Result<()> {
        self.advertiser.start().await
    }

    async fn stop(&self) -> Result<()> {
        self.advertiser.stop().await
    }
}

#[tonic::async_trait]
impl DiscoveryBackend for BleDiscovery {
    async fn register_service(&self, info: &DeviceInfo) -> Result<()> {
        self.advertiser.register_service(info).await
    }

    async fn update_metadata(&self, metadata: &DiscoveryMetadata) -> Result<()> {
        self.advertiser.update_metadata(metadata).await
    }

    async fn unregister(&self) -> Result<()> {
        self.advertiser.unregister().await
    }
}

struct BleAdvertiser {
    config: DiscoveryConfig,
    identity: IdentityConfig,
    device_info: RwLock<DeviceInfo>,
    metadata: RwLock<DiscoveryMetadata>,
    state: RwLock<Option<BleState>>,
}

#[cfg(target_os = "linux")]
struct BleState {
    _session: bluer::Session,
    adapter: bluer::Adapter,
    handle: bluer::adv::AdvertisementHandle,
}

#[cfg(not(target_os = "linux"))]
struct BleState;

impl BleAdvertiser {
    fn new(config: DiscoveryConfig, identity: IdentityConfig) -> Self {
        Self {
            config,
            identity: identity.clone(),
            device_info: RwLock::new(default_device_info(&identity)),
            metadata: RwLock::new(default_metadata()),
            state: RwLock::new(None),
        }
    }

    #[cfg(target_os = "linux")]
    async fn start_advertisement(&self) -> Result<()> {
        let session = bluer::Session::new()
            .await
            .context("creating BlueZ session for BLE advertisement")?;
        let adapter = session
            .default_adapter()
            .await
            .context("loading default BLE adapter")?;

        adapter
            .set_powered(true)
            .await
            .context("powering BLE adapter")?;

        let advertisement = self.build_advertisement().await?;
        let handle = adapter
            .advertise(advertisement)
            .await
            .context("registering BLE advertisement")?;

        *self.state.write().await = Some(BleState {
            _session: session,
            adapter,
            handle,
        });

        tracing::info!(
            device_id = %self.identity.device_id,
            "BLE discovery beacon started"
        );
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    async fn start_advertisement(&self) -> Result<()> {
        anyhow::bail!("BLE discovery requires Linux with BlueZ")
    }

    #[cfg(target_os = "linux")]
    async fn build_advertisement(&self) -> Result<bluer::adv::Advertisement> {
        let device_info = self.device_info.read().await.clone();
        let metadata = self.metadata.read().await.clone();
        let service_uuid = uuid::Uuid::parse_str(BLE_SERVICE_UUID).context("parsing BLE UUID")?;

        let payload = encode_ble_payload(&device_info, &metadata);
        let mut service_uuids = BTreeSet::new();
        service_uuids.insert(service_uuid);

        let mut service_data = BTreeMap::new();
        service_data.insert(service_uuid, payload);

        let timeout = (self.config.ble_provision_timeout_sec > 0)
            .then(|| Duration::from_secs(self.config.ble_provision_timeout_sec));

        Ok(bluer::adv::Advertisement {
            advertisement_type: bluer::adv::Type::Peripheral,
            service_uuids,
            service_data,
            discoverable: Some(true),
            local_name: Some(self.identity.device_name.clone()),
            timeout,
            ..Default::default()
        })
    }

    #[cfg(target_os = "linux")]
    async fn reannounce(&self) -> Result<()> {
        let advertisement = self.build_advertisement().await?;

        let mut state = self.state.write().await;
        if let Some(existing) = state.as_mut() {
            let new_handle = existing
                .adapter
                .advertise(advertisement)
                .await
                .context("refreshing BLE advertisement")?;
            existing.handle = new_handle;
            return Ok(());
        }

        drop(state);
        self.start_advertisement().await
    }

    #[cfg(not(target_os = "linux"))]
    async fn reannounce(&self) -> Result<()> {
        self.start_advertisement().await
    }

    async fn stop_advertising(&self) -> Result<()> {
        self.state.write().await.take();
        Ok(())
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for BleAdvertiser {
    fn name(&self) -> &'static str {
        "ble_advertiser"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> Result<()> {
        *self.device_info.write().await = default_device_info(&ctx.config.identity);
        Ok(())
    }

    async fn start(&self) -> Result<()> {
        if !self.config.ble_provision_enabled {
            tracing::info!("BLE discovery disabled by config");
            return Ok(());
        }

        self.start_advertisement().await
    }

    async fn stop(&self) -> Result<()> {
        self.stop_advertising().await
    }
}

#[tonic::async_trait]
impl DiscoveryBackend for BleAdvertiser {
    async fn register_service(&self, info: &DeviceInfo) -> Result<()> {
        *self.device_info.write().await = info.clone();
        self.reannounce().await
    }

    async fn update_metadata(&self, metadata: &DiscoveryMetadata) -> Result<()> {
        *self.metadata.write().await = metadata.clone();
        self.reannounce().await
    }

    async fn unregister(&self) -> Result<()> {
        self.stop_advertising().await
    }
}

fn default_device_info(identity: &IdentityConfig) -> DeviceInfo {
    DeviceInfo {
        device_id: identity.device_id.clone(),
        device_name: identity.device_name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        port: DEFAULT_GRPC_PORT,
        endpoints: vec![DiscoveryEndpoint {
            name: "grpc".to_string(),
            scheme: "grpc".to_string(),
            port: DEFAULT_GRPC_PORT,
            path: None,
            metadata: HashMap::from([("services".to_string(), "plugin,binding".to_string())]),
        }],
        capabilities: vec![
            "embedded.grpc".to_string(),
            "embedded.plugin_manager".to_string(),
            "embedded.binding".to_string(),
        ],
        attributes: HashMap::new(),
    }
}

fn default_metadata() -> DiscoveryMetadata {
    DiscoveryMetadata {
        status: "unbound".to_string(),
        server_id: None,
        capabilities: Vec::new(),
        attributes: HashMap::new(),
    }
}

pub fn normalize_mdns_service_type(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('.');
    if trimmed.ends_with(".local") {
        format!("{trimmed}.")
    } else {
        format!("{trimmed}.local.")
    }
}

fn mdns_hostname(device_id: &str) -> String {
    let mut host = device_id
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' => ch.to_ascii_lowercase(),
            _ => '-',
        })
        .collect::<String>();

    if host.is_empty() {
        host = "alia-embedded".to_string();
    }

    format!("{host}.local.")
}

fn mdns_properties(info: &DeviceInfo, metadata: &DiscoveryMetadata) -> HashMap<String, String> {
    let mut properties = HashMap::from([
        ("id".to_string(), info.device_id.clone()),
        ("name".to_string(), info.device_name.clone()),
        ("version".to_string(), info.version.clone()),
        ("transport".to_string(), "grpc".to_string()),
        ("port".to_string(), info.port.to_string()),
        ("status".to_string(), metadata.status.clone()),
    ]);

    if let Some(server_id) = &metadata.server_id {
        properties.insert("server".to_string(), server_id.clone());
    }

    let mut capabilities = info.capabilities.clone();
    capabilities.extend(metadata.capabilities.clone());
    capabilities.sort();
    capabilities.dedup();
    if !capabilities.is_empty() {
        properties.insert("caps".to_string(), capabilities.join(","));
    }

    for (key, value) in &info.attributes {
        properties.insert(format!("info.{key}"), value.clone());
    }
    for (key, value) in &metadata.attributes {
        properties.insert(key.clone(), value.clone());
    }

    properties.insert(
        "endpoint.count".to_string(),
        info.endpoints.len().to_string(),
    );
    for endpoint in &info.endpoints {
        let descriptor = match &endpoint.path {
            Some(path) => format!("{}:{}{}", endpoint.scheme, endpoint.port, path),
            None => format!("{}:{}", endpoint.scheme, endpoint.port),
        };
        properties.insert(format!("endpoint.{}", endpoint.name), descriptor);
        for (key, value) in &endpoint.metadata {
            properties.insert(format!("endpoint.{}.{}", endpoint.name, key), value.clone());
        }
    }

    properties
}

pub fn encode_ble_payload(info: &DeviceInfo, metadata: &DiscoveryMetadata) -> Vec<u8> {
    let mut payload = Vec::with_capacity(12);
    payload.push(1);
    payload.push(status_code(&metadata.status));
    payload.extend_from_slice(&info.port.to_be_bytes());
    payload.extend_from_slice(&stable_fingerprint(&info.device_id));
    payload.extend_from_slice(
        &metadata
            .server_id
            .as_deref()
            .map(stable_fingerprint)
            .unwrap_or([0, 0, 0, 0]),
    );
    payload
}

fn status_code(status: &str) -> u8 {
    match status {
        "bound" => 1,
        "handoff_pending" => 2,
        _ => 0,
    }
}

fn stable_fingerprint(value: &str) -> [u8; 4] {
    let mut hash = 0x811c9dc5u32;
    for byte in value.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash.to_be_bytes()
}
