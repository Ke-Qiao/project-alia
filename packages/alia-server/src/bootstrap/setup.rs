use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::io::{self, IsTerminal, Write};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;

use crate::channels::cli::{INPUT_POLL_INTERVAL, TerminalSession};
use crate::discovery::{ServerDiscoveryAdvertisement, ServerDiscoveryAnnouncement};
use crate::oobe::bootstrap::{
    SetupResult, default_config_target, print_banner, release_example_config,
};
use crate::oobe::routes::{OobeState, routes};

/// Complete the existing CLI/HTTP setup flow without terminating the host.
/// Returns the generated configuration path, or `None` when setup is cancelled.
pub async fn run_setup(shutdown: CancellationToken) -> anyhow::Result<Option<PathBuf>> {
    const OOBE_PORT: u16 = 3000;
    let cancel = shutdown.child_token();
    let _cancel_on_drop = cancel.clone().drop_guard();
    if cancel.is_cancelled() {
        return Ok(None);
    }
    let (done_tx, mut done_rx) = watch::channel::<Option<SetupResult>>(None);
    let done_tx = Arc::new(done_tx);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", OOBE_PORT))
        .await
        .context("binding setup HTTP listener")?;

    print_banner(OOBE_PORT);
    let mut input = if io::stdin().is_terminal() && io::stdout().is_terminal() {
        let done_tx = (*done_tx).clone();
        let input_cancel = cancel.clone();
        Some(tokio::task::spawn_blocking(move || {
            setup_input(done_tx, input_cancel)
        }))
    } else {
        tracing::info!("setup has no interactive terminal; use the HTTP setup API");
        None
    };
    let app = Router::new()
        .merge(routes())
        .layer(CorsLayer::permissive())
        .with_state(OobeState { done_tx });
    let app = super::attach_frontend(app);
    let discovery =
        match ServerDiscoveryAdvertisement::start(ServerDiscoveryAnnouncement::oobe(OOBE_PORT))
            .await
        {
            Ok(discovery) => discovery,
            Err(error) => {
                tracing::warn!(error = %error, "setup discovery unavailable");
                None
            }
        };

    let mut input_completed = false;
    let served = tokio::select! {
        _ = cancel.cancelled() => Ok(()),
        result = axum::serve(listener, app) => result.context("serving setup HTTP"),
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
    cancel.cancel();
    let discovery_result = if let Some(discovery) = discovery {
        discovery.shutdown().await
    } else {
        Ok(())
    };
    // HTTP completion and caller cancellation must release stdin before return.
    if !input_completed && let Some(input) = input {
        input.await.context("joining setup input worker")??;
    }
    served?;
    discovery_result?;
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
                let _ = done.send(Some(SetupResult::Cancelled));
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
                        let path = release_example_config(&default_config_target())?;
                        let _ = done.send(Some(SetupResult::ConfigReady(path)));
                        break;
                    }
                    "q" => {
                        let _ = done.send(Some(SetupResult::Cancelled));
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
    async fn cancelled_setup_does_not_bind_or_read_terminal() {
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        assert!(run_setup(shutdown).await.unwrap().is_none());
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
