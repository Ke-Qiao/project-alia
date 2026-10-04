use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use alia_transport::connections::Connections;
use anyhow::Context;
use axum::Router;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::io::{self, IsTerminal, Write};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::channels::cli::{INPUT_POLL_INTERVAL, TerminalSession};
use crate::oobe::bootstrap::{
    SetupResult, default_config_target, finish_setup, print_banner, release_example_config,
};
use crate::oobe::routes::{OobeState, routes};

/// Complete the existing CLI/HTTP setup flow without terminating the host.
/// Returns the generated configuration path, or `None` when setup is cancelled.
pub async fn run_setup(shutdown: CancellationToken) -> anyhow::Result<Option<PathBuf>> {
    const OOBE_PORT: u16 = 3000;
    if shutdown.is_cancelled() {
        return Ok(None);
    }
    let listener = bind_setup_listener(OOBE_PORT).await?;
    print_banner(OOBE_PORT);
    run_setup_on(
        listener,
        shutdown,
        io::stdin().is_terminal() && io::stdout().is_terminal(),
    )
    .await
}

async fn run_setup_on(
    listener: tokio::net::TcpListener,
    shutdown: CancellationToken,
    interactive: bool,
) -> anyhow::Result<Option<PathBuf>> {
    let cancel = shutdown.child_token();
    let _cancel_on_drop = cancel.clone().drop_guard();
    if cancel.is_cancelled() {
        return Ok(None);
    }
    let (done_tx, mut done_rx) = watch::channel::<Option<SetupResult>>(None);
    let done_tx = Arc::new(done_tx);
    let mut input = if interactive {
        let done_tx = (*done_tx).clone();
        let input_cancel = cancel.clone();
        Some(tokio::task::spawn_blocking(move || {
            setup_input(done_tx, input_cancel)
        }))
    } else {
        tracing::info!("setup has no interactive terminal; use the HTTP setup API");
        None
    };
    let app = Router::new().merge(routes()).with_state(OobeState {
        done_tx: done_tx.clone(),
        cancel: cancel.clone(),
    });
    let connections = SetupConnections(Connections::new());
    let server = axum::serve(
        super::runtime::HttpListener {
            listener,
            connections: connections.0.clone(),
        },
        app,
    )
    .with_graceful_shutdown(cancel.clone().cancelled_owned());
    let server = std::future::IntoFuture::into_future(server);
    tokio::pin!(server);

    let mut input_completed = false;
    let mut server_completed = false;
    let served = tokio::select! {
        _ = cancel.cancelled() => Ok(()),
        result = &mut server => {
            server_completed = true;
            result.context("serving setup HTTP")
        },
        result = done_rx.wait_for(|value| value.is_some()) => {
            result.map(|_| ()).context("setup completion channel closed")
        }
        result = async {
            match input.as_mut() {
                Some(input) => input.await.context("joining setup input worker")?,
                None => std::future::pending().await,
            }
        } => {
            input_completed = true;
            result
        }
    };
    if cancel.is_cancelled() || served.is_err() || input_completed {
        // A request whose body finishes during drain cannot publish a new
        // configuration after cancellation or input failure.
        let _ = finish_setup(&done_tx, || Ok(SetupResult::Cancelled));
    }
    cancel.cancel();
    // ConfigReady can precede the successful response being written. Preserve
    // the original server until that response drains. Stalled peers do not own
    // shutdown: close their actual sockets after the bounded grace period.
    let drained = if server_completed {
        Ok(())
    } else {
        match tokio::time::timeout(Duration::from_secs(2), &mut server).await {
            Ok(result) => result.context("draining setup HTTP"),
            Err(_) => {
                connections.0.close();
                server.await.context("closing setup HTTP connections")
            }
        }
    };
    // HTTP completion and caller cancellation must release stdin before return.
    if !input_completed && let Some(input) = input {
        input.await.context("joining setup input worker")??;
    }
    served?;
    drained?;
    if shutdown.is_cancelled() {
        return Ok(None);
    }
    let result = done_rx.borrow().clone();
    match result {
        Some(SetupResult::ConfigReady(path)) => Ok(Some(path)),
        Some(SetupResult::Cancelled) => Ok(None),
        None => anyhow::bail!("setup server exited before setup completed"),
    }
}

struct SetupConnections(Arc<Connections>);
impl Drop for SetupConnections {
    fn drop(&mut self) {
        self.0.close();
    }
}

async fn bind_setup_listener(port: u16) -> anyhow::Result<tokio::net::TcpListener> {
    // Setup can write local configuration before any account exists. Keep it
    // local; remote operators can use an authenticated SSH tunnel.
    tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .context("binding local setup HTTP listener")
}

fn setup_input(
    done: watch::Sender<Option<SetupResult>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let mut terminal = TerminalSession::acquire()?;
    let mut line = String::new();
    let result = (|| {
        while !cancel.is_cancelled() {
            if !event::poll(INPUT_POLL_INTERVAL)? || cancel.is_cancelled() {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                let _ = finish_setup(&done, || Ok(SetupResult::Cancelled));
                break;
            }
            match key.code {
                KeyCode::Char(character) if line.len() < 8 => {
                    line.push(character);
                }
                KeyCode::Backspace => {
                    line.pop();
                }
                KeyCode::Enter => match line.trim().to_ascii_lowercase().as_str() {
                    "c" => {
                        // HTTP may have completed while terminal input was
                        // pending. A losing CLI must neither write nor replace it.
                        if let Err(error) = finish_setup(&done, || {
                            if cancel.is_cancelled() {
                                return Ok(SetupResult::Cancelled);
                            }
                            release_example_config(&default_config_target())
                                .map(SetupResult::ConfigReady)
                        }) && done.borrow().is_none()
                        {
                            return Err(error);
                        }
                        break;
                    }
                    "q" => {
                        let _ = finish_setup(&done, || Ok(SetupResult::Cancelled));
                        break;
                    }
                    _ => line.clear(),
                },
                _ => {}
            }
            write!(io::stdout(), "\r\x1b[2Ksetup> {line}")?;
            io::stdout().flush()?;
        }
        Ok(())
    })();
    let restored = terminal.restore();
    result.and(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unauthenticated_setup_listener_is_loopback_only() {
        let listener = bind_setup_listener(0).await.unwrap();
        assert_eq!(
            listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        );
    }

    #[tokio::test]
    async fn setup_completion_delivers_response_and_closes_stalled_connections() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = bind_setup_listener(0).await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let _stop_on_drop = stop.clone().drop_guard();
        let serving = tokio::spawn(run_setup_on(listener, stop, false));
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let template: serde_json::Value = client
            .post(format!("http://{address}/api/v1/setup/generate-config"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut stalled = tokio::net::TcpStream::connect(address).await.unwrap();
        stalled.write_all(b"POST /api/v1/setup/save-config HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 10000\r\n\r\n{").await.unwrap();
        let directory =
            std::env::temp_dir().join(format!("alia-setup-owner-{}", uuid::Uuid::new_v4()));
        let target = directory.join("server.toml");
        let response: serde_json::Value = client
            .post(format!("http://{address}/api/v1/setup/save-config"))
            .json(&serde_json::json!({"path": target, "config": template["template"]}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .expect("ConfigReady must not drop the successful HTTP response");
        assert_eq!(response["success"], true, "{response}");
        let completed = tokio::time::timeout(Duration::from_secs(4), serving)
            .await
            .expect("an incomplete peer body must not retain setup")
            .unwrap()
            .unwrap();
        assert_eq!(completed, Some(target.clone()));
        crate::config::ServerConfig::load(&target).unwrap();
        let mut remaining = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), stalled.read_to_end(&mut remaining))
            .await
            .expect("accepted setup socket must be closed")
            .unwrap();
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn cancelled_setup_does_not_bind_or_read_terminal() {
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        assert!(run_setup(shutdown).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn cancellation_rejects_an_accepted_save_whose_body_finishes_during_drain() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = bind_setup_listener(0).await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let _stop_on_drop = stop.clone().drop_guard();
        let serving = tokio::spawn(run_setup_on(listener, stop.clone(), false));
        let directory =
            std::env::temp_dir().join(format!("alia-setup-cancel-{}", uuid::Uuid::new_v4()));
        let target = directory.join("server.toml");
        let body = serde_json::to_vec(&serde_json::json!({
            "path": target,
            "config": crate::oobe::bootstrap::example_config().unwrap(),
        }))
        .unwrap();
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket.write_all(format!("POST /api/v1/setup/save-config HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
        // 100 Continue proves the accepted request is waiting for its body;
        // no sleep or scheduler timing assumption is needed.
        let mut accepted = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !accepted.ends_with(b"\r\n\r\n") {
                accepted.push(socket.read_u8().await.unwrap());
                assert!(accepted.len() < 1024);
            }
        })
        .await
        .unwrap();
        assert!(
            String::from_utf8(accepted)
                .unwrap()
                .contains("100 Continue")
        );
        stop.cancel();
        let _ = socket.write_all(&body).await;
        let completed = tokio::time::timeout(Duration::from_secs(4), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(completed, None);
        assert!(
            !target.exists(),
            "drain cannot publish after caller cancellation"
        );
        assert!(!directory.exists());
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    }

    #[tokio::test]
    #[ignore = "requires a real PTY; run with --ignored --nocapture --test-threads=1"]
    async fn setup_terminal_worker_releases_input_after_cancellation() {
        assert!(io::stdin().is_terminal() && io::stdout().is_terminal());
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let (done, _result) = watch::channel(None);
        let worker = tokio::task::spawn_blocking(move || setup_input(done, worker_cancel));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !crossterm::terminal::is_raw_mode_enabled().unwrap() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!crossterm::terminal::is_raw_mode_enabled().unwrap());
    }
}
