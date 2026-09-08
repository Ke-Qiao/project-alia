use std::collections::HashMap;
use std::net::IpAddr;

use anyhow::Context;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig};

use crate::config::ServerConfig;

const DEFAULT_SERVER_SERVICE: &str = "_alia-server._tcp";

#[derive(Debug, Clone)]
pub struct ServerDiscoveryAnnouncement {
    pub enabled: bool,
    pub service_name: String,
    pub instance_name: String,
    pub server_id: String,
    pub mode: String,
    pub port: u16,
    pub endpoint: String,
    pub properties: HashMap<String, String>,
}

impl ServerDiscoveryAnnouncement {
    pub fn from_config(config: &ServerConfig) -> Self {
        let endpoint = advertised_endpoint(
            config.server.host.as_str(),
            config.server.port,
            config.server.public_endpoint.as_deref(),
        );
        let instance_name = config
            .server
            .discovery_instance_name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&config.server.id)
            .to_string();

        let mut properties = base_properties(&config.server.id, "runtime", &endpoint);
        properties.insert("api".to_string(), "/api/v1".to_string());
        properties.insert("health".to_string(), "/health".to_string());
        properties.insert("scalar".to_string(), "/scalar".to_string());

        Self {
            enabled: config.server.discovery_enabled,
            service_name: normalize_service_type(&config.server.discovery_service_name),
            instance_name,
            server_id: config.server.id.clone(),
            mode: "runtime".to_string(),
            port: config.server.port,
            endpoint,
            properties,
        }
    }

    pub fn oobe(port: u16) -> Self {
        let endpoint = advertised_endpoint("0.0.0.0", port, None);
        let mut properties = base_properties("alia-setup", "oobe", &endpoint);
        properties.insert("setup".to_string(), "/api/v1/setup/status".to_string());

        Self {
            enabled: true,
            service_name: normalize_service_type(DEFAULT_SERVER_SERVICE),
            instance_name: "Alia Setup".to_string(),
            server_id: "alia-setup".to_string(),
            mode: "oobe".to_string(),
            port,
            endpoint,
            properties,
        }
    }
}

pub struct ServerDiscoveryAdvertisement {
    cancel: tokio_util::sync::CancellationToken,
    cleanup: tokio::sync::Mutex<AdvertisementCleanup>,
}

struct AdvertisementCleanup {
    task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    result: Option<Result<(), String>>,
}

impl ServerDiscoveryAdvertisement {
    pub async fn start(announcement: ServerDiscoveryAnnouncement) -> anyhow::Result<Option<Self>> {
        if !announcement.enabled {
            return Ok(None);
        }

        let hostname = mdns_hostname(&announcement.server_id);
        let service_info = ServiceInfo::new(
            &announcement.service_name,
            &announcement.instance_name,
            &hostname,
            "",
            announcement.port,
            announcement.properties,
        )
        .context("building server mDNS service info")?
        .enable_addr_auto();

        let fullname = service_info.get_fullname().to_string();
        let daemon = ServiceDaemon::new().context("creating server mDNS daemon")?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let cleanup_cancel = cancel.clone();
        let cleanup_daemon = daemon.clone();
        let cleanup = tokio::spawn(async move {
            cleanup_cancel.cancelled().await;
            // Request daemon shutdown even when unregister fails. Its ACK is
            // the native library's evidence; it exposes no OS-thread join.
            let unregister = cleanup_daemon.unregister(&fullname);
            let shutdown = alia_transport::shutdown_discovery_daemon(&cleanup_daemon).await;
            let mut errors = Vec::new();
            match unregister {
                Ok(ack) => {
                    if let Err(error) = ack.recv_async().await {
                        errors.push(format!("unregister acknowledgement: {error}"));
                    }
                }
                Err(error) => errors.push(format!("unregister: {error}")),
            }
            if let Err(error) = shutdown {
                errors.push(format!("daemon shutdown: {error:#}"));
            }
            anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
            Ok(())
        });
        let advertisement = Self {
            cancel,
            cleanup: tokio::sync::Mutex::new(AdvertisementCleanup {
                task: Some(cleanup),
                result: None,
            }),
        };
        if let Err(error) = daemon.register(service_info) {
            let cleanup = advertisement.shutdown().await;
            return Err(anyhow::anyhow!(
                "registering server mDNS service: {error}; cleanup: {cleanup:?}"
            ));
        }

        tracing::info!(
            service = %announcement.service_name,
            instance = %announcement.instance_name,
            endpoint = %announcement.endpoint,
            mode = %announcement.mode,
            "server discovery advertised"
        );

        Ok(Some(advertisement))
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.cancel.cancel();
        let mut cleanup = self.cleanup.lock().await;
        if let Some(task) = cleanup.task.as_mut() {
            let result = task
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result);
            cleanup.task.take();
            cleanup.result = Some(result.map_err(|error| format!("{error:#}")));
        }
        match cleanup.result.as_ref().expect("discovery cleanup retained") {
            Ok(()) => Ok(()),
            Err(error) => anyhow::bail!("{error}"),
        }
    }
}

impl Drop for ServerDiscoveryAdvertisement {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn base_properties(server_id: &str, mode: &str, endpoint: &str) -> HashMap<String, String> {
    HashMap::from([
        ("schema".to_string(), "alia.server.discovery.v1".to_string()),
        ("kind".to_string(), "alia-server".to_string()),
        ("id".to_string(), server_id.to_string()),
        ("name".to_string(), server_id.to_string()),
        ("mode".to_string(), mode.to_string()),
        ("version".to_string(), env!("CARGO_PKG_VERSION").to_string()),
        ("transport".to_string(), "http".to_string()),
        ("endpoint".to_string(), endpoint.to_string()),
    ])
}

pub fn normalize_service_type(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('.');
    if trimmed.ends_with(".local") {
        format!("{trimmed}.")
    } else {
        format!("{trimmed}.local.")
    }
}

pub fn advertised_endpoint(host: &str, port: u16, public_endpoint: Option<&str>) -> String {
    if let Some(endpoint) = public_endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return endpoint.trim_end_matches('/').to_string();
    }

    let host = host.trim();
    let advertised_host = if host.is_empty() || is_wildcard_host(host) {
        local_ips()
            .into_iter()
            .find(|ip| ip.is_ipv4())
            .or_else(|| local_ips().into_iter().next())
            .map(format_ip_for_url)
            .unwrap_or_else(|| "localhost".to_string())
    } else {
        format_host_for_url(host)
    };

    format!("http://{advertised_host}:{port}")
}

fn is_wildcard_host(host: &str) -> bool {
    matches!(host, "0.0.0.0" | "::" | "[::]")
}

fn format_host_for_url(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn format_ip_for_url(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    }
}

fn mdns_hostname(server_id: &str) -> String {
    let mut host = server_id
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' => ch.to_ascii_lowercase(),
            _ => '-',
        })
        .collect::<String>();

    if host.is_empty() {
        host = "alia-server".to_string();
    }

    format!("{host}.local.")
}

fn local_ips() -> Vec<IpAddr> {
    let Ok(interfaces) = NetworkInterface::show() else {
        return vec![];
    };

    interfaces
        .into_iter()
        .filter(|iface| {
            let name = iface.name.to_lowercase();
            !(name.contains("vmnet")
                || name.contains("vmware")
                || name.contains("vboxnet")
                || name.contains("virtualbox")
                || name.contains("docker")
                || name.contains("veth")
                || name.contains("br-")
                || name.contains("cni")
                || name.contains("flannel")
                || name.contains("utun")
                || name.contains("tun")
                || name.contains("tap")
                || name.contains("wg")
                || name.contains("tailscale")
                || name.contains("zerotier")
                || name.starts_with("zt")
                || name.contains("ppp")
                || name.contains("awdl")
                || name.contains("llw"))
        })
        .flat_map(|iface| iface.addr)
        .filter_map(|addr| match addr {
            Addr::V4(v4) => Some(IpAddr::V4(v4.ip)),
            Addr::V6(v6) => Some(IpAddr::V6(v6.ip)),
        })
        .filter(is_usable_ip)
        .collect()
}

fn is_usable_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let is_standard = !v4.is_loopback() && !v4.is_unspecified() && !v4.is_broadcast();
            let is_proxy_range = v4.octets()[0] == 198 && (v4.octets()[1] & 0xfe) == 18;
            is_standard && !is_proxy_range
        }
        IpAddr::V6(v6) => {
            !v6.is_loopback()
                && !v6.is_unicast_link_local()
                && !v6.is_unique_local()
                && !v6.is_multicast()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_service_type() {
        assert_eq!(
            normalize_service_type("_alia-server._tcp"),
            "_alia-server._tcp.local."
        );
        assert_eq!(
            normalize_service_type("_alia-server._tcp.local."),
            "_alia-server._tcp.local."
        );
    }

    #[test]
    fn uses_public_endpoint_when_configured() {
        assert_eq!(
            advertised_endpoint("0.0.0.0", 3000, Some("http://192.168.1.10:3000/")),
            "http://192.168.1.10:3000"
        );
    }
}
