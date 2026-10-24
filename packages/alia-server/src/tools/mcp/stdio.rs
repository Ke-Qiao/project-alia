use super::MAX_MESSAGE_BYTES;
use futures::{SinkExt, StreamExt};
use rmcp::{
    RoleClient,
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::{Transport, async_rw::JsonRpcMessageCodec},
};
use sha2::{Digest, Sha256};
use std::{io, path::Path, process::Stdio, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};
use tokio_util::{
    codec::{FramedRead, FramedWrite},
    sync::CancellationToken,
};

pub(super) struct StdioTransport {
    read: FramedRead<ChildStdout, JsonRpcMessageCodec<ServerJsonRpcMessage>>,
    write: Arc<Mutex<Option<FramedWrite<ChildStdin, JsonRpcMessageCodec<ClientJsonRpcMessage>>>>>,
    broken: CancellationToken,
}
impl Transport<RoleClient> for StdioTransport {
    type Error = io::Error;
    fn send(
        &mut self,
        item: ClientJsonRpcMessage,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send + 'static {
        let write = self.write.clone();
        async move {
            if serde_json::to_vec(&item)?.len() > MAX_MESSAGE_BYTES {
                return Err(io::Error::other("MCP outgoing message budget"));
            }
            let mut guard = write.lock().await;
            guard
                .as_mut()
                .ok_or_else(|| io::Error::other("MCP stdin closed"))?
                .send(item)
                .await
                .map_err(io::Error::other)
        }
    }
    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        match self.read.next().await {
            Some(Ok(message)) => Some(message),
            _ => {
                self.broken.cancel();
                None
            }
        }
    }
    async fn close(&mut self) -> io::Result<()> {
        self.write.lock().await.take();
        Ok(())
    }
}

pub(super) struct ChildOwner {
    child: Child,
    stderr: tokio::task::JoinHandle<io::Result<()>>,
    _executable: ExecutableSnapshot,
}

/// Execute the exact bytes that were checked; renaming or rewriting the source
/// between validation and spawn must not change the executable identity.
struct ExecutableSnapshot(std::path::PathBuf);
impl Drop for ExecutableSnapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("server"));
        let _ = std::fs::remove_dir(&self.0);
    }
}
impl ChildOwner {
    pub async fn spawn(
        program: &Path,
        expected_sha256: &str,
        args: &[String],
        env: &std::collections::BTreeMap<String, String>,
        broken: CancellationToken,
    ) -> anyhow::Result<(Self, StdioTransport)> {
        anyhow::ensure!(
            program.is_absolute(),
            "MCP executable must be an absolute path"
        );
        anyhow::ensure!(
            expected_sha256.len() == 64,
            "MCP executable SHA-256 is required"
        );
        anyhow::ensure!(
            args.len() <= 128 && env.len() <= 128,
            "MCP process configuration count budget"
        );
        anyhow::ensure!(
            args.iter().map(String::len).sum::<usize>()
                + env.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>()
                <= 64 * 1024,
            "MCP process configuration byte budget"
        );
        let mut open = tokio::fs::OpenOptions::new();
        open.read(true);
        #[cfg(unix)]
        open.custom_flags(libc::O_NOFOLLOW);
        let file = open.open(program).await?;
        let metadata = file.metadata().await?;
        const MAX_EXECUTABLE: u64 = 256 * 1024 * 1024;
        anyhow::ensure!(
            metadata.is_file() && metadata.len() <= MAX_EXECUTABLE,
            "MCP executable must be a bounded regular file"
        );
        let mut bytes = Vec::new();
        file.take(MAX_EXECUTABLE + 1)
            .read_to_end(&mut bytes)
            .await?;
        anyhow::ensure!(
            bytes.len() <= MAX_EXECUTABLE as usize
                && format!("{:x}", Sha256::digest(&bytes)) == expected_sha256,
            "MCP executable identity changed"
        );
        let dir = std::env::temp_dir().join(format!("alia-mcp-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir)?;
        let snapshot = ExecutableSnapshot(dir);
        let executable = snapshot.0.join("server");
        let mut copy = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&executable)
            .await?;
        copy.write_all(&bytes).await?;
        copy.flush().await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            copy.set_permissions(std::fs::Permissions::from_mode(0o500))
                .await?;
        }
        drop(copy);
        drop(bytes);
        let mut command = Command::new(&executable);
        command
            .args(args)
            .env_clear()
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(not(unix))]
        anyhow::bail!("MCP stdio process ownership requires Unix on this build");
        let mut child = command.spawn()?;
        let write = child.stdin.take().expect("piped stdin");
        let read = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let stderr_cancel = broken.clone();
        let stderr = tokio::spawn(async move {
            let mut bytes = [0u8; 4096];
            let mut total = 0usize;
            loop {
                let count = stderr.read(&mut bytes).await?;
                if count == 0 {
                    return Ok(());
                }
                total = total.saturating_add(count);
                if total > 256 * 1024 {
                    stderr_cancel.cancel();
                }
                // Always drain, never retain or log untrusted stderr/credentials.
            }
        });
        Ok((
            Self {
                child,
                stderr,
                _executable: snapshot,
            },
            StdioTransport {
                read: FramedRead::new(
                    read,
                    JsonRpcMessageCodec::new_with_max_length(MAX_MESSAGE_BYTES),
                ),
                write: Arc::new(Mutex::new(Some(FramedWrite::new(
                    write,
                    JsonRpcMessageCodec::new_with_max_length(MAX_MESSAGE_BYTES),
                )))),
                broken,
            },
        ))
    }
    pub async fn close(mut self) -> anyhow::Result<()> {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            // Never reap before group termination: retain the original PID owner
            // even after a cleanup failure. A stop timeout may retain this task.
            let mut reported = false;
            loop {
                match kill_process_group(pid) {
                    Ok(()) => break,
                    Err(error) => {
                        if !reported {
                            tracing::error!(pid, error=%error, "MCP process cleanup pending");
                            reported = true;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
            }
        }
        self.child.wait().await?;
        self.stderr.await??;
        Ok(())
    }
}
#[cfg(unix)]
fn child_has_exited(pid: u32) -> io::Result<bool> {
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
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { info.si_pid() } != 0)
}
#[cfg(unix)]
fn kill_process_group(pid: u32) -> io::Result<()> {
    if unsafe { libc::kill(-(pid as i32), libc::SIGKILL) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    if error.raw_os_error() == Some(libc::EPERM) && child_has_exited(pid)? {
        let mut members = [0_i32; 2];
        let bytes = unsafe {
            libc::proc_listpids(
                2,
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[tokio::test]
    async fn stdio_hash_mismatch_is_rejected_and_original_process_group_is_joined() {
        let dir = std::env::temp_dir().join(format!("alia-mcp-fixture-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let program = dir.join("fixture.sh");
        let bytes=b"#!/bin/sh\nprintf '%s\\n' '{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}'\n/bin/sleep 60 &\nwait\n";
        tokio::fs::write(&program, bytes).await.unwrap();
        tokio::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
            .await
            .unwrap();
        assert!(
            ChildOwner::spawn(
                &program,
                &"0".repeat(64),
                &[],
                &Default::default(),
                CancellationToken::new()
            )
            .await
            .is_err()
        );
        let digest = format!("{:x}", Sha256::digest(bytes));
        let (owner, mut transport) = ChildOwner::spawn(
            &program,
            &digest,
            &[],
            &Default::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let pid = owner.child.id().unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(3), transport.receive())
                .await
                .unwrap()
                .is_some()
        );
        tokio::fs::write(&program, b"#!/bin/sh\nexit 1\n")
            .await
            .unwrap();
        assert!(
            ChildOwner::spawn(
                &program,
                &digest,
                &[],
                &Default::default(),
                CancellationToken::new()
            )
            .await
            .is_err()
        );
        transport.close().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), owner.close())
            .await
            .unwrap()
            .unwrap();
        drop(transport);
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }
}
