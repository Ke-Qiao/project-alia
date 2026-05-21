use std::collections::HashMap;
use std::sync::Arc;

#[cfg(target_os = "linux")]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(target_os = "linux")]
use std::process::Stdio;
#[cfg(target_os = "linux")]
use std::time::Duration;

use anyhow::{Context, Result, bail};
#[cfg(target_os = "linux")]
use futures::FutureExt;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock};

use super::context::ModuleContext;
use super::lifecycle::ModuleLifecycle;
use super::traits::{DeviceInfo, DiscoveryBackend, DiscoveryEndpoint, DiscoveryMetadata};
use crate::config::{DiscoveryConfig, IdentityConfig};

const DEFAULT_GRPC_PORT: u16 = 50051;
const BLE_SERVICE_UUID: &str = "3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f1";
const BLE_NETWORK_STATUS_UUID: &str = "3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f2";
const BLE_NETWORK_CONFIG_UUID: &str = "3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f3";

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
                tracing::warn!(
                    error = %format!("{error:#}"),
                    "failed to refresh auxiliary BLE beacon"
                );
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
    network_status: Arc<Mutex<NetworkProvisionStatus>>,
    state: RwLock<Option<BleState>>,
}

#[cfg(target_os = "linux")]
struct BleState {
    _session: bluer::Session,
    adapter: bluer::Adapter,
    handle: bluer::adv::AdvertisementHandle,
    _gatt: bluer::gatt::local::ApplicationHandle,
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
            network_status: Arc::new(Mutex::new(NetworkProvisionStatus::idle())),
            state: RwLock::new(None),
        }
    }

    #[cfg(target_os = "linux")]
    async fn start_advertisement(&self) -> Result<()> {
        let session = bluer::Session::new().await.with_context(|| {
            bluez_runtime_hint("creating BlueZ D-Bus session for BLE advertisement")
        })?;
        let adapter = session
            .default_adapter()
            .await
            .with_context(|| bluez_runtime_hint("loading default BLE adapter"))?;

        adapter
            .set_powered(true)
            .await
            .with_context(|| bluez_runtime_hint("powering BLE adapter"))?;

        let advertisement = self.build_advertisement().await?;
        let handle = adapter
            .advertise(advertisement)
            .await
            .with_context(|| bluez_runtime_hint("registering BLE advertisement"))?;
        let gatt = self.start_gatt_service(&adapter).await?;

        *self.state.write().await = Some(BleState {
            _session: session,
            adapter,
            handle,
            _gatt: gatt,
        });

        tracing::info!(
            device_id = %self.identity.device_id,
            "BLE discovery beacon and provisioning GATT service started"
        );
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    async fn start_advertisement(&self) -> Result<()> {
        anyhow::bail!("BLE discovery requires Linux with BlueZ")
    }

    #[cfg(target_os = "linux")]
    async fn build_advertisement(&self) -> Result<bluer::adv::Advertisement> {
        let service_uuid = uuid::Uuid::parse_str(BLE_SERVICE_UUID).context("parsing BLE UUID")?;

        let mut service_uuids = BTreeSet::new();
        service_uuids.insert(service_uuid);

        let timeout = (self.config.ble_provision_timeout_sec > 0)
            .then(|| Duration::from_secs(self.config.ble_provision_timeout_sec));

        Ok(bluer::adv::Advertisement {
            advertisement_type: bluer::adv::Type::Peripheral,
            service_uuids,
            discoverable: Some(true),
            timeout,
            ..Default::default()
        })
    }

    #[cfg(target_os = "linux")]
    async fn start_gatt_service(
        &self,
        adapter: &bluer::Adapter,
    ) -> Result<bluer::gatt::local::ApplicationHandle> {
        use bluer::gatt::local::{
            Application, Characteristic, CharacteristicRead, CharacteristicWrite,
            CharacteristicWriteMethod, ReqError, Service,
        };

        let service_uuid = uuid::Uuid::parse_str(BLE_SERVICE_UUID)
            .context("parsing BLE provisioning service UUID")?;
        let status_uuid = uuid::Uuid::parse_str(BLE_NETWORK_STATUS_UUID)
            .context("parsing BLE network status characteristic UUID")?;
        let config_uuid = uuid::Uuid::parse_str(BLE_NETWORK_CONFIG_UUID)
            .context("parsing BLE network config characteristic UUID")?;

        let status_read = self.network_status.clone();
        let status_write = self.network_status.clone();

        let app = Application {
            services: vec![Service {
                uuid: service_uuid,
                primary: true,
                characteristics: vec![
                    Characteristic {
                        uuid: status_uuid,
                        read: Some(CharacteristicRead {
                            read: true,
                            fun: Box::new(move |_| {
                                let status = status_read.clone();
                                async move {
                                    let status = status.lock().await.clone();
                                    Ok(network_status_bytes(&status))
                                }
                                .boxed()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    Characteristic {
                        uuid: config_uuid,
                        write: Some(CharacteristicWrite {
                            write: true,
                            write_without_response: true,
                            method: CharacteristicWriteMethod::Fun(Box::new(move |value, _| {
                                let status = status_write.clone();
                                async move {
                                    let request = parse_network_provision_request(&value).map_err(
                                        |error| {
                                            tracing::warn!(
                                                error = %error,
                                                "invalid BLE network provisioning payload"
                                            );
                                            ReqError::InvalidValueLength
                                        },
                                    )?;

                                    tokio::spawn(async move {
                                        apply_network_provisioning(request, status).await;
                                    });

                                    Ok(())
                                }
                                .boxed()
                            })),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        adapter
            .serve_gatt_application(app)
            .await
            .with_context(|| bluez_runtime_hint("registering BLE provisioning GATT service"))
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
                .with_context(|| bluez_runtime_hint("refreshing BLE advertisement"))?;
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

#[cfg(target_os = "linux")]
fn bluez_runtime_hint(step: &str) -> String {
    format!(
        "{step}; on Raspberry Pi verify BlueZ is installed and running (`sudo systemctl enable --now bluetooth`), the controller is not rfkill-blocked (`rfkill unblock bluetooth`), `bluetoothctl show` lists a powered adapter, and LE advertising is available. If advertisement registration fails on older BlueZ builds, start bluetoothd with experimental features enabled."
    )
}

#[derive(Debug, Clone, Serialize)]
struct NetworkProvisionStatus {
    schema: &'static str,
    state: &'static str,
    message: String,
    updated_at: String,
}

impl NetworkProvisionStatus {
    fn idle() -> Self {
        Self::new("idle", "awaiting Wi-Fi credentials")
    }

    fn new(state: &'static str, message: impl Into<String>) -> Self {
        Self {
            schema: "alia.embedded.network_provisioning.v1",
            state,
            message: message.into(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct NetworkProvisionRequest {
    ssid: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    interface: Option<String>,
    #[serde(default)]
    hidden: bool,
}

fn parse_network_provision_request(bytes: &[u8]) -> Result<NetworkProvisionRequest> {
    let request: NetworkProvisionRequest =
        serde_json::from_slice(bytes).context("parsing network provisioning JSON")?;
    validate_network_provision_request(&request)?;
    Ok(request)
}

fn validate_network_provision_request(request: &NetworkProvisionRequest) -> Result<()> {
    let ssid_len = request.ssid.as_bytes().len();
    if ssid_len == 0 || ssid_len > 32 {
        bail!("Wi-Fi SSID must be 1..=32 bytes");
    }

    if let Some(password) = request
        .password
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        let len = password.as_bytes().len();
        let is_hex_64 = len == 64 && password.chars().all(|ch| ch.is_ascii_hexdigit());
        if !(8..=63).contains(&len) && !is_hex_64 {
            bail!("WPA-PSK password must be 8..=63 bytes, or 64 hex characters");
        }
    }

    if let Some(interface) = request
        .interface
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        if !interface
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            bail!("network interface contains unsupported characters");
        }
    }

    if request.hidden {
        bail!("hidden Wi-Fi network provisioning is not supported by the BLE fast path yet");
    }

    Ok(())
}

fn network_status_bytes(status: &NetworkProvisionStatus) -> Vec<u8> {
    serde_json::to_vec(status).unwrap_or_else(|_| {
        br#"{"schema":"alia.embedded.network_provisioning.v1","state":"failed","message":"status serialization failed"}"#.to_vec()
    })
}

async fn set_network_status(
    status: &Arc<Mutex<NetworkProvisionStatus>>,
    state: &'static str,
    message: impl Into<String>,
) {
    *status.lock().await = NetworkProvisionStatus::new(state, message);
}

#[cfg(target_os = "linux")]
async fn apply_network_provisioning(
    request: NetworkProvisionRequest,
    status: Arc<Mutex<NetworkProvisionStatus>>,
) {
    set_network_status(
        &status,
        "applying",
        format!("connecting to {}", request.ssid),
    )
    .await;

    match connect_wifi_nmcli(&request).await {
        Ok(()) => {
            tracing::info!(ssid = %request.ssid, "BLE Wi-Fi provisioning applied");
            set_network_status(
                &status,
                "connected",
                format!("connected to {}", request.ssid),
            )
            .await;
        }
        Err(error) => {
            tracing::warn!(ssid = %request.ssid, error = %error, "BLE Wi-Fi provisioning failed");
            set_network_status(&status, "failed", error.to_string()).await;
        }
    }
}

#[cfg(target_os = "linux")]
async fn connect_wifi_nmcli(request: &NetworkProvisionRequest) -> Result<()> {
    let args = nmcli_wifi_connect_args(request)?;
    let output = Command::new("nmcli")
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .await
        .context("running nmcli for Wi-Fi provisioning")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("nmcli exited with status {}", output.status)
    };

    bail!("nmcli Wi-Fi connection failed: {detail}");
}

fn nmcli_wifi_connect_args(request: &NetworkProvisionRequest) -> Result<Vec<String>> {
    validate_network_provision_request(request)?;

    let mut args = vec![
        "--wait".to_string(),
        "30".to_string(),
        "device".to_string(),
        "wifi".to_string(),
        "connect".to_string(),
        request.ssid.trim().to_string(),
    ];

    if let Some(password) = request
        .password
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        args.push("password".to_string());
        args.push(password.to_string());
    }

    if let Some(interface) = request
        .interface
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        args.push("ifname".to_string());
        args.push(interface.to_string());
    }

    Ok(args)
}

#[cfg(not(target_os = "linux"))]
async fn apply_network_provisioning(
    _request: NetworkProvisionRequest,
    status: Arc<Mutex<NetworkProvisionStatus>>,
) {
    set_network_status(
        &status,
        "failed",
        "BLE Wi-Fi provisioning requires Linux with NetworkManager/nmcli",
    )
    .await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_network_provisioning_payload() {
        let request = parse_network_provision_request(
            br#"{"ssid":"AliaLab","password":"correct-horse","interface":"wlan0"}"#,
        )
        .unwrap();

        assert_eq!(request.ssid, "AliaLab");
        assert_eq!(request.password.as_deref(), Some("correct-horse"));
        assert_eq!(request.interface.as_deref(), Some("wlan0"));
    }

    #[test]
    fn builds_nmcli_wifi_connect_args_without_shell_interpolation() {
        let request = NetworkProvisionRequest {
            ssid: "Alia Lab".to_string(),
            password: Some("correct-horse".to_string()),
            interface: Some("wlan0".to_string()),
            hidden: false,
        };

        assert_eq!(
            nmcli_wifi_connect_args(&request).unwrap(),
            vec![
                "--wait",
                "30",
                "device",
                "wifi",
                "connect",
                "Alia Lab",
                "password",
                "correct-horse",
                "ifname",
                "wlan0",
            ]
        );
    }

    #[test]
    fn rejects_invalid_network_provisioning_payloads() {
        assert!(parse_network_provision_request(br#"{"ssid":""}"#).is_err());
        assert!(parse_network_provision_request(br#"{"ssid":"Alia","password":"short"}"#).is_err());
        assert!(
            parse_network_provision_request(
                br#"{"ssid":"Alia","password":"correct-horse","interface":"wlan0; reboot"}"#
            )
            .is_err()
        );
        assert!(
            parse_network_provision_request(
                br#"{"ssid":"Alia","password":"correct-horse","hidden":true}"#
            )
            .is_err()
        );
    }
}
