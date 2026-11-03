//! OOBE bootstrap logic: detection, CLI key handler, config generation.

use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::config::publication::{self, PendingPublication};
use crate::config::{ServerConfig, merge_toml_values};

/// HTTP, CLI and cancellation share one publication and completion decision.
pub struct SetupCompletion {
    state: tokio::sync::watch::Sender<SetupState>,
}

#[derive(Clone)]
pub(crate) enum SetupState {
    Idle,
    Pending(PendingPublication),
    Ready(PathBuf),
    Cancelled,
}

impl SetupState {
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(self, Self::Ready(_) | Self::Cancelled)
    }
}

impl SetupCompletion {
    pub(crate) fn new() -> Self {
        let (state, _) = tokio::sync::watch::channel(SetupState::Idle);
        Self { state }
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<SetupState> {
        self.state.subscribe()
    }

    pub(crate) fn is_pending(&self) -> bool {
        matches!(&*self.state.borrow(), SetupState::Pending(_))
    }

    pub(crate) fn is_terminal(&self) -> bool {
        self.state.borrow().is_terminal()
    }

    pub(crate) fn cancel(&self) {
        self.state.send_if_modified(|state| {
            if state.is_terminal() {
                return false;
            }
            // A pending publication remains on disk for restart recovery.
            *state = SetupState::Cancelled;
            true
        });
    }

    pub(crate) fn save(
        &self,
        target: &Path,
        content: &str,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<PathBuf> {
        self.publish(target, content, true, None, cancel)
    }

    pub(crate) fn generate(
        &self,
        target: &Path,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<PathBuf> {
        self.publish(
            target,
            EXAMPLE_CONFIG,
            false,
            Some(EXAMPLE_CONFIG_FRAGMENTS),
            cancel,
        )
    }

    fn publish(
        &self,
        target: &Path,
        content: &str,
        replace: bool,
        fragments: Option<&[(&str, &str)]>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<PathBuf> {
        let mut result = Err(anyhow::anyhow!("setup has already completed"));
        // Publication and its recovery stay inside the same completion gate.
        self.state.send_if_modified(|state| {
            if state.is_terminal() {
                return false;
            }
            if cancel.is_cancelled() {
                result = Err(anyhow::anyhow!("setup is shutting down"));
                return false;
            }
            result = match state {
                SetupState::Idle => publication::save(target, content, replace, fragments),
                SetupState::Pending(pending) => {
                    if !pending.matches_request(target, content, replace, fragments) {
                        result = Err(anyhow::anyhow!(
                            "configuration publication durability is unknown; retry the same path, content and installation method"
                        ));
                        return false;
                    }
                    pending.retry().context(
                        "configuration publication durability remains unknown; recovery failed",
                    )
                }
                SetupState::Ready(_) | SetupState::Cancelled => unreachable!(),
            };
            match &result {
                Ok(path) => {
                    *state = SetupState::Ready(path.clone());
                    true
                }
                Err(error) => match error.downcast_ref::<PendingPublication>() {
                    Some(pending) => {
                        *state = SetupState::Pending(pending.clone());
                        true
                    }
                    None => false,
                },
            }
        });
        result
    }
}

/// A pending publication must be recovered before offering another setup.
pub fn needs_setup() -> bool {
    let path = ServerConfig::discover_config_path();
    let path = Path::new(&path);
    !path.exists() && !publication::has_pending(path)
}

/// Base example config (same content as config/server.example.toml).
/// We embed it at compile time so the binary is self-contained.
const EXAMPLE_CONFIG: &str = include_str!("../../../../config/server.example.toml");
const EXAMPLE_CONFIG_FRAGMENTS: &[(&str, &str)] = &[
    (
        "providers/easy_turn.toml",
        include_str!("../../../../config/server.example.d/providers/easy_turn.toml"),
    ),
    (
        "providers/typesafe.toml",
        include_str!("../../../../config/server.example.d/providers/typesafe.toml"),
    ),
    (
        "providers/openai.toml",
        include_str!("../../../../config/server.example.d/providers/openai.toml"),
    ),
    (
        "providers/anthropic.toml",
        include_str!("../../../../config/server.example.d/providers/anthropic.toml"),
    ),
    (
        "providers/gemini.toml",
        include_str!("../../../../config/server.example.d/providers/gemini.toml"),
    ),
    (
        "providers/whisper_cpp.toml",
        include_str!("../../../../config/server.example.d/providers/whisper_cpp.toml"),
    ),
    (
        "providers/gpt_sovits.toml",
        include_str!("../../../../config/server.example.d/providers/gpt_sovits.toml"),
    ),
    (
        "policies/routing.toml",
        include_str!("../../../../config/server.example.d/policies/routing.toml"),
    ),
    (
        "policies/conversation.toml",
        include_str!("../../../../config/server.example.d/policies/conversation.toml"),
    ),
    (
        "policies/memory.toml",
        include_str!("../../../../config/server.example.d/policies/memory.toml"),
    ),
    (
        "policies/cache.toml",
        include_str!("../../../../config/server.example.d/policies/cache.toml"),
    ),
    (
        "policies/affect.toml",
        include_str!("../../../../config/server.example.d/policies/affect.toml"),
    ),
    (
        "policies/regulation.toml",
        include_str!("../../../../config/server.example.d/policies/regulation.toml"),
    ),
    (
        "policies/expression.toml",
        include_str!("../../../../config/server.example.d/policies/expression.toml"),
    ),
    (
        "policies/insights.toml",
        include_str!("../../../../config/server.example.d/policies/insights.toml"),
    ),
    (
        "policies/rules.toml",
        include_str!("../../../../config/server.example.d/policies/rules.toml"),
    ),
    (
        "policies/scheduler.toml",
        include_str!("../../../../config/server.example.d/policies/scheduler.toml"),
    ),
    (
        "policies/tools.toml",
        include_str!("../../../../config/server.example.d/policies/tools.toml"),
    ),
    (
        "profiles/llm.toml",
        include_str!("../../../../config/server.example.d/profiles/llm.toml"),
    ),
    (
        "profiles/voice.default.toml",
        include_str!("../../../../config/server.example.d/profiles/voice.default.toml"),
    ),
    (
        "profiles/expression_styles.toml",
        include_str!("../../../../config/server.example.d/profiles/expression_styles.toml"),
    ),
    (
        "resources/skills.toml",
        include_str!("../../../../config/server.example.d/resources/skills.toml"),
    ),
    (
        "resources/cache_policy.toml",
        include_str!("../../../../config/server.example.d/resources/cache_policy.toml"),
    ),
    (
        "resources/appraisal_lexicons/en.toml",
        include_str!("../../../../config/server.example.d/resources/appraisal_lexicons/en.toml"),
    ),
    (
        "resources/appraisal_lexicons/zh.toml",
        include_str!("../../../../config/server.example.d/resources/appraisal_lexicons/zh.toml"),
    ),
];

/// Determine the best path to write the config file.
/// Respects ALIA_CONFIG_PATH if set, then tries ./config/, then XDG.
pub(crate) fn default_config_target() -> PathBuf {
    if let Ok(p) = std::env::var("ALIA_CONFIG_PATH") {
        return PathBuf::from(p);
    }

    // If we're in a directory that has `config/` (dev layout), use it.
    let local = PathBuf::from("config/server.toml");
    if local.parent().map_or(false, |p| p.exists()) {
        return local;
    }

    // Otherwise use XDG config dir
    if let Some(dir) = dirs::config_dir() {
        return dir.join("alia").join("server.toml");
    }

    local
}

/// HTTP setup edits one complete document, assembled from the same embedded
/// files used by CLI setup. Fragment precedence matches ServerConfig::load.
pub(crate) fn example_config() -> anyhow::Result<String> {
    let mut config = toml::from_str(EXAMPLE_CONFIG)?;
    let mut fragments: Vec<_> = EXAMPLE_CONFIG_FRAGMENTS.iter().collect();
    fragments.sort_by_key(|(name, _)| *name);
    for (_, content) in fragments {
        merge_toml_values(&mut config, toml::from_str(content)?);
    }
    toml::to_string_pretty(&config).context("serialize complete setup template")
}

/// Print the OOBE banner to the terminal.
pub fn print_banner(port: u16) {
    eprintln!();
    eprintln!("════════════════════════════════════════════════════════════");
    eprintln!("               ✦  Alia — First Run Setup  ✦");
    eprintln!("════════════════════════════════════════════════════════════");
    eprintln!();
    eprintln!("  No configuration file found.");
    eprintln!();
    eprintln!("  → Local setup HTTP API: http://127.0.0.1:{port}/api/v1/setup");
    eprintln!("    Use curl locally, or forward this loopback port through SSH.");
    eprintln!();
    eprintln!("  → Or press [c] + Enter to generate a config file");
    eprintln!("    directly and edit it manually.");
    eprintln!();
    eprintln!("  → Press [q] + Enter or Ctrl-C to quit.");
    eprintln!();
    eprintln!("════════════════════════════════════════════════════════════");
    eprintln!();
}
