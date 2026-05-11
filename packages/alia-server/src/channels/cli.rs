use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rustyline::error::ReadlineError;
use rustyline::{Config, DefaultEditor, EditMode};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::event::EventBus;
use crate::module::ModuleLifecycle;
use crate::module::lifecycle::{ModuleContext, ModuleHealth};

use super::adapter::{
    ChannelAction, ChannelAdapter, ChannelCapabilities, ChannelEvent, ChannelTarget,
};

const PROMPT: &str = "\x1b[36malia>\x1b[0m ";
const HISTORY_FILE: &str = ".alia_cli_history";

/// Braille-dot spinner frames for a smooth loading animation.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_INTERVAL_MS: u64 = 80;

/// CLI channel adapter — interactive REPL powered by rustyline.
///
/// Features: line editing, input history (persisted to ~/.alia_cli_history),
/// Ctrl+C (cancel current input) and Ctrl+D (graceful exit).
/// Intended for development, debugging, and headless operation.
pub struct CliChannel {
    id: String,
    event_bus: EventBus,
    cancel_token: Option<CancellationToken>,
    reader_handle: Arc<RwLock<Option<JoinHandle<()>>>>,

    /// Shared flag: `true` while the pipeline / LLM is generating a response.
    /// The read-loop shows a spinner and blocks new input until it flips back.
    is_busy: Arc<AtomicBool>,

    /// Set by `begin_stream` to tell the spinner thread that streaming I/O has
    /// taken over — the spinner should clear itself but NOT print a response.
    is_streaming: Arc<AtomicBool>,

    /// Slot for non-streaming responses.  `send_message` writes here; the
    /// read-loop thread drains it after the spinner stops so that all
    /// non-streaming terminal I/O stays on a single thread.
    pending_response: Arc<Mutex<Option<String>>>,

    /// Optional conversation backend for handling /session commands
    conversation: Option<Arc<dyn crate::providers::adapter::ConversationBackend>>,
}

impl CliChannel {
    pub fn new(id: impl Into<String>, event_bus: EventBus) -> Self {
        Self {
            id: id.into(),
            event_bus,
            cancel_token: None,
            reader_handle: Arc::new(RwLock::new(None)),
            is_busy: Arc::new(AtomicBool::new(false)),
            is_streaming: Arc::new(AtomicBool::new(false)),
            pending_response: Arc::new(Mutex::new(None)),
            conversation: None,
        }
    }

    fn history_path() -> Option<std::path::PathBuf> {
        dirs_next().map(|d| d.join(HISTORY_FILE))
    }
}

/// Resolve ~/.alia_cli_history portably.
fn dirs_next() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

/// Spin-wait with an animated spinner until `busy` becomes `false` or
/// `cancel` is triggered.
///
/// When `is_streaming` is `false` after the loop (non-streaming path), drains
/// the pending response slot and prints it.  When `is_streaming` is `true`,
/// `begin_stream` has already cleared the spinner line and taken over output,
/// so the spinner just exits quietly.
fn wait_for_response(
    busy: &AtomicBool,
    cancel: &CancellationToken,
    pending: &Mutex<Option<String>>,
    is_streaming: &AtomicBool,
) {
    let mut frame_idx: usize = 0;
    let mut stdout = io::stdout();

    while busy.load(Ordering::Acquire) && !cancel.is_cancelled() {
        let frame = SPINNER_FRAMES[frame_idx % SPINNER_FRAMES.len()];
        print!("\r\x1b[2K  \x1b[35m{frame}\x1b[0m \x1b[2mthinking…\x1b[0m");
        let _ = stdout.flush();
        frame_idx += 1;
        std::thread::sleep(std::time::Duration::from_millis(SPINNER_INTERVAL_MS));
    }

    if is_streaming.load(Ordering::Acquire) {
        // Streaming has taken over — begin_stream already cleared the line
        // and is printing tokens.  We just wait for streaming to finish
        // (is_streaming flips back to false in end_stream).
        while is_streaming.load(Ordering::Acquire) && !cancel.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(SPINNER_INTERVAL_MS));
        }
    } else {
        // Non-streaming path: clear spinner and print the full response.
        print!("\r\x1b[2K");
        let _ = stdout.flush();

        if let Some(msg) = pending.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let mut out = io::stdout().lock();
            let _ = writeln!(out, "\x1b[32malia:\x1b[0m {msg}");
            let _ = out.flush();
        }
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
        let event_bus = self.event_bus.clone();
        let channel_id = self.id.clone();
        let cancel = self
            .cancel_token
            .clone()
            .unwrap_or_else(CancellationToken::new);
        let is_busy = self.is_busy.clone();
        let is_streaming = self.is_streaming.clone();
        let pending_response = self.pending_response.clone();
        let conversation = self.conversation.clone();

        let handle = tokio::task::spawn_blocking(move || {
            let config = Config::builder()
                .edit_mode(EditMode::Emacs)
                .auto_add_history(true)
                .max_history_size(1000)
                .expect("valid history size")
                .build();

            let mut rl = match DefaultEditor::with_config(config) {
                Ok(rl) => rl,
                Err(e) => {
                    tracing::error!(error = %e, "failed to initialize CLI readline");
                    return;
                }
            };

            // Load history (ignore errors — file may not exist yet)
            if let Some(ref path) = CliChannel::history_path() {
                let _ = rl.load_history(path);
            }

            loop {
                if cancel.is_cancelled() {
                    break;
                }

                match rl.readline(PROMPT) {
                    Ok(line) => {
                        let text = line.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }

                        let mut parts = text.split_whitespace();
                        let cmd = parts.next().unwrap_or("");

                        match cmd {
                            "/quit" | "/exit" => {
                                tracing::info!("CLI channel: user requested exit");
                                cancel.cancel();
                                break;
                            }
                            "/help" => {
                                println!("\x1b[33mAlia CLI commands:\x1b[0m");
                                println!("  /help                 — show this help");
                                println!("  /quit                 — exit the CLI");
                                println!("  /clear                — clear screen");
                                println!("  /session new          — start a new chat session");
                                println!("  /session list         — list recent sessions");
                                println!("  /session switch <id>  — switch to session ID");
                                println!();
                                println!("Type anything else to chat with Alia.");
                                continue;
                            }
                            "/clear" => {
                                print!("\x1b[2J\x1b[H");
                                let _ = io::stdout().flush();
                                continue;
                            }
                            "/session" => {
                                let subcmd = parts.next().unwrap_or("");
                                if let Some(ref conv) = conversation {
                                    let rt = tokio::runtime::Handle::current();
                                    let ch_id = channel_id.clone();
                                    let u_id = "cli-user".to_string();

                                    match subcmd {
                                        "new" => {
                                            match rt.block_on(async {
                                                // UUID generation is done in backend. Just create one by inserting a new record.
                                                // Actually the trait doesn't have an explicit `create_session`, only `get_or_create_active`.
                                                // We can spoof it by generating a random UUID and forcing a switch, but the trait expects `set_active_session` to validate it.
                                                // So we need to add a fake message or just use the DB directly?
                                                // Wait, we can just let `get_or_create_active_session` run with a fresh user, but that breaks identity.
                                                // Let's add a `create_session` method to the trait. Or...
                                                // For now, let's just tell the user to use an API if we can't do it via trait easily.
                                                // Wait, I can update the trait and the Postgres backend.
                                                // It's easier to just call a newly added method: `create_session(channel_id, user_id)`
                                                conv.create_session(&ch_id, &u_id).await
                                            }) {
                                                Ok(id) => {
                                                    let _ = rt.block_on(
                                                        conv.set_active_session(&ch_id, &u_id, id),
                                                    );
                                                    println!(
                                                        "\x1b[32mStarted new session: {}\x1b[0m",
                                                        id
                                                    );
                                                }
                                                Err(e) => println!("\x1b[31mError: {}\x1b[0m", e),
                                            }
                                        }
                                        "list" => {
                                            match rt.block_on(conv.list_sessions(&ch_id, &u_id)) {
                                                Ok(sessions) => {
                                                    println!("\x1b[33mRecent Sessions:\x1b[0m");
                                                    let active_id = rt
                                                        .block_on(
                                                            conv.get_or_create_active_session(
                                                                &ch_id, &u_id,
                                                            ),
                                                        )
                                                        .unwrap_or_default();
                                                    for (id, name, updated_at) in
                                                        sessions.iter().take(10)
                                                    {
                                                        let name_str =
                                                            name.as_deref().unwrap_or("Unnamed");
                                                        let marker = if *id == active_id {
                                                            "*"
                                                        } else {
                                                            " "
                                                        };
                                                        println!(
                                                            "{} {} ({}) - {}",
                                                            marker,
                                                            id,
                                                            name_str,
                                                            updated_at.format("%Y-%m-%d %H:%M:%S")
                                                        );
                                                    }
                                                }
                                                Err(e) => println!("\x1b[31mError: {}\x1b[0m", e),
                                            }
                                        }
                                        "switch" => {
                                            if let Some(id_str) = parts.next() {
                                                if let Ok(id) = uuid::Uuid::parse_str(id_str) {
                                                    match rt.block_on(
                                                        conv.set_active_session(&ch_id, &u_id, id),
                                                    ) {
                                                        Ok(_) => println!(
                                                            "\x1b[32mSwitched to session {}\x1b[0m",
                                                            id
                                                        ),
                                                        Err(e) => println!(
                                                            "\x1b[31mFailed to switch: {}\x1b[0m",
                                                            e
                                                        ),
                                                    }
                                                } else {
                                                    println!(
                                                        "\x1b[31mInvalid session ID format.\x1b[0m"
                                                    );
                                                }
                                            } else {
                                                println!(
                                                    "\x1b[31mUsage: /session switch <uuid>\x1b[0m"
                                                );
                                            }
                                        }
                                        _ => println!(
                                            "\x1b[31mUnknown session command. Try: new, list, switch\x1b[0m"
                                        ),
                                    }
                                } else {
                                    println!("\x1b[31mConversation backend not available.\x1b[0m");
                                }
                                continue;
                            }
                            _ => {}
                        }

                        // Mark as busy BEFORE publishing so the spinner starts
                        // immediately after the event is sent.
                        is_busy.store(true, Ordering::Release);

                        let event = ChannelEvent::TextMessage {
                            channel_id: channel_id.clone(),
                            user_id: "cli-user".into(),
                            text,
                        };
                        event_bus.publish(crate::event::ServerEvent::ChannelInput(event));

                        // Block with a spinner until the response arrives.
                        wait_for_response(&is_busy, &cancel, &pending_response, &is_streaming);
                    }
                    Err(ReadlineError::Interrupted) => {
                        // Ctrl+C — cancel current line, continue loop
                        continue;
                    }
                    Err(ReadlineError::Eof) => {
                        // Ctrl+D — graceful exit
                        tracing::info!("CLI channel: EOF (Ctrl+D), shutting down");
                        cancel.cancel();
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "CLI readline error");
                        break;
                    }
                }
            }

            // Persist history
            if let Some(ref path) = CliChannel::history_path() {
                if let Err(e) = rl.save_history(path) {
                    tracing::warn!(error = %e, "failed to save CLI history");
                }
            }
        });

        *self.reader_handle.write().await = Some(handle);
        tracing::info!(channel_id = %self.id, "CLI channel started (interactive REPL)");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        if let Some(handle) = self.reader_handle.write().await.take() {
            handle.abort();
        }
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
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

    async fn send_message(&self, _target: &ChannelTarget, msg: &str) -> anyhow::Result<()> {
        // Deposit the response text for the read-loop thread to print.
        {
            let mut slot = self
                .pending_response
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *slot = Some(msg.to_owned());
        }
        // Clear the busy flag so the spinner stops and the read-loop picks
        // up the pending response.
        self.is_busy.store(false, Ordering::Release);
        Ok(())
    }

    async fn begin_stream(&self, _target: &ChannelTarget) -> anyhow::Result<()> {
        // Signal that streaming is active — the spinner thread will stop
        // rendering and will NOT print a response (we handle output here).
        self.is_streaming.store(true, Ordering::Release);

        // Stop the spinner.
        self.is_busy.store(false, Ordering::Release);

        // Give the spinner thread one cycle to notice and stop writing.
        tokio::time::sleep(std::time::Duration::from_millis(SPINNER_INTERVAL_MS + 20)).await;

        // Clear the spinner line and print the response prefix.
        let mut stdout = io::stdout().lock();
        write!(stdout, "\r\x1b[2K\x1b[32malia:\x1b[0m ")?;
        stdout.flush()?;
        Ok(())
    }

    async fn send_stream_chunk(&self, _target: &ChannelTarget, chunk: &str) -> anyhow::Result<()> {
        let mut stdout = io::stdout().lock();
        write!(stdout, "{chunk}")?;
        stdout.flush()?;
        Ok(())
    }

    async fn end_stream(&self, _target: &ChannelTarget, _full_text: &str) -> anyhow::Result<()> {
        // Print a trailing newline after the streamed tokens.
        let mut stdout = io::stdout().lock();
        writeln!(stdout)?;
        stdout.flush()?;

        // Signal the read-loop thread that streaming is done — it can resume
        // readline.
        self.is_streaming.store(false, Ordering::Release);
        Ok(())
    }

    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
        Err(anyhow::anyhow!(
            "action '{}' not supported by CLI channel",
            action.name
        ))
    }
}
