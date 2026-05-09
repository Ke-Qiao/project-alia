//! OOBE bootstrap logic: detection, CLI key handler, config generation.

use std::io::Read;
use std::path::{Path, PathBuf};

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
    if std::env::var("ALIA_CONFIG_PATH").is_ok() {
        // Explicitly set — don't enter OOBE even if file is missing,
        // because the user intended a specific path.
        return false;
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
/// Prefers `./config/server.toml`, falls back to `~/.config/alia/server.toml`.
fn default_config_target() -> PathBuf {
    // If we're in a directory that has `config/` (dev layout), use it.
    let local = PathBuf::from("config/server.toml");
    if local.parent().map_or(false, |p| p.exists()) {
        return local;
    }

    // Otherwise use XDG config dir
    if let Some(dir) = dirs::config_dir() {
        return dir.join("alia").join("server.toml");
    }

    // Last resort
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

/// Print the OOBE banner to the terminal.
pub fn print_banner(port: u16) {
    eprintln!();
    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║              ✦  Alia — First Run Setup  ✦                  ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║                                                            ║");
    eprintln!("║  No configuration file found.                              ║");
    eprintln!("║                                                            ║");
    eprintln!(
        "║  → Open http://localhost:{:<5} in your browser             ║",
        port
    );
    eprintln!("║    for the interactive setup wizard.                       ║");
    eprintln!("║                                                            ║");
    eprintln!("║  → Or press [c] + Enter to generate a config file         ║");
    eprintln!("║    directly and edit it manually.                          ║");
    eprintln!("║                                                            ║");
    eprintln!("║  → Press [q] + Enter or Ctrl-C to quit.                   ║");
    eprintln!("║                                                            ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
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
                                    eprintln!("  ✓ Config file written to: {}", path.display());
                                    eprintln!("  Edit the file, then restart the server.");
                                    eprintln!();
                                    let _ = tx.send(Some(SetupResult::ConfigReady(path)));
                                    return;
                                }
                                Err(e) => {
                                    eprintln!("  ✗ Failed to write config: {e}");
                                    eprintln!("  Try running with appropriate permissions.");
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
