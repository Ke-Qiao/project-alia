use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use alia_core::audio_clock::AudioDirection;
use alia_embedded::config::{AudioBackend, AudioConfig};
use alia_embedded::module::context::AudioBus;
use alia_embedded::module::cpal_audio::CpalAudioCapture;
use alia_embedded::module::traits::AudioCaptureBackend;
use alia_embedded::module::volume::{AudioVolumeConfig, AudioVolumeState};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::TryRecvError;
use tokio_util::sync::CancellationToken;

use super::probe::{Phase, Probe};
use super::{FIRMWARE, Options, protocol, serial};

struct Capture(CpalAudioCapture);
impl Drop for Capture {
    fn drop(&mut self) {
        if let Err(error) = self.0.stop_capture() {
            eprintln!("capture cleanup failed: {error:#}");
        }
    }
}

fn new_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {} without overwriting", path.display()))
}

struct Journal {
    events: File,
    pcm: Option<BufWriter<File>>,
    window_started: Duration,
    partial_discards: u64,
}

impl Journal {
    fn event(&mut self, now: Duration, event: Value) -> Result<()> {
        serde_json::to_writer(
            &mut self.events,
            &json!({"host_elapsed_ms":now.as_millis(),"event":event}),
        )?;
        self.events.write_all(b"\n")?;
        Ok(())
    }

    fn window(&mut self, probe: &mut Probe, phase: Phase, now: Duration) -> Result<()> {
        self.event(
            now,
            json!({"type":"audio_window", "phase":phase,
            "start_ms":self.window_started.as_millis(), "levels":probe.window.report(),
            "total_samples":probe.total_samples, "last_timing":probe.last_timing}),
        )?;
        probe.window = Default::default();
        self.window_started = now;
        Ok(())
    }

    fn transition(&mut self, probe: &mut Probe, previous: Phase, now: Duration) -> Result<()> {
        if probe.phase != previous {
            self.window(probe, previous, now)?;
            self.event(now, json!({"type":"phase", "phase":probe.phase,"instruction":probe.phase.instruction()}))?;
            eprintln!("[{:.1}s] {}", now.as_secs_f64(), probe.phase.instruction());
        }
        Ok(())
    }

    fn send(
        &mut self,
        serial: &mut serial::Serial,
        bytes: &[u8],
        now: Duration,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let frame = protocol::Decoder::default()
            .push(bytes)
            .pop()
            .context("invalid outgoing frame")?;
        // Log intent before I/O: a failed/partial write must never be retried implicitly.
        self.event(
            now,
            json!({"type":"tx_attempt", "kind":frame.kind,"id":frame.id,
            "payload":String::from_utf8_lossy(&frame.payload)}),
        )?;
        serial.write(bytes, cancel)?;
        self.event(
            now,
            json!({"type":"tx_complete","kind":frame.kind,"id":frame.id}),
        )
    }
}

pub(super) fn run(options: Options, cancel: CancellationToken) -> Result<()> {
    ensure!(
        !cancel.is_cancelled(),
        "cancelled before opening output or devices"
    );
    DirBuilder::new()
        .mode(0o700)
        .create(&options.output)
        .context("output must be a new directory with an existing parent")?;
    let mut journal = Journal {
        events: new_file(&options.output.join("events.jsonl"))?,
        pcm: if options.save_pcm {
            Some(BufWriter::new(new_file(
                &options.output.join("capture.s16le"),
            )?))
        } else {
            None
        },
        window_started: Duration::ZERO,
        partial_discards: 0,
    };
    let id = (uuid::Uuid::new_v4().as_u128() & 0xffff) as u16;
    let mut probe = Probe::new(
        options.beam,
        Duration::from_secs(options.silence_seconds),
        id,
    );
    journal.event(
        Duration::ZERO,
        json!({"type":"run", "schema_version":1,
        "source":"live_uart_and_alsa", "started_at":chrono::Utc::now(), "options":options,
        "expected_firmware":FIRMWARE, "version_id":id, "manual_id":probe.manual_id,
        "timing_basis":"host observation; phase boundaries are not acoustic sample timestamps"}),
    )?;
    let config = AudioConfig {
        capture_backend: AudioBackend::Alsa,
        capture_device: options.pcm.clone(),
        capture_sample_rate: 16000,
        capture_channels: 1,
        capture_volume: 1.0,
        ..AudioConfig::default()
    };
    let volume = AudioVolumeState::new(AudioVolumeConfig::from_audio_config(&config));
    let capture = Capture(CpalAudioCapture::new(config, volume));
    let bus = AudioBus::new(128);
    let start = Instant::now();
    let mut result = collect(
        &options,
        &cancel,
        &capture.0,
        &bus,
        &mut probe,
        &mut journal,
        start,
    );
    let cleanup = capture.0.stop_capture();
    if let Err(error) = &cleanup {
        result = Err(anyhow::anyhow!(
            "capture cleanup: {error:#}; observation: {result:?}"
        ));
    }
    if let Some(pcm) = &mut journal.pcm
        && let Err(error) = pcm.flush().and_then(|()| pcm.get_ref().sync_all())
    {
        result = Err(anyhow::anyhow!(
            "saving PCM failed: {error}; observation: {result:?}"
        ));
    }
    let now = start.elapsed();
    let phase = probe.phase;
    journal.window(&mut probe, phase, now)?;
    let snapshot = bus.reference().snapshot(AudioDirection::Capture);
    if let Some(snapshot) = &snapshot
        && let Some(event) = probe.reference_loss(snapshot.lost_blocks, snapshot.lost_frames)
    {
        journal.event(now, event)?;
    }
    if snapshot
        .as_ref()
        .is_some_and(|s| !s.closed || s.last_fault.is_some())
        && result.is_ok()
    {
        result = Err(anyhow::anyhow!(
            "capture closed with a fault or incomplete cleanup"
        ));
    }
    let report = json!({"schema_version":1, "source":"live_uart_and_alsa",
        "options":options, "observation_completed":result.is_ok() && probe.phase == Phase::Done,
        "error":result.as_ref().err().map(|e| format!("{e:#}")), "probe":probe.report(),
        "elapsed_ms":now.as_millis(), "uart_partial_discards":journal.partial_discards,
        "capture_after_cleanup":snapshot, "cleanup_error":cleanup.err().map(|e| format!("{e:#}"))});
    journal.event(now, json!({"type":"end","report":report}))?;
    journal.events.sync_all()?;
    let mut report_file = new_file(&options.output.join("report.json"))?;
    serde_json::to_writer_pretty(&mut report_file, &report)?;
    report_file.write_all(b"\n")?;
    report_file.sync_all()?;
    eprintln!("Evidence: {}", options.output.display());
    result
}

fn collect(
    options: &Options,
    cancel: &CancellationToken,
    capture: &CpalAudioCapture,
    bus: &AudioBus,
    probe: &mut Probe,
    journal: &mut Journal,
    start: Instant,
) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "cancelled before opening devices");
    let mut uart = serial::Serial::open(&options.uart)?;
    let mut audio = bus.subscribe();
    let mut native = bus
        .reference()
        .try_claim_reader()
        .context("capture evidence reader already owned")?;
    capture.start_capture(bus)?;
    ensure!(!cancel.is_cancelled(), "cancelled during capture startup");
    probe.phase_started = start.elapsed();
    probe.last_audio = start.elapsed();
    journal.send(
        &mut uart,
        &protocol::version_query(probe.version_id),
        start.elapsed(),
        cancel,
    )?;
    eprintln!("{}", probe.phase.instruction());
    let mut decoder = protocol::Decoder::default();
    let mut partial_since = None;
    let mut sample_format = None;
    let mut bytes = [0u8; 1024];
    while probe.phase != Phase::Done {
        ensure!(!cancel.is_cancelled(), "cancelled; observation incomplete");
        ensure!(capture.is_available(), "capture owner stopped");
        while let Some(block) = native.try_read() {
            if block.sample_format.is_some() && block.sample_format != sample_format {
                sample_format = block.sample_format;
                journal.event(
                    start.elapsed(),
                    json!({"type":"native_format",
                    "format":block.timing.format,"sample_format":sample_format}),
                )?;
            }
            if let Some(reason) = block.discontinuity {
                journal.event(
                    start.elapsed(),
                    json!({"type":"native_marker","reason":reason,"timing":block.timing}),
                )?;
            }
        }
        if let Some(snapshot) = native.snapshot(AudioDirection::Capture) {
            if let Some(event) = probe.reference_loss(snapshot.lost_blocks, snapshot.lost_frames) {
                journal.event(start.elapsed(), event)?;
            }
            ensure!(
                snapshot.running && snapshot.last_fault.is_none(),
                "native capture failed: {snapshot:?}"
            );
        }
        // A bounded drain prevents a faulty producer from hiding cancellation or UART deadlines.
        for _ in 0..128 {
            match audio.try_recv() {
                Ok(frame) => {
                    probe.audio(&frame, start.elapsed())?;
                    if let Some(pcm) = &mut journal.pcm {
                        for sample in frame.pcm {
                            pcm.write_all(&sample.to_le_bytes())?;
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(error) => bail!("capture receiver lost/closed PCM: {error}"),
            }
        }
        let count = uart.read(&mut bytes)?;
        let now = start.elapsed();
        if partial_since
            .is_some_and(|since| now.saturating_sub(since) >= Duration::from_millis(250))
        {
            decoder.clear();
            partial_since = None;
            journal.partial_discards += 1;
            journal.event(now, json!({"type":"uart_incomplete_frame_discarded"}))?;
        }
        for frame in decoder.push(&bytes[..count]) {
            journal.event(
                now,
                json!({"type":"rx","kind":frame.kind,"id":frame.id,
                "payload":String::from_utf8_lossy(&frame.payload)}),
            )?;
            let previous = probe.phase;
            if let Some(reply) = probe.receive(&frame, now)? {
                journal.send(&mut uart, &reply, now, cancel)?;
            }
            journal.transition(probe, previous, now)?;
        }
        if decoder.is_empty() {
            partial_since = None;
        } else if partial_since.is_none() {
            partial_since = Some(now);
        }
        probe.check_audio_progress(now)?;
        let previous = probe.phase;
        if let Some(command) = probe.tick(now)? {
            journal.send(&mut uart, &command, now, cancel)?;
        }
        journal.transition(probe, previous, now)?;
        if now.saturating_sub(journal.window_started) >= Duration::from_secs(1) {
            journal.window(probe, probe.phase, now)?;
        }
    }
    Ok(())
}
