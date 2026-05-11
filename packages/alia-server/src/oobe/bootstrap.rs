//! OOBE bootstrap logic: detection, CLI key handler, config generation.

use std::io::Read;
use std::path::{Path, PathBuf};

use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig};
use tokio::sync::watch;

/// Result of the OOBE setup flow.
#[derive(Clone)]
pub enum SetupResult {
    /// Config was written; return the path to load.
    ConfigReady(PathBuf),
    /// User cancelled (Ctrl-C).
    Cancelled,
}

/// Check if any config file exists in the standard search locations.
pub fn needs_setup() -> bool {
    // If explicitly set, check if the file actually exists
    if let Ok(p) = std::env::var("ALIA_CONFIG_PATH") {
        return !Path::new(&p).exists();
    }

    let candidates = [
        "config/server.toml".to_string(),
        dirs::config_dir()
            .map(|d| {
                d.join("alia")
                    .join("server.toml")
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default(),
        "/etc/alia/server.toml".to_string(),
    ];

    !candidates
        .iter()
        .any(|c| !c.is_empty() && Path::new(c).exists())
}

/// Embedded example config (same content as config/server.example.toml).
/// We embed it at compile time so the binary is self-contained.
const EXAMPLE_CONFIG: &str = include_str!("../../../../config/server.example.toml");

/// Determine the best path to write the config file.
/// Respects ALIA_CONFIG_PATH if set, then tries ./config/, then XDG.
fn default_config_target() -> PathBuf {
    if let Ok(p) = std::env::var("ALIA_CONFIG_PATH") {
        return PathBuf::from(p);
    }

    // If we're in a directory that has `config/` (dev layout), use it.
    let local = PathBuf::from("config/server.toml");
    if local.parent().map_or(false, |p| p.exists()) {
        return local;
    }

    // Otherwise use XDG config dir
    if let Some(dir) = dirs::config_dir() {
        return dir.join("alia").join("server.toml");
    }

    local
}

/// Write the example config to disk.
pub fn release_example_config(target: &Path) -> anyhow::Result<PathBuf> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, EXAMPLE_CONFIG)?;
    Ok(target.to_path_buf())
}

/// Discover all usable local IP addresses by enumerating network interfaces.
/// Filters out virtual/tunnel interfaces and applies IP eligibility rules.
fn local_ips() -> Vec<std::net::IpAddr> {
    let Ok(interfaces) = NetworkInterface::show() else {
        return vec![];
    };

    interfaces
        .into_iter()
        .filter(|iface| {
            let name = iface.name.to_lowercase();

            let is_virtual =
                // --- VMware ---
                name.contains("vmnet") || name.contains("vmware") || name.contains("vmnic") ||
                // --- VirtualBox ---
                name.contains("vboxnet") || name.contains("virtualbox") ||
                // --- Hyper-V & Windows Virtual Adapters ---
                name.contains("vethernet") || name.contains("hyper-v") ||
                // --- Docker & Container Networking ---
                name.contains("docker") || name.contains("veth") || name.contains("br-") || 
                name.contains("cni") || name.contains("flannel") ||
                // --- VPNs & Tunnels ---
                name.contains("utun") || name.contains("tun") || name.contains("tap") || 
                name.contains("wg") || name.contains("tailscale") || name.contains("zerotier") || 
                name.starts_with("zt") || name.contains("ppp") ||
                // --- Apple Specific (AirDrop/Sidecar) ---
                name.contains("awdl") || name.contains("llw");

            !is_virtual
        })
        .flat_map(|iface| iface.addr)
        .filter_map(|addr| {
            let ip = match addr {
                Addr::V4(v4) => std::net::IpAddr::V4(v4.ip),
                Addr::V6(v6) => std::net::IpAddr::V6(v6.ip),
            };

            if is_usable_ip(&ip) { Some(ip) } else { None }
        })
        .collect()
}

/// Filter function: keep only usable/real IPs.
fn is_usable_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            // Standard checks
            let is_standard = !v4.is_loopback() && !v4.is_unspecified() && !v4.is_broadcast();

            // Filter out 198.18.0.0/15 (often used by proxy tools like Clash/V2Ray)
            let is_proxy_range = v4.octets()[0] == 198 && (v4.octets()[1] & 0xfe) == 18;

            is_standard && !is_proxy_range
        }
        std::net::IpAddr::V6(v6) => {
            // Keep IPv6 except loopback, link-local (fe80::), and unique-local (fd00::)
            !v6.is_loopback()
                && !v6.is_unicast_link_local()
                && !v6.is_unique_local()
                && !v6.is_multicast()
        }
    }
}

/// Print the OOBE banner to the terminal.
pub fn print_banner(port: u16) {
    let mut ips = local_ips();
    ips.sort_by_key(|ip| (ip.is_ipv6(), ip.to_string())); // IPv4 first, then IPv6, sorted
    ips.dedup(); // Remove duplicates

    let ip_line = if ips.is_empty() {
        format!("  http://localhost:{port}")
    } else {
        ips.iter()
            .map(|ip| format!("http://{ip}:{port}"))
            .chain(std::iter::once(format!("http://localhost:{port}")))
            .collect::<Vec<_>>()
            .join("\n  ")
    };

    eprintln!();
    eprintln!("════════════════════════════════════════════════════════════");
    eprintln!("               ✦  Alia — First Run Setup  ✦");
    eprintln!("════════════════════════════════════════════════════════════");
    eprintln!();
    eprintln!("  No configuration file found.");
    eprintln!();
    eprintln!("  → Connect alia-frontend to server for the interactive setup wizard:\n");
    eprintln!("  list of addresses reported by your system:");
    eprintln!("  {ip_line}");
    eprintln!();
    eprintln!("  → Or press [c] + Enter to generate a config file");
    eprintln!("    directly and edit it manually.");
    eprintln!();
    eprintln!("  → Press [q] + Enter or Ctrl-C to quit.");
    eprintln!();
    eprintln!("════════════════════════════════════════════════════════════");
    eprintln!();
}

/// Spawn a blocking thread that reads single-key CLI input.
/// Sends `SetupResult` through the watch channel when complete.
pub fn spawn_cli_listener(tx: watch::Sender<Option<SetupResult>>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut buf = [0u8; 16];

        loop {
            match stdin.lock().read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let input = std::str::from_utf8(&buf[..n])
                        .unwrap_or("")
                        .trim()
                        .to_lowercase();

                    match input.as_str() {
                        "c" => {
                            let target = default_config_target();
                            match release_example_config(&target) {
                                Ok(path) => {
                                    eprintln!();
                                    eprintln!("✓ Config file written to: {}", path.display());
                                    eprintln!("Edit the file, then restart the server.");
                                    eprintln!();
                                    let _ = tx.send(Some(SetupResult::ConfigReady(path)));
                                    return;
                                }
                                Err(e) => {
                                    eprintln!("✗ Failed to write config: {e}");
                                    eprintln!("Try running with appropriate permissions.");
                                }
                            }
                        }
                        "q" => {
                            let _ = tx.send(Some(SetupResult::Cancelled));
                            return;
                        }
                        _ => {}
                    }
                }
                Err(_) => break,
            }
        }
    });
}
