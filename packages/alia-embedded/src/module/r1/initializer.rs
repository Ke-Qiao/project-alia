use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::R1Config;

/// A command acknowledgement, not proof of live or intelligible PCM.
#[derive(Debug, Serialize)]
pub struct InitializationReport {
    pub cold_boot_id: Uuid,
    pub serial_path: PathBuf,
    pub guard_path: PathBuf,
    /// Expected value in a claim; an accepted report confirms it was verified.
    pub firmware: String,
    pub beam: u8,
    pub command_id: u16,
    pub command_accepted: bool,
    pub acoustic_capture_verified: bool,
}

#[cfg(target_os = "linux")]
pub(crate) use linux::PreparedInitialization;

#[cfg(not(target_os = "linux"))]
pub(crate) struct PreparedInitialization;

#[cfg(not(target_os = "linux"))]
impl PreparedInitialization {
    pub(crate) fn prepare(
        config: &R1Config,
        state_path: &Path,
        cold_boot_id: Uuid,
        cancel: CancellationToken,
    ) -> Result<Self> {
        let _ = (config, state_path, cold_boot_id, cancel);
        anyhow::bail!("R1 initialization requires Linux; no state or devices were opened")
    }

    pub(crate) fn run(self, cancel: CancellationToken) -> Result<InitializationReport> {
        let _ = cancel;
        anyhow::bail!("R1 initialization requires Linux; no state or devices were opened")
    }
}

#[cfg(any(target_os = "linux", test))]
mod transaction {
    use std::time::Duration;

    use anyhow::{Result, ensure};

    use super::super::protocol;

    const DEADLINE: Duration = Duration::from_secs(2);

    #[derive(Debug, PartialEq)]
    pub(super) enum Phase {
        Version,
        Ready,
        Ack,
        Accepted,
    }

    pub(super) struct Transaction {
        pub phase: Phase,
        pub version_id: u16,
        pub command_id: u16,
        started: Duration,
        requery_pending: bool,
        requery_attempted: bool,
    }

    impl Transaction {
        pub fn new(version_id: u16) -> Self {
            Self {
                phase: Phase::Version,
                version_id,
                command_id: version_id.wrapping_add(1),
                started: Duration::ZERO,
                requery_pending: false,
                requery_attempted: false,
            }
        }

        pub fn check_deadline(&self, now: Duration) -> Result<()> {
            if matches!(self.phase, Phase::Version | Phase::Ack) {
                ensure!(
                    now.saturating_sub(self.started) < DEADLINE,
                    if self.phase == Phase::Version {
                        "R1 version response timed out; manual_wakeup was not sent"
                    } else {
                        "R1 manual_wakeup ACK timed out; outcome unknown, do not retry"
                    }
                );
            }
            Ok(())
        }

        pub fn receive(
            &mut self,
            frame: &protocol::Frame,
            now: Duration,
        ) -> Result<Option<Vec<u8>>> {
            self.check_deadline(now)?;
            ensure!(
                !matches!(self.phase, Phase::Version | Phase::Ready) || frame.kind != 0x04,
                "R1 board event before manual_wakeup; operator-declared cold boot may already be active"
            );
            if let Some(ack) = frame.handshake_ack() {
                ensure!(
                    self.phase == Phase::Version,
                    "R1 handshake after version verification; board may have restarted, do not retry"
                );
                self.requery_pending = !self.requery_attempted;
                return Ok(Some(ack));
            }
            let id = match self.phase {
                Phase::Version => self.version_id,
                Phase::Ack => self.command_id,
                _ => return Ok(None),
            };
            let Some(accepted) = frame.command_ack(id) else {
                return Ok(None);
            };
            ensure!(
                accepted,
                "R1 command ACK rejected, malformed or missing numeric code:0"
            );
            if self.phase == Phase::Version {
                ensure!(
                    frame.version_response(id).as_deref()
                        == Some(protocol::MANUAL_CAPTURE_FIRMWARE),
                    "R1 firmware differs from the validated manual capture firmware; manual_wakeup was not sent"
                );
                self.phase = Phase::Ready;
            } else {
                self.phase = Phase::Accepted;
            }
            Ok(None)
        }

        pub fn requery(&mut self, now: Duration) -> Result<Option<Vec<u8>>> {
            self.check_deadline(now)?;
            if self.phase == Phase::Version && self.requery_pending {
                self.requery_pending = false;
                self.requery_attempted = true;
                return Ok(Some(protocol::version_query(self.version_id)));
            }
            Ok(None)
        }

        pub fn manual(&mut self, now: Duration) -> Result<Vec<u8>> {
            ensure!(
                self.phase == Phase::Ready,
                "R1 manual_wakeup is not ready or was already attempted"
            );
            self.phase = Phase::Ack;
            self.started = now;
            Ok(protocol::manual_wakeup_beam_zero(self.command_id))
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::{DirBuilder, File, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::time::{Duration, Instant};

    use anyhow::{Context, ensure};

    use super::super::{protocol, serial};
    use super::transaction::{Phase, Transaction};
    use super::*;

    fn claim(
        config: &R1Config,
        state_path: &Path,
        cold_boot_id: Uuid,
        command_id: u16,
    ) -> Result<(File, InitializationReport)> {
        ensure!(
            !cold_boot_id.is_nil(),
            "cold_boot_id must identify an operator-confirmed cold boot"
        );
        let state_path = std::path::absolute(state_path)?;
        let mut directory_name = state_path
            .file_name()
            .context("state_path requires a filename")?
            .to_os_string();
        directory_name.push(".r1-initialization");
        let directory = state_path
            .parent()
            .context("state_path requires a parent")?
            .join(directory_name);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)?;
        let metadata = std::fs::symlink_metadata(&directory)?;
        ensure!(
            metadata.is_dir()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.permissions().mode() & 0o022 == 0,
            "R1 claim directory must be an owned directory without group/world writes or symlink redirection"
        );
        let guard_path = directory.join(format!("{cold_boot_id}.jsonl"));
        let mut guard = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&guard_path)
            .with_context(|| {
                format!(
                    "claiming R1 cold boot {}: an existing claim must never be retried",
                    guard_path.display()
                )
            })?;
        let report = InitializationReport {
            cold_boot_id,
            serial_path: config.serial_path.clone(),
            guard_path,
            firmware: protocol::MANUAL_CAPTURE_FIRMWARE.into(),
            beam: 0,
            command_id,
            command_accepted: false,
            acoustic_capture_verified: false,
        };
        serde_json::to_writer(
            &mut guard,
            &serde_json::json!({
                "status":"claimed_outcome_unknown", "operator_declared_cold_boot":true,
                "report":report
            }),
        )?;
        guard.write_all(b"\n")?;
        guard.sync_all()?;
        // Persist newly created parents as well as the one-shot claim before any UART write.
        for ancestor in directory.ancestors() {
            File::open(ancestor)?.sync_all()?;
        }
        Ok((guard, report))
    }

    /// Own the durable claim and UART before capture starts. No command is sent here.
    /// Dropping this owner releases the UART but never removes its one-shot claim.
    pub(crate) struct PreparedInitialization {
        serial: serial::Serial,
        transaction: Transaction,
        guard: File,
        report: InitializationReport,
    }

    impl PreparedInitialization {
        /// Explicit operator-declared cold boot only, never reconnect or capture reset.
        /// Keep state on durable, owner-controlled storage; removing claims loses replay protection.
        pub(crate) fn prepare(
            config: &R1Config,
            state_path: &Path,
            cold_boot_id: Uuid,
            cancel: CancellationToken,
        ) -> Result<Self> {
            config.validate()?;
            ensure!(
                !cancel.is_cancelled(),
                "R1 initialization cancelled before claiming cold boot"
            );
            let transaction = Transaction::new(rand::random());
            let (guard, report) = claim(config, state_path, cold_boot_id, transaction.command_id)?;
            ensure!(
                !cancel.is_cancelled(),
                "R1 initialization cancelled after claim; do not retry this cold boot ID"
            );
            let serial = serial::Serial::open(&config.serial_path)?;
            Ok(Self {
                serial,
                transaction,
                guard,
                report,
            })
        }

        /// Consume the same UART after validating actual native PCM; do not reopen or
        /// clear input here, because capture startup may have queued a board handshake.
        /// The caller keeps capture open through completion and joins on cancellation.
        pub(crate) fn run(self, cancel: CancellationToken) -> Result<InitializationReport> {
            run(
                self.serial,
                self.transaction,
                self.guard,
                self.report,
                &cancel,
            )
        }
    }

    #[derive(Default)]
    struct IoDiagnostics {
        rx_bytes: u64,
        decoded_frames: u64,
        handshake_ack_attempted: u64,
        handshake_ack_completed: u64,
        version_requery_attempted: u64,
        version_requery_completed: u64,
        partial_clears: u64,
        manual_write_attempted: bool,
        manual_write_completed: bool,
    }

    fn run(
        mut serial: serial::Serial,
        mut transaction: Transaction,
        mut guard: File,
        mut report: InitializationReport,
        cancel: &CancellationToken,
    ) -> Result<InitializationReport> {
        let start = Instant::now();
        let mut io = IoDiagnostics::default();
        let result = (|| {
            serial.write(&protocol::version_query(transaction.version_id), cancel)?;
            let mut decoder = protocol::Decoder::default();
            let mut partial_since = None;
            let mut bytes = [0u8; 1024];
            loop {
                ensure!(
                    !cancel.is_cancelled(),
                    "R1 initialization cancelled; outcome may be unknown, do not retry"
                );
                transaction.check_deadline(start.elapsed())?;
                let count = serial.read(&mut bytes)?;
                io.rx_bytes = io.rx_bytes.saturating_add(count as u64);
                let now = start.elapsed();
                ensure!(
                    !cancel.is_cancelled(),
                    "R1 initialization cancelled; outcome may be unknown, do not retry"
                );
                if partial_since
                    .is_some_and(|since| now.saturating_sub(since) >= Duration::from_millis(250))
                {
                    decoder.clear();
                    io.partial_clears = io.partial_clears.saturating_add(1);
                    partial_since = None;
                }
                let frames = decoder.push(&bytes[..count]);
                io.decoded_frames = io.decoded_frames.saturating_add(frames.len() as u64);
                if decoder.is_empty() {
                    partial_since = None;
                } else if partial_since.is_none() || !frames.is_empty() {
                    partial_since = Some(now);
                }
                for frame in frames {
                    if let Some(ack) = transaction.receive(&frame, start.elapsed())? {
                        io.handshake_ack_attempted = io.handshake_ack_attempted.saturating_add(1);
                        serial.write(&ack, cancel)?;
                        io.handshake_ack_completed = io.handshake_ack_completed.saturating_add(1);
                    }
                }
                if let Some(query) = transaction.requery(start.elapsed())? {
                    io.version_requery_attempted = io.version_requery_attempted.saturating_add(1);
                    serial.write(&query, cancel)?;
                    io.version_requery_completed = io.version_requery_completed.saturating_add(1);
                }
                if transaction.phase == Phase::Ready {
                    // Transition before I/O: partial writes and uncertain ACKs consume this attempt.
                    let command = transaction.manual(start.elapsed())?;
                    io.manual_write_attempted = true;
                    serial.write(&command, cancel)?;
                    io.manual_write_completed = true;
                }
                if transaction.phase == Phase::Accepted {
                    report.command_accepted = true;
                    serde_json::to_writer(
                        &mut guard,
                        &serde_json::json!({"status":"command_accepted", "report":report}),
                    )?;
                    guard.write_all(b"\n")?;
                    guard.sync_all()?;
                    return Ok(report);
                }
            }
        })();
        result.map_err(|error: anyhow::Error| {
            // Keep the original error visible to to_string callers and retain its
            // source chain. Only scalar I/O evidence is added, never frame payloads.
            let message = format!(
                "{error}; R1 initialization diagnostics phase={:?} version_id={} command_id={} rx_bytes={} decoded_frames={} handshake_ack_attempted={} handshake_ack_completed={} version_requery_attempted={} version_requery_completed={} partial_clears={} manual_write_attempted={} manual_write_completed={} elapsed_ms={}",
                transaction.phase,
                transaction.version_id,
                transaction.command_id,
                io.rx_bytes,
                io.decoded_frames,
                io.handshake_ack_attempted,
                io.handshake_ack_completed,
                io.version_requery_attempted,
                io.version_requery_completed,
                io.partial_clears,
                io.manual_write_attempted,
                io.manual_write_completed,
                start.elapsed().as_millis(),
            );
            error.context(message)
        })
    }

    #[cfg(test)]
    mod tests {
        use std::io::{ErrorKind, Read};
        use std::thread;

        use super::*;

        fn config() -> R1Config {
            R1Config {
                serial_path: "/missing-test-r1".into(),
            }
        }

        #[test]
        fn durable_claim_survives_reopen_and_rejects_duplicate_or_corrupt_records() {
            let directory = tempfile::tempdir().unwrap();
            let state = directory.path().join("state.sqlite3");
            let id = Uuid::new_v4();
            let (guard, report) = claim(&config(), &state, id, 42).unwrap();
            drop(guard);
            let original = std::fs::read(&report.guard_path).unwrap();
            assert!(String::from_utf8_lossy(&original).contains("claimed_outcome_unknown"));
            assert!(claim(&config(), &state, id, 43).is_err());
            assert_eq!(std::fs::read(&report.guard_path).unwrap(), original);
            std::fs::write(&report.guard_path, []).unwrap();
            assert!(claim(&config(), &state, id, 44).is_err());
        }

        #[test]
        fn claim_rejects_redirected_or_shared_writable_guard_directory() {
            let directory = tempfile::tempdir().unwrap();
            let state = directory.path().join("state.sqlite3");
            let guard_directory = directory.path().join("state.sqlite3.r1-initialization");
            let redirected = directory.path().join("redirected");
            std::fs::create_dir(&redirected).unwrap();
            std::os::unix::fs::symlink(&redirected, &guard_directory).unwrap();
            assert!(claim(&config(), &state, Uuid::new_v4(), 42).is_err());
            assert_eq!(std::fs::read_dir(&redirected).unwrap().count(), 0);
            std::fs::remove_file(&guard_directory).unwrap();
            std::fs::create_dir(&guard_directory).unwrap();
            std::fs::set_permissions(&guard_directory, std::fs::Permissions::from_mode(0o777))
                .unwrap();
            assert!(claim(&config(), &state, Uuid::new_v4(), 42).is_err());
            assert_eq!(std::fs::read_dir(&guard_directory).unwrap().count(), 0);
        }

        #[test]
        fn cancellation_and_invalid_cold_id_do_not_claim_or_touch_hardware() {
            let directory = tempfile::tempdir().unwrap();
            let state = directory.path().join("state.sqlite3");
            let cancel = CancellationToken::new();
            cancel.cancel();
            assert!(
                PreparedInitialization::prepare(&config(), &state, Uuid::new_v4(), cancel).is_err()
            );
            assert!(
                PreparedInitialization::prepare(
                    &config(),
                    &state,
                    Uuid::nil(),
                    CancellationToken::new()
                )
                .is_err()
            );
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        }

        #[test]
        fn failed_open_keeps_claim_and_same_id_never_retries() {
            let directory = tempfile::tempdir().unwrap();
            let state = directory.path().join("state.sqlite3");
            let id = Uuid::new_v4();
            let first =
                PreparedInitialization::prepare(&config(), &state, id, CancellationToken::new())
                    .err()
                    .unwrap();
            assert!(format!("{first:#}").contains("opening R1 UART"));
            let second =
                PreparedInitialization::prepare(&config(), &state, id, CancellationToken::new())
                    .err()
                    .unwrap();
            assert!(format!("{second:#}").contains("existing claim must never be retried"));
        }

        fn read_frame(master: &mut File) -> protocol::Frame {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut decoder = protocol::Decoder::default();
            let mut byte = [0u8; 1];
            loop {
                assert!(Instant::now() < deadline, "missing host UART frame");
                match master.read(&mut byte) {
                    Ok(1) => {
                        if let Some(frame) = decoder.push(&byte).pop() {
                            return frame;
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    result => panic!("unexpected PTY read: {result:?}"),
                }
            }
        }

        fn pty_cold_handshake(queued_before_run: bool) {
            let directory = tempfile::tempdir().unwrap();
            let (guard, report) = claim(
                &config(),
                &directory.path().join("state.sqlite3"),
                Uuid::new_v4(),
                101,
            )
            .unwrap();
            let guard_path = report.guard_path.clone();
            let (mut board, serial, path) = serial::pty();
            // PTYs do not implement modem line ioctls. Use the existing UART fixture,
            // including the input flush that production Serial::open performs.
            serial.clear_input().unwrap();
            let prepared = PreparedInitialization {
                serial,
                transaction: Transaction::new(100),
                guard,
                report,
            };
            let handshake = protocol::encode(1, 9, &[0xa5, 0, 0, 0]);
            if queued_before_run {
                // Model a handshake arriving while native capture is starting, before
                // validated PCM allows the initialization transaction to run.
                board.write_all(&handshake).unwrap();
                // Observe readable queued input without consuming it, so a misplaced
                // late TCIFLUSH deterministically discards this already-arrived frame.
                use std::os::fd::AsRawFd;
                let observer = OpenOptions::new().read(true).open(path).unwrap();
                let mut queued = libc::pollfd {
                    fd: observer.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                assert_eq!(unsafe { libc::poll(&mut queued, 1, 3000) }, 1);
                assert_ne!(queued.revents & libc::POLLIN, 0);
                let mut bytes = [0u8; 1];
                assert_eq!(
                    board.read(&mut bytes).unwrap_err().kind(),
                    ErrorKind::WouldBlock
                );
            }
            let worker = thread::spawn(move || prepared.run(CancellationToken::new()));
            let query = read_frame(&mut board);
            assert_eq!(query.id, 100);
            if !queued_before_run {
                board.write_all(&handshake).unwrap();
            }
            let ack = read_frame(&mut board);
            assert_eq!((ack.kind, ack.id), (255, 9));
            assert_eq!(read_frame(&mut board), query);
            board
                .write_all(&protocol::encode(
                    5,
                    100,
                    &serde_json::to_vec(
                        &serde_json::json!({"code":0,"content":protocol::MANUAL_CAPTURE_FIRMWARE}),
                    )
                    .unwrap(),
                ))
                .unwrap();
            let manual = read_frame(&mut board);
            assert_eq!((manual.kind, manual.id), (5, 101));
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&manual.payload).unwrap(),
                serde_json::json!({"type":"manual_wakeup","content":{"beam":0}})
            );
            board
                .write_all(&protocol::encode(5, 101, br#"{"code":0}"#))
                .unwrap();
            let report = worker.join().unwrap().unwrap();
            assert!(report.command_accepted);
            assert!(!report.acoustic_capture_verified);
            let contents = std::fs::read_to_string(guard_path).unwrap();
            assert_eq!(contents.lines().count(), 2);
            assert!(
                contents
                    .lines()
                    .nth(1)
                    .unwrap()
                    .contains("command_accepted")
            );
        }

        #[test]
        fn pty_cold_handshake_then_single_manual_command_and_durable_ack() {
            pty_cold_handshake(false);
        }

        #[test]
        fn pty_prepared_uart_preserves_handshake_queued_before_pcm_validation() {
            pty_cold_handshake(true);
        }

        #[test]
        fn pty_prepared_drop_or_cancellation_sends_nothing_and_keeps_claim() {
            for cancelled in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let state = directory.path().join("state.sqlite3");
                let id = Uuid::new_v4();
                let (guard, report) = claim(&config(), &state, id, 101).unwrap();
                let guard_path = report.guard_path.clone();
                let (mut board, serial, path) = serial::pty();
                // Keep a test slave descriptor open so no-data remains WouldBlock,
                // rather than the PTY's EIO after the prepared UART owner is dropped.
                let _slave = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .unwrap();
                let prepared = PreparedInitialization {
                    serial,
                    transaction: Transaction::new(100),
                    guard,
                    report,
                };
                if cancelled {
                    let cancel = CancellationToken::new();
                    cancel.cancel();
                    let error = prepared.run(cancel).unwrap_err();
                    assert_eq!(error.root_cause().to_string(), "R1 UART write cancelled");
                    assert!(error.to_string().contains("manual_write_attempted=false"));
                } else {
                    // Covers invalid/missing PCM or capture-start failure before run.
                    drop(prepared);
                }
                let mut bytes = [0u8; 1];
                assert_eq!(
                    board.read(&mut bytes).unwrap_err().kind(),
                    ErrorKind::WouldBlock
                );
                let contents = std::fs::read_to_string(&guard_path).unwrap();
                assert_eq!(contents.lines().count(), 1);
                assert!(contents.contains("claimed_outcome_unknown"));
                assert!(claim(&config(), &state, id, 102).is_err());
                assert_eq!(std::fs::read_to_string(guard_path).unwrap(), contents);
            }
        }

        fn pty_timeout_diagnostics(after_manual: bool) {
            let directory = tempfile::tempdir().unwrap();
            let (guard, report) = claim(
                &config(),
                &directory.path().join("state.sqlite3"),
                Uuid::new_v4(),
                101,
            )
            .unwrap();
            let guard_path = report.guard_path.clone();
            let (mut board, serial, _) = serial::pty();
            let (send, receive) = std::sync::mpsc::sync_channel(1);
            let worker = thread::spawn(move || {
                send.send(run(
                    serial,
                    Transaction::new(100),
                    guard,
                    report,
                    &CancellationToken::new(),
                ))
                .unwrap();
            });
            assert_eq!(read_frame(&mut board).id, 100);
            let handshake = protocol::encode(1, 9, &[0xa5, 0, 0, 0]);
            board.write_all(&handshake).unwrap();
            assert_eq!(read_frame(&mut board).kind, 255);
            assert_eq!(read_frame(&mut board).id, 100);
            let mut rx_bytes = handshake.len();
            if after_manual {
                let version = protocol::encode(
                    5,
                    100,
                    &serde_json::to_vec(
                        &serde_json::json!({"code":0,"content":protocol::MANUAL_CAPTURE_FIRMWARE}),
                    )
                    .unwrap(),
                );
                board.write_all(&version).unwrap();
                rx_bytes += version.len();
                let manual = read_frame(&mut board);
                assert_eq!((manual.kind, manual.id), (5, 101));
            } else {
                let partial = protocol::encode(5, 100, b"private-partial-payload");
                board.write_all(&partial[..partial.len() - 1]).unwrap();
                rx_bytes += partial.len() - 1;
            }
            let error = receive
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap_err();
            worker.join().unwrap();
            let message = error.to_string();
            let expected = if after_manual {
                "R1 manual_wakeup ACK timed out; outcome unknown, do not retry"
            } else {
                "R1 version response timed out; manual_wakeup was not sent"
            };
            assert!(message.starts_with(expected));
            assert_eq!(error.root_cause().to_string(), expected);
            for field in [
                "version_id=100",
                "command_id=101",
                "handshake_ack_attempted=1",
                "handshake_ack_completed=1",
                "version_requery_attempted=1",
                "version_requery_completed=1",
                "elapsed_ms=",
            ] {
                assert!(message.contains(field), "missing {field}: {message}");
            }
            assert!(message.contains(&format!("rx_bytes={rx_bytes} ")));
            assert!(!format!("{error:#}").contains("private-partial-payload"));
            assert!(!format!("{error:#}").contains(protocol::MANUAL_CAPTURE_FIRMWARE));
            if after_manual {
                assert!(message.contains("phase=Ack "));
                assert!(message.contains("decoded_frames=2 "));
                assert!(
                    message.contains("manual_write_attempted=true manual_write_completed=true")
                );
                assert!(!message.contains("manual_wakeup was not sent"));
            } else {
                assert!(message.contains("phase=Version "));
                assert!(message.contains("decoded_frames=1 "));
                assert!(
                    message.contains("manual_write_attempted=false manual_write_completed=false")
                );
                let clears: u64 = message
                    .split("partial_clears=")
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap();
                assert!(clears >= 1);
            }
            assert_eq!(
                std::fs::read_to_string(guard_path).unwrap().lines().count(),
                1
            );
        }

        #[test]
        fn pty_version_timeout_reports_handshake_and_discarded_partial_without_manual() {
            pty_timeout_diagnostics(false);
        }

        #[test]
        fn pty_ack_timeout_retains_cause_and_reports_manual_write_without_acceptance() {
            pty_timeout_diagnostics(true);
        }

        #[test]
        fn pty_cancel_or_disconnect_after_manual_keeps_unknown_claim_without_retry() {
            for disconnect in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let state = directory.path().join("state.sqlite3");
                let id = Uuid::new_v4();
                let (guard, report) = claim(&config(), &state, id, 101).unwrap();
                let guard_path = report.guard_path.clone();
                let (mut board, serial, _) = serial::pty();
                let cancel = CancellationToken::new();
                let worker_cancel = cancel.clone();
                let (send, receive) = std::sync::mpsc::sync_channel(1);
                let worker = thread::spawn(move || {
                    send.send(run(
                        serial,
                        Transaction::new(100),
                        guard,
                        report,
                        &worker_cancel,
                    ))
                    .unwrap();
                });
                assert_eq!(read_frame(&mut board).id, 100);
                board.write_all(&protocol::encode(5, 100, &serde_json::to_vec(&serde_json::json!({"code":0,"content":protocol::MANUAL_CAPTURE_FIRMWARE})).unwrap())).unwrap();
                let manual = read_frame(&mut board);
                assert_eq!((manual.kind, manual.id), (5, 101));
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&manual.payload).unwrap()["type"],
                    "manual_wakeup"
                );
                if disconnect {
                    drop(board);
                } else {
                    let mut extra = [0u8; 1024];
                    assert_eq!(
                        board.read(&mut extra).unwrap_err().kind(),
                        ErrorKind::WouldBlock
                    );
                    cancel.cancel();
                }
                let error = receive
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .unwrap_err();
                let message = error.to_string();
                assert!(message.contains("phase=Ack "));
                assert!(
                    message.contains("manual_write_attempted=true manual_write_completed=true")
                );
                assert!(!message.contains("manual_wakeup was not sent"));
                assert!(error.chain().count() >= 2);
                assert!(!error.root_cause().to_string().contains("diagnostics"));
                worker.join().unwrap();
                let contents = std::fs::read_to_string(&guard_path).unwrap();
                assert_eq!(contents.lines().count(), 1);
                assert!(contents.contains("claimed_outcome_unknown"));
                assert!(claim(&config(), &state, id, 102).is_err());
                assert_eq!(std::fs::read_to_string(&guard_path).unwrap(), contents);
            }
        }
    }
}

#[cfg(test)]
#[path = "initializer_tests.rs"]
mod tests;
