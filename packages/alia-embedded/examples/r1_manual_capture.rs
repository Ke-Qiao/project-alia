//! Model-free R1 observation; hardware is opened only by the explicit `run` command.
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};

// Share the wire parser/owner without extending the production wakeword API.
#[cfg(target_os = "linux")]
#[path = "r1_manual_capture/linux.rs"]
mod linux;
#[cfg(any(target_os = "linux", test))]
#[path = "r1_manual_capture/probe.rs"]
mod probe;
#[allow(dead_code)]
#[path = "../src/module/r1/protocol.rs"]
mod protocol;
#[cfg(target_os = "linux")]
#[path = "../src/module/r1/serial.rs"]
mod serial;
#[cfg(test)]
#[path = "r1_manual_capture/tests.rs"]
mod tests;

const FIRMWARE: &str = "version: UART_REAL, circle6:1.9.6.f8ef651@master engine:circle6.1004; build date: Sep  3 2026 22:40:02";
const USAGE: &str = "Usage: r1_manual_capture --help
       r1_manual_capture run <uart-by-id> <alsa-hw> <beam:0..5> <silence-seconds:30..3600> <new-output-dir> <cold-boot-note> [--save-pcm]

Linux only. Wait for the main task to release the device before running.
Checks the pinned 1.9.6 firmware, observes 5s baseline, sends manual_wakeup ONCE,
then prompts for 45s speech / long silence / 45s speech. No automatic retry.
No models, playback, power control, firmware changes or service management.
JSONL statistics are saved; --save-pcm also saves private 16kHz mono S16_LE audio.
Cold boot is operator-declared; ACK/energy alone never proves intelligibility.";

#[derive(Debug, serde::Serialize)]
struct Options {
    uart: PathBuf,
    pcm: String,
    beam: u8,
    silence_seconds: u64,
    output: PathBuf,
    cold_boot_note: String,
    save_pcm: bool,
}

fn parse(args: &[String]) -> Result<Option<Options>> {
    if args.is_empty() || matches!(args, [help] if help == "--help" || help == "-h") {
        return Ok(None);
    }
    ensure!(args.len() == 7 || args.len() == 8, "{USAGE}");
    ensure!(args[0] == "run", "{USAGE}");
    ensure!(args.len() == 7 || args[7] == "--save-pcm", "{USAGE}");
    let uart = PathBuf::from(&args[1]);
    ensure!(
        uart.parent() == Some(Path::new("/dev/serial/by-id"))
            && uart.file_name().is_some()
            && !args[1].contains("/../"),
        "use an explicit /dev/serial/by-id/... control UART"
    );
    let pcm = &args[2];
    let card = pcm
        .strip_prefix("hw:CARD=")
        .and_then(|s| s.strip_suffix(",DEV=0"));
    ensure!(
        card.is_some_and(|s| !s.is_empty()
            && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            && !s.bytes().all(|c| c.is_ascii_digit())),
        "use an explicit hw:CARD=<name>,DEV=0 PCM, without plugins or card ordinals"
    );
    let beam: u8 = args[3].parse()?;
    ensure!(beam <= 5, "beam must be 0..5 (0 is directional)");
    let silence_seconds: u64 = args[4].parse()?;
    ensure!(
        (30..=3600).contains(&silence_seconds),
        "silence must be 30..3600 seconds"
    );
    ensure!(!args[5].trim().is_empty(), "output directory is required");
    ensure!(
        !args[6].trim().is_empty() && args[6].len() <= 2048,
        "record the cold boot time, device identity and speaker direction (max 2048 bytes)"
    );
    Ok(Some(Options {
        uart,
        pcm: pcm.clone(),
        beam,
        silence_seconds,
        output: PathBuf::from(&args[5]),
        cold_boot_note: args[6].clone(),
        save_pcm: args.len() == 8,
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    let Some(options) = parse(&std::env::args().skip(1).collect::<Vec<_>>())? else {
        println!("{USAGE}\n\nPinned firmware: {FIRMWARE}");
        return Ok(());
    };
    #[cfg(target_os = "linux")]
    {
        use tokio::signal::unix::{SignalKind, signal};
        use tokio_util::sync::CancellationToken;
        let cancel = CancellationToken::new();
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        let signal_cancel = cancel.clone();
        let signals = tokio::spawn(async move {
            tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
            signal_cancel.cancel();
        });
        let result = tokio::task::spawn_blocking(move || linux::run(options, cancel)).await;
        signals.abort();
        result??;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = options;
        anyhow::bail!("hardware mode requires Linux; no devices or output files were opened")
    }
}
