use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MAX_OUTPUT: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct ProcessOwner {
    pub(super) plugins: Arc<RwLock<HashMap<String, PluginEntry>>>,
    pub(super) events: broadcast::Sender<PluginEventRecord>,
    pub(super) name: String,
    pub(super) instance: String,
    pub(super) stop: CancellationToken,
    pub(super) ready: CancellationToken,
}

impl ProcessOwner {
    pub(super) async fn run(
        self,
        mut child: tokio::process::Child,
        mut stdin: tokio::process::ChildStdin,
        stdout: tokio::process::ChildStdout,
        stderr: tokio::process::ChildStderr,
        mut commands: mpsc::Receiver<Vec<u8>>,
    ) {
        let pid = child.id();
        let out_owner = self.clone();
        let out = tokio::spawn(async move { out_owner.read_output(stdout, true).await });
        let err_owner = self.clone();
        let err = tokio::spawn(async move { err_owner.read_output(stderr, false).await });
        let startup_deadline = tokio::time::sleep(std::time::Duration::from_secs(5));
        tokio::pin!(startup_deadline);
        let observation = loop {
            tokio::select! {
                biased;
                _ = self.stop.cancelled() => break Ok(()),
                _ = &mut startup_deadline, if !self.ready.is_cancelled() => self.stop.cancel(),
                result = wait_for_unreaped_exit(pid.expect("spawned child has a PID")) => break result,
                command = commands.recv() => {
                    let Some(command) = command else { self.stop.cancel(); continue; };
                    let write = async { stdin.write_all(&command).await?; stdin.flush().await };
                    let written = tokio::select! {
                        biased;
                        _ = self.stop.cancelled() => false,
                        result = tokio::time::timeout(std::time::Duration::from_secs(2), write) => matches!(result, Ok(Ok(()))),
                    };
                    if !written { self.stop.cancel(); }
                }
            }
        };
        drop(stdin);
        let requested_stop = {
            let mut plugins = self.plugins.write().await;
            if let Some(entry) = plugins.get_mut(&self.name)
                && entry
                    .run
                    .as_ref()
                    .is_some_and(|run| run.instance_id == self.instance)
            {
                let requested = entry.state == PluginState::Stopping;
                entry.state = PluginState::Stopping;
                requested
            } else {
                false
            }
        };
        // shortcut: trusted local groups only; use a sandbox before admitting code that can escape with setsid.
        // Keep the leader unreaped until group termination; its PID cannot be reused.
        if let Err(error) = observation {
            self.record_cleanup_error(error.to_string()).await;
            std::future::pending::<()>().await;
        }
        loop {
            match kill_process_group(pid.expect("spawned child has a PID")) {
                Ok(()) => break,
                Err(error) => {
                    self.record_cleanup_error(error.to_string()).await;
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
        let status = child.wait().await;
        let out = out.await;
        let err = err.await;
        if status.is_err() || out.is_err() || err.is_err() {
            self.record_cleanup_error("process wait or pipe owner join failed".into())
                .await;
            return;
        }
        let success = status.as_ref().is_ok_and(|status| status.success())
            && matches!(out, Ok(Ok(())))
            && matches!(err, Ok(Ok(())));
        let mut plugins = self.plugins.write().await;
        if let Some(entry) = plugins.get_mut(&self.name)
            && entry
                .run
                .as_ref()
                .is_some_and(|run| run.instance_id == self.instance)
        {
            entry.state = if requested_stop || success {
                PluginState::Stopped
            } else {
                PluginState::Failed
            };
            entry.run = None;
            entry.runtime_capabilities.clear();
            emit(
                &self.events,
                &self.name,
                &self.instance,
                "",
                "lifecycle",
                serde_json::json!({"state":if entry.state == PluginState::Stopped {"stopped"} else {"failed"},"exit_code":status.ok().and_then(|s| s.code())}),
            );
        }
    }

    async fn record_cleanup_error(&self, error: String) {
        let mut plugins = self.plugins.write().await;
        if let Some(entry) = plugins.get_mut(&self.name)
            && let Some(run) = entry.run.as_mut()
            && run.instance_id == self.instance
        {
            entry.state = PluginState::Stopping;
            run.cleanup_error = Some(error.clone());
            emit(
                &self.events,
                &self.name,
                &self.instance,
                "",
                "cleanup_pending",
                serde_json::json!({"error":error}),
            );
        }
    }

    async fn read_output(
        self,
        stream: impl tokio::io::AsyncRead + Unpin,
        protocol: bool,
    ) -> Result<()> {
        let result = self.read_output_inner(stream, protocol).await;
        if let Err(error) = &result {
            warn!(plugin = %self.name, instance = %self.instance, %error, "plugin pipe failed; stopping its owner");
            self.stop.cancel();
        }
        result
    }

    async fn read_output_inner(
        &self,
        stream: impl tokio::io::AsyncRead + Unpin,
        protocol: bool,
    ) -> Result<()> {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        let mut total = 0usize;
        loop {
            let buffer = reader.fill_buf().await?;
            if buffer.is_empty() {
                ensure!(line.is_empty(), "incomplete plugin output frame");
                break;
            }
            let consumed = buffer
                .iter()
                .position(|b| *b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(buffer.len());
            total = total.saturating_add(consumed);
            ensure!(
                total <= MAX_OUTPUT && line.len() + consumed <= MAX_MESSAGE,
                "plugin output exceeds budget"
            );
            line.extend_from_slice(&buffer[..consumed]);
            let complete = line.last() == Some(&b'\n');
            reader.consume(consumed);
            if !complete {
                continue;
            }
            if protocol {
                let value: serde_json::Value =
                    serde_json::from_slice(&line).context("invalid plugin output JSON")?;
                let event = value
                    .get("event_type")
                    .and_then(serde_json::Value::as_str)
                    .context("plugin event_type missing")?;
                let payload = value
                    .get("payload")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let mut command_id = "";
                let mut plugins = self.plugins.write().await;
                let Some(entry) = plugins.get_mut(&self.name) else {
                    break;
                };
                ensure!(
                    entry
                        .run
                        .as_ref()
                        .is_some_and(|run| run.instance_id == self.instance),
                    "plugin output instance retired"
                );
                if event == "ready" {
                    ensure!(
                        payload
                            .get("instance_id")
                            .and_then(serde_json::Value::as_str)
                            == Some(&self.instance)
                            && payload
                                .get("protocol_version")
                                .and_then(serde_json::Value::as_u64)
                                == Some(1),
                        "plugin ready handshake mismatch"
                    );
                    if entry.state == PluginState::Starting {
                        entry.state = PluginState::Running;
                        self.ready.cancel();
                    }
                } else if event == "command_result" {
                    ensure!(
                        payload
                            .get("instance_id")
                            .and_then(serde_json::Value::as_str)
                            == Some(&self.instance),
                        "command result instance mismatch"
                    );
                    command_id = payload
                        .get("command_id")
                        .and_then(serde_json::Value::as_str)
                        .context("command result ID missing")?;
                    let state = payload
                        .get("state")
                        .and_then(serde_json::Value::as_str)
                        .context("command result state missing")?;
                    ensure!(
                        matches!(state, "completed" | "failed" | "cancelled"),
                        "invalid command result state"
                    );
                    let record = entry
                        .commands
                        .get_mut(command_id)
                        .context("unknown command result ID")?;
                    ensure!(
                        record.receipt.instance_id == self.instance,
                        "command belongs to a different instance"
                    );
                    let result_json = serde_json::to_vec(
                        &payload
                            .get("result")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )?;
                    ensure!(
                        record.receipt.state == "unknown"
                            || (record.receipt.state == state
                                && record.receipt.result_json == result_json),
                        "conflicting terminal command result"
                    );
                    record.receipt.state = state.to_owned();
                    record.receipt.result_json = result_json;
                } else if event == "expression_state" && entry.state == PluginState::Running {
                    entry.runtime_capabilities =
                        runtime_expression_capabilities(&serde_json::to_vec(&payload)?);
                }
                emit(
                    &self.events,
                    &self.name,
                    &self.instance,
                    command_id,
                    event,
                    payload.clone(),
                );
            } else {
                // Log content is untrusted and may contain secrets; expose only its size.
                emit(
                    &self.events,
                    &self.name,
                    &self.instance,
                    "",
                    "stderr",
                    serde_json::json!({"bytes":line.len()}),
                );
            }
            line.clear();
        }
        Ok(())
    }
}

#[cfg(unix)]
fn child_has_exited(pid: u32) -> std::io::Result<bool> {
    // Keep the original child unreaped until group cleanup, so its PID cannot be reused.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    } < 0
    {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
        return Ok(false);
    }
    Ok(unsafe { info.si_pid() } != 0)
}

#[cfg(unix)]
async fn wait_for_unreaped_exit(pid: u32) -> std::io::Result<()> {
    while !child_has_exited(pid)? {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    Ok(())
}

#[cfg(unix)]
fn kill_process_group(pid: u32) -> std::io::Result<()> {
    if unsafe { libc::kill(-(pid as i32), libc::SIGKILL) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    if error.raw_os_error() == Some(libc::EPERM) && child_has_exited(pid)? {
        // Darwin reports EPERM for a group containing only its unreaped, exited leader.
        const PROC_PGRP_ONLY: u32 = 2;
        let mut members = [0_i32; 2];
        let bytes = unsafe {
            libc::proc_listpids(
                PROC_PGRP_ONLY,
                pid,
                members.as_mut_ptr().cast(),
                std::mem::size_of_val(&members) as i32,
            )
        };
        if bytes == std::mem::size_of::<i32>() as i32 && members[0] == pid as i32 {
            return Ok(());
        }
    }
    Err(error)
}

#[cfg(not(unix))]
async fn wait_for_unreaped_exit(_: u32) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "plugin process ownership is supported on Unix only",
    ))
}

#[cfg(not(unix))]
fn kill_process_group(_: u32) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "plugin process ownership is supported on Unix only",
    ))
}
