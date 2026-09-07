//! Interactive terminal channel with a cancellable, joined input worker.
//!
//! Crossterm poll/read stay on one worker thread. All terminal output is queued
//! to that worker; the async runtime never blocks waiting for terminal input.

use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, ensure};
use async_trait::async_trait;
use crossterm::cursor::MoveToColumn;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, Clear, ClearType};
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::adapter::{
    ChannelAction, ChannelAdapter, ChannelCapabilities, ChannelEvent, ChannelTarget,
};
use crate::event::{EventBus, ServerEvent};
use crate::module::ModuleLifecycle;
use crate::module::lifecycle::{ModuleContext, ModuleHealth};
use crate::providers::adapter::ConversationBackend;

pub(crate) const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const USER_ID: &str = "cli-user";
const MAX_LINE_CHARS: usize = 4096;
const MAX_HISTORY: usize = 1000;
static TERMINAL_IN_USE: AtomicBool = AtomicBool::new(false);

/// One owner for crossterm's process-global input reader and terminal state.
/// The guard is also used by first-run setup; it owns no detached threads.
pub(crate) struct TerminalSession {
    raw: bool,
}

impl TerminalSession {
    pub(crate) fn acquire() -> anyhow::Result<Self> {
        ensure!(
            io::stdin().is_terminal() && io::stdout().is_terminal(),
            "CLI requires an interactive terminal; use the REST API for non-TTY clients"
        );
        ensure!(
            TERMINAL_IN_USE
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "another Alia terminal input worker is already running"
        );
        let mut session = Self { raw: false };
        ensure!(
            !terminal::is_raw_mode_enabled()?,
            "the host already owns terminal raw mode"
        );
        terminal::enable_raw_mode().context("enabling terminal input")?;
        session.raw = true;
        Ok(session)
    }

    pub(crate) fn restore(&mut self) -> anyhow::Result<()> {
        if self.raw {
            terminal::disable_raw_mode().context("restoring terminal input mode")?;
            self.raw = false;
        }
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            tracing::error!(error = %error, "terminal restoration failed");
        }
        TERMINAL_IN_USE.store(false, Ordering::Release);
    }
}

enum ConsoleOutput {
    Message(String),
    StreamStart,
    StreamChunk(String),
    StreamEnd,
}

struct ConsoleWorker {
    cancel: CancellationToken,
    handle: JoinHandle<anyhow::Result<()>>,
}

pub struct CliChannel {
    id: String,
    event_bus: EventBus,
    cancel_token: Option<CancellationToken>,
    conversation: Option<Arc<dyn ConversationBackend>>,
    worker: Mutex<Option<ConsoleWorker>>,
    output: RwLock<Option<mpsc::Sender<ConsoleOutput>>>,
}

impl CliChannel {
    pub fn new(id: impl Into<String>, event_bus: EventBus) -> Self {
        Self {
            id: id.into(),
            event_bus,
            cancel_token: None,
            conversation: None,
            worker: Mutex::new(None),
            output: RwLock::new(None),
        }
    }

    async fn output(&self, output: ConsoleOutput) -> anyhow::Result<()> {
        let sender = self
            .output
            .read()
            .await
            .clone()
            .context("CLI is not running")?;
        sender
            .send(output)
            .await
            .context("CLI output worker has stopped")
    }
}

#[async_trait]
impl ModuleLifecycle for CliChannel {
    fn name(&self) -> &'static str {
        "cli-channel"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        self.cancel_token = Some(ctx.cancel_token.clone());
        self.conversation = ctx.conversation.clone();
        Ok(())
    }

    async fn start(&self) -> anyhow::Result<()> {
        let mut worker = self.worker.lock().await;
        ensure!(worker.is_none(), "CLI is already running");
        let parent = self.cancel_token.clone().unwrap_or_default();
        ensure!(!parent.is_cancelled(), "cannot start CLI after shutdown");
        let cancel = parent.child_token();
        let cancel_failed_start = cancel.clone().drop_guard();
        let (output_tx, output_rx) = mpsc::channel(64);
        let (ready_tx, ready_rx) = oneshot::channel();
        let bus = self.event_bus.clone();
        let channel_id = self.id.clone();
        let conversation = self.conversation.clone();
        let worker_cancel = cancel.clone();
        let runtime = tokio::runtime::Handle::current();
        let handle = tokio::task::spawn_blocking(move || {
            let _cancel_on_exit = worker_cancel.clone().drop_guard();
            let mut terminal = match TerminalSession::acquire() {
                Ok(terminal) => terminal,
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return Err(error);
                }
            };
            let _ = ready_tx.send(Ok(()));
            let result = run_console(
                output_rx,
                &bus,
                &channel_id,
                conversation.as_deref(),
                &runtime,
                &worker_cancel,
                &parent,
            );
            let restore = terminal.restore();
            result.and(restore)
        });
        match ready_rx
            .await
            .context("CLI worker stopped before initialization")?
        {
            Ok(()) => {
                *self.output.write().await = Some(output_tx);
                *worker = Some(ConsoleWorker { cancel, handle });
                cancel_failed_start.disarm();
                Ok(())
            }
            Err(message) => {
                cancel.cancel();
                let _ = handle.await;
                anyhow::bail!(message)
            }
        }
    }

    async fn stop(&self) -> anyhow::Result<()> {
        let mut worker = self.worker.lock().await;
        self.output.write().await.take();
        if let Some(worker) = worker.take() {
            worker.cancel.cancel();
            // Do not abort or detach: after this await, poll/read has stopped
            // and the terminal guard has restored the original input mode.
            worker.handle.await.context("joining CLI input worker")??;
        }
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        match self.worker.lock().await.as_ref() {
            Some(worker) if !worker.handle.is_finished() => ModuleHealth::Healthy,
            _ => ModuleHealth::Unhealthy("CLI input worker is not running".to_string()),
        }
    }
}

#[async_trait]
impl ChannelAdapter for CliChannel {
    fn channel_type(&self) -> &'static str {
        "cli"
    }
    fn channel_id(&self) -> &str {
        &self.id
    }
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            text: true,
            streaming: true,
            ..Default::default()
        }
    }
    async fn send_message(&self, _target: &ChannelTarget, text: &str) -> anyhow::Result<()> {
        self.output(ConsoleOutput::Message(text.to_owned())).await
    }
    async fn begin_stream(&self, _target: &ChannelTarget) -> anyhow::Result<()> {
        self.output(ConsoleOutput::StreamStart).await
    }
    async fn send_stream_chunk(&self, _target: &ChannelTarget, text: &str) -> anyhow::Result<()> {
        self.output(ConsoleOutput::StreamChunk(text.to_owned()))
            .await
    }
    async fn end_stream(&self, _target: &ChannelTarget, _text: &str) -> anyhow::Result<()> {
        self.output(ConsoleOutput::StreamEnd).await
    }
    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("action '{}' not supported by CLI channel", action.name)
    }
}

fn render_prompt(out: &mut impl Write, line: &str) -> io::Result<()> {
    // Display a bounded tail so wide Unicode input never wraps the editable
    // prompt. The complete input remains available to the conversation.
    let columns = terminal::size().map(|size| size.0).unwrap_or(80);
    let visible: String = line
        .chars()
        .rev()
        .take(columns.saturating_sub(10) as usize / 2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    execute!(out, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
    write!(
        out,
        "alia> {}{}",
        if visible.len() < line.len() {
            "…"
        } else {
            ""
        },
        visible
    )?;
    out.flush()
}

fn print_text(out: &mut impl Write, text: &str) -> io::Result<()> {
    // Raw mode disables the terminal's LF -> CRLF output conversion.
    write!(out, "{}", text.replace("\r\n", "\n").replace('\n', "\r\n"))?;
    out.flush()
}

fn run_console(
    mut output: mpsc::Receiver<ConsoleOutput>,
    bus: &EventBus,
    channel_id: &str,
    conversation: Option<&dyn ConversationBackend>,
    runtime: &tokio::runtime::Handle,
    cancel: &CancellationToken,
    parent: &CancellationToken,
) -> anyhow::Result<()> {
    let mut out = io::stdout();
    let mut line = String::new();
    let mut history = Vec::<String>::new();
    let mut history_index: usize = 0;
    let mut busy = false;
    render_prompt(&mut out, &line)?;
    while !cancel.is_cancelled() {
        if output.is_closed() && output.is_empty() {
            break;
        }
        while let Ok(message) = output.try_recv() {
            if cancel.is_cancelled() {
                break;
            }
            match message {
                ConsoleOutput::Message(text) => {
                    execute!(out, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
                    print_text(&mut out, &format!("alia: {text}\n"))?;
                    busy = false;
                    render_prompt(&mut out, &line)?;
                }
                ConsoleOutput::StreamStart => {
                    execute!(out, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
                    print_text(&mut out, "alia: ")?;
                    busy = true;
                }
                ConsoleOutput::StreamChunk(text) => print_text(&mut out, &text)?,
                ConsoleOutput::StreamEnd => {
                    print_text(&mut out, "\n")?;
                    busy = false;
                    render_prompt(&mut out, &line)?;
                }
            }
        }
        if !event::poll(INPUT_POLL_INTERVAL)? || cancel.is_cancelled() {
            continue;
        }
        // poll(true) guarantees read will not block when called on this thread.
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => {
                    parent.cancel();
                    break;
                }
                KeyCode::Char('c') if !busy => {
                    line.clear();
                    render_prompt(&mut out, &line)?;
                }
                _ => {}
            }
            continue;
        }
        if busy {
            continue;
        }
        match key.code {
            KeyCode::Char(character) if line.chars().count() < MAX_LINE_CHARS => {
                line.push(character)
            }
            KeyCode::Backspace => {
                line.pop();
            }
            KeyCode::Up => {
                history_index = history_index.saturating_sub(1);
                if let Some(previous) = history.get(history_index) {
                    line = previous.clone();
                }
            }
            KeyCode::Down => {
                history_index = (history_index + 1).min(history.len());
                line = history.get(history_index).cloned().unwrap_or_default();
            }
            KeyCode::Enter => {
                print_text(&mut out, "\n")?;
                let text = std::mem::take(&mut line).trim().to_string();
                if !text.is_empty() {
                    if history.last() != Some(&text) {
                        history.push(text.clone());
                    }
                    if history.len() > MAX_HISTORY {
                        history.remove(0);
                    }
                    history_index = history.len();
                    if matches!(text.as_str(), "/quit" | "/exit") {
                        parent.cancel();
                        break;
                    }
                    if text == "/clear" {
                        execute!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))?;
                    } else if text == "/help" {
                        print_text(
                            &mut out,
                            "/help  /quit  /clear  /session new|list|switch <uuid>\nBackspace edits; Up/Down recall this session's input; Ctrl+C clears; Ctrl+D exits.\n",
                        )?;
                    } else if text.split_whitespace().next() == Some("/session") {
                        let reply = runtime.block_on(async {
                            tokio::select! {
                                _ = cancel.cancelled() => Ok(String::new()),
                                reply = session_command(conversation, channel_id, &text) => reply,
                            }
                        });
                        print_text(
                            &mut out,
                            &format!(
                                "{}\n",
                                reply.unwrap_or_else(|error| format!("Error: {error}"))
                            ),
                        )?;
                    } else {
                        busy = true;
                        bus.publish(ServerEvent::ChannelInput(ChannelEvent::TextMessage {
                            channel_id: channel_id.to_owned(),
                            user_id: USER_ID.to_owned(),
                            text,
                        }));
                        print_text(&mut out, "thinking…")?;
                    }
                }
            }
            _ => {}
        }
        if !busy {
            render_prompt(&mut out, &line)?;
        }
    }
    print_text(&mut out, "\n")?;
    Ok(())
}

async fn session_command(
    conversation: Option<&dyn ConversationBackend>,
    channel_id: &str,
    input: &str,
) -> anyhow::Result<String> {
    let conversation = conversation.context("conversation backend unavailable")?;
    let command = input.split_whitespace().collect::<Vec<_>>();
    match command.as_slice() {
        ["/session", "new"] => {
            let id = conversation.create_session(channel_id, USER_ID).await?;
            conversation
                .set_active_session(channel_id, USER_ID, id)
                .await?;
            Ok(format!("Started new session: {id}"))
        }
        ["/session", "list"] => {
            let sessions = conversation.list_sessions(channel_id, USER_ID).await?;
            Ok(sessions
                .iter()
                .take(10)
                .map(|(id, name, updated)| {
                    format!(
                        "{id} ({}) - {updated}",
                        name.as_deref().unwrap_or("Unnamed")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        ["/session", "switch", id] => {
            let id = id.parse().context("invalid session UUID")?;
            conversation
                .set_active_session(channel_id, USER_ID, id)
                .await?;
            Ok(format!("Switched to session: {id}"))
        }
        _ => anyhow::bail!("Usage: /session new|list|switch <uuid>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn output_without_a_running_worker_fails_explicitly() {
        let channel = CliChannel::new("cli", EventBus::new(4));
        assert!(
            channel
                .begin_stream(&ChannelTarget {
                    voice: None,
                    user_id: USER_ID.to_owned()
                })
                .await
                .is_err()
        );
        channel.stop().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a real PTY; run with --ignored --nocapture --test-threads=1"]
    async fn idle_terminal_worker_joins_and_restores_terminal_on_shutdown() {
        assert!(io::stdin().is_terminal() && io::stdout().is_terminal());
        assert!(!terminal::is_raw_mode_enabled().unwrap());
        let parent = CancellationToken::new();
        let mut channel = CliChannel::new("cli-test", EventBus::new(4));
        channel.cancel_token = Some(parent.clone());
        for _ in 0..2 {
            channel.start().await.unwrap();
            assert!(terminal::is_raw_mode_enabled().unwrap());
            let competing = CliChannel::new("competing-cli", EventBus::new(4));
            assert!(competing.start().await.is_err());
            assert!(terminal::is_raw_mode_enabled().unwrap());
            tokio::time::timeout(Duration::from_secs(2), channel.stop())
                .await
                .unwrap()
                .unwrap();
            assert!(!terminal::is_raw_mode_enabled().unwrap());
            assert!(!TERMINAL_IN_USE.load(Ordering::Acquire));
            assert!(!parent.is_cancelled());
        }
    }
}
