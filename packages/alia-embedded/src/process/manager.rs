use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn, error};

use crate::config::ProcessConfig;

/// Maximum line length from child process stdout/stderr (1 MB).
const MAX_LINE_LENGTH: usize = 1_048_576;

/// Channel capacity for output broadcast.
const OUTPUT_CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone)]
pub struct OutputLine {
    pub name: String,
    pub channel: OutputChannel,
    pub line: String,
    pub timestamp_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputChannel {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

struct ManagedProcess {
    config: ProcessConfig,
    state: ProcessState,
    pid: Option<u32>,
    started_at: Option<Instant>,
    restart_count: u32,
    child_handle: Option<tokio::task::JoinHandle<()>>,
}

pub struct ProcessManager {
    processes: RwLock<HashMap<String, ManagedProcess>>,
    output_tx: broadcast::Sender<OutputLine>,
    run_dir: PathBuf,
}

impl ProcessManager {
    pub fn new(configs: Vec<ProcessConfig>, run_dir: PathBuf) -> Self {
        let (output_tx, _) = broadcast::channel(OUTPUT_CHANNEL_CAPACITY);

        let mut processes = HashMap::new();
        for cfg in configs {
            processes.insert(
                cfg.name.clone(),
                ManagedProcess {
                    config: cfg,
                    state: ProcessState::Stopped,
                    pid: None,
                    started_at: None,
                    restart_count: 0,
                    child_handle: None,
                },
            );
        }

        Self {
            processes: RwLock::new(processes),
            output_tx,
            run_dir,
        }
    }

    /// Clean up orphan processes from a previous run by scanning PID files.
    pub async fn cleanup_orphans(&self) -> Result<()> {
        let run_dir = &self.run_dir;
        if !run_dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(run_dir)
            .await
            .with_context(|| format!("reading run dir: {}", run_dir.display()))?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "pid") {
                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => {
                        if let Ok(pid) = content.trim().parse::<i32>() {
                            // Check if process is still alive and kill it
                            // SAFETY: sending signal 0 checks if process exists
                            let exists = unsafe { libc::kill(pid, 0) } == 0;
                            if exists {
                                warn!(pid, file = %path.display(), "killing orphan process");
                                unsafe { libc::kill(pid, libc::SIGTERM) };
                                // Give it a moment, then force kill
                                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                let still_exists = unsafe { libc::kill(pid, 0) } == 0;
                                if still_exists {
                                    unsafe { libc::kill(pid, libc::SIGKILL) };
                                }
                            }
                        }
                        let _ = tokio::fs::remove_file(&path).await;
                    }
                    Err(e) => {
                        warn!(error = %e, file = %path.display(), "failed to read pid file");
                        let _ = tokio::fs::remove_file(&path).await;
                    }
                }
            }
        }

        Ok(())
    }

    /// Start a named process.
    pub async fn start(&self, name: &str) -> Result<u32> {
        let mut processes = self.processes.write().await;
        let proc = processes
            .get_mut(name)
            .with_context(|| format!("unknown process: {name}"))?;

        if proc.state == ProcessState::Running {
            bail!("process {name} is already running");
        }

        proc.state = ProcessState::Starting;

        let mut cmd = Command::new(&proc.config.command);
        cmd.args(&proc.config.args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawning process: {name}"))?;

        let pid = child.id().unwrap_or(0);

        // Write PID file for orphan cleanup
        tokio::fs::create_dir_all(&self.run_dir).await?;
        let pid_path = self.run_dir.join(format!("{name}.pid"));
        tokio::fs::write(&pid_path, pid.to_string()).await?;

        proc.pid = Some(pid);
        proc.state = ProcessState::Running;
        proc.started_at = Some(Instant::now());

        info!(name, pid, "process started");

        // Spawn output capture task
        let handle = self.spawn_output_capture(name.to_string(), &mut child);
        proc.child_handle = Some(handle);

        // Spawn auto-restart watcher
        let auto_restart = proc.config.auto_restart;
        let process_name = name.to_string();
        let run_dir = self.run_dir.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            let pid_path = run_dir.join(format!("{process_name}.pid"));
            let _ = tokio::fs::remove_file(&pid_path).await;
            match status {
                Ok(s) if s.success() => {
                    info!(name = %process_name, "process exited normally");
                }
                Ok(s) => {
                    warn!(name = %process_name, code = ?s.code(), "process exited with error");
                }
                Err(e) => {
                    error!(name = %process_name, error = %e, "process wait error");
                }
            }
            if auto_restart {
                warn!(name = %process_name, "auto-restart not yet wired to ProcessManager re-start");
            }
        });

        Ok(pid)
    }

    /// Stop a named process.
    pub async fn stop(&self, name: &str, force: bool) -> Result<()> {
        let mut processes = self.processes.write().await;
        let proc = processes
            .get_mut(name)
            .with_context(|| format!("unknown process: {name}"))?;

        if proc.state != ProcessState::Running {
            bail!("process {name} is not running");
        }

        proc.state = ProcessState::Stopping;

        if let Some(pid) = proc.pid {
            let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
            // SAFETY: sending signal to a known child PID
            unsafe {
                libc::kill(pid as i32, signal);
            }
            info!(name, pid, force, "sent stop signal");
        }

        // Clean up PID file
        let pid_path = self.run_dir.join(format!("{name}.pid"));
        let _ = tokio::fs::remove_file(&pid_path).await;

        if let Some(handle) = proc.child_handle.take() {
            handle.abort();
        }

        proc.state = ProcessState::Stopped;
        proc.pid = None;
        proc.started_at = None;

        Ok(())
    }

    /// Restart a named process.
    pub async fn restart(&self, name: &str) -> Result<u32> {
        // Stop first (ignore error if not running)
        let _ = self.stop(name, false).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        self.start(name).await
    }

    /// List all managed processes and their states.
    pub async fn list(&self) -> Vec<ProcessSnapshot> {
        let processes = self.processes.read().await;
        processes
            .values()
            .map(|p| ProcessSnapshot {
                name: p.config.name.clone(),
                state: p.state,
                pid: p.pid.unwrap_or(0),
                uptime_secs: p
                    .started_at
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(0),
                restart_count: p.restart_count,
            })
            .collect()
    }

    /// Subscribe to process output.
    pub fn subscribe_output(&self) -> broadcast::Receiver<OutputLine> {
        self.output_tx.subscribe()
    }

    fn spawn_output_capture(
        &self,
        name: String,
        child: &mut Child,
    ) -> tokio::task::JoinHandle<()> {
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let tx = self.output_tx.clone();

        tokio::spawn(async move {
            let mut tasks = Vec::new();

            if let Some(stdout) = stdout {
                let name = name.clone();
                let tx = tx.clone();
                tasks.push(tokio::spawn(async move {
                    let reader = BufReader::new(stdout);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if line.len() > MAX_LINE_LENGTH {
                            warn!(name = %name, "stdout line exceeds max length, truncating");
                            continue;
                        }
                        let _ = tx.send(OutputLine {
                            name: name.clone(),
                            channel: OutputChannel::Stdout,
                            line,
                            timestamp_us: timestamp_us(),
                        });
                    }
                }));
            }

            if let Some(stderr) = stderr {
                let name = name.clone();
                let tx = tx.clone();
                tasks.push(tokio::spawn(async move {
                    let reader = BufReader::new(stderr);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if line.len() > MAX_LINE_LENGTH {
                            warn!(name = %name, "stderr line exceeds max length, truncating");
                            continue;
                        }
                        let _ = tx.send(OutputLine {
                            name: name.clone(),
                            channel: OutputChannel::Stderr,
                            line,
                            timestamp_us: timestamp_us(),
                        });
                    }
                }));
            }

            for task in tasks {
                let _ = task.await;
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProcessSnapshot {
    pub name: String,
    pub state: ProcessState,
    pub pid: u32,
    pub uptime_secs: u64,
    pub restart_count: u32,
}

fn timestamp_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
