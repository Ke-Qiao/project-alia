use std::io::{self, Write};
use std::sync::Arc;

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
}

impl CliChannel {
    pub fn new(id: impl Into<String>, event_bus: EventBus) -> Self {
        Self {
            id: id.into(),
            event_bus,
            cancel_token: None,
            reader_handle: Arc::new(RwLock::new(None)),
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

#[async_trait]
impl ModuleLifecycle for CliChannel {
    fn name(&self) -> &'static str {
        "cli-channel"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        self.cancel_token = Some(ctx.cancel_token.clone());
        Ok(())
    }

    async fn start(&self) -> anyhow::Result<()> {
        let event_bus = self.event_bus.clone();
        let channel_id = self.id.clone();
        let cancel = self
            .cancel_token
            .clone()
            .unwrap_or_else(CancellationToken::new);

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

                        // Built-in commands
                        match text.as_str() {
                            "/quit" | "/exit" => {
                                tracing::info!("CLI channel: user requested exit");
                                cancel.cancel();
                                break;
                            }
                            "/help" => {
                                println!("\x1b[33mAlia CLI commands:\x1b[0m");
                                println!("  /help   — show this help");
                                println!("  /quit   — exit the CLI");
                                println!("  /clear  — clear screen");
                                println!();
                                println!("Type anything else to chat with Alia.");
                                continue;
                            }
                            "/clear" => {
                                print!("\x1b[2J\x1b[H");
                                let _ = io::stdout().flush();
                                continue;
                            }
                            _ => {}
                        }

                        let event = ChannelEvent::TextMessage {
                            channel_id: channel_id.clone(),
                            user_id: "cli-user".into(),
                            text,
                        };
                        event_bus.publish(crate::event::ServerEvent::ChannelInput(event));
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
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "\x1b[32malia:\x1b[0m {msg}")?;
        stdout.flush()?;
        Ok(())
    }

    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
        Err(anyhow::anyhow!(
            "action '{}' not supported by CLI channel",
            action.name
        ))
    }
}
