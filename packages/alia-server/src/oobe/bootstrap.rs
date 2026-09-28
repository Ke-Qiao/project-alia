//! OOBE bootstrap logic: detection, CLI key handler, config generation.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::config::{ServerConfig, merge_toml_values};

/// Result of the OOBE setup flow.
#[derive(Clone)]
pub enum SetupResult {
    /// Config was written; return the path to load.
    ConfigReady(PathBuf),
    /// User cancelled (Ctrl-C).
    Cancelled,
}

/// Check if any config file exists in the standard search locations.
pub fn needs_setup() -> bool {
    // If explicitly set, check if the file actually exists
    if let Ok(p) = std::env::var("ALIA_CONFIG_PATH") {
        return !Path::new(&p).exists();
    }

    let candidates = [
        "config/server.toml".to_string(),
        dirs::config_dir()
            .map(|d| {
                d.join("alia")
                    .join("server.toml")
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default(),
        "/etc/alia/server.toml".to_string(),
    ];

    !candidates
        .iter()
        .any(|c| !c.is_empty() && Path::new(c).exists())
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
        "policies/transcript_correction.toml",
        include_str!("../../../../config/server.example.d/policies/transcript_correction.toml"),
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

/// Keep the CLI's editable base + fragments layout. This is a first-install
/// operation: existing configuration must never be overwritten with defaults.
pub fn release_example_config(target: &Path) -> anyhow::Result<PathBuf> {
    let fragment_dir = target.with_extension("d");
    anyhow::ensure!(
        std::fs::symlink_metadata(target).is_err(),
        "configuration already exists: {}",
        target.display()
    );
    ServerConfig::load_candidate(target, &example_config()?)?.validate_llm_roles()?;
    std::fs::create_dir_all(config_parent(target))?;
    // Exclusive creation also protects an existing fragment tree. Only this
    // newly-owned tree is removed if installation cannot publish its base.
    std::fs::create_dir(&fragment_dir).with_context(|| {
        format!(
            "create new configuration fragments {}",
            fragment_dir.display()
        )
    })?;
    let result = (|| {
        for (name, content) in EXAMPLE_CONFIG_FRAGMENTS {
            let path = fragment_dir.join(name);
            std::fs::create_dir_all(config_parent(&path))?;
            std::fs::write(path, content)?;
        }
        write_validated_config(target, EXAMPLE_CONFIG, false)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&fragment_dir);
    }
    result
}

pub(crate) fn save_config(target: &Path, content: &str) -> anyhow::Result<PathBuf> {
    write_validated_config(target, content, true)
}

fn config_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn write_validated_config(target: &Path, content: &str, replace: bool) -> anyhow::Result<PathBuf> {
    // Resolve an existing symlink exactly as the runtime loader does, so the
    // validated fragment directory is also the one loaded after publication.
    let destination = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    ServerConfig::load_candidate(&destination, content)?.validate_llm_roles()?;
    let parent = config_parent(&destination);
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".alia-setup-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| -> anyhow::Result<()> {
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if replace {
            std::fs::rename(&temporary, &destination)?;
        } else {
            // A second CLI/HTTP completion cannot be overwritten between the
            // first-install check and this final publication.
            std::fs::hard_link(&temporary, &destination)?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&temporary);
    result.with_context(|| format!("save configuration {}", target.display()))?;
    Ok(target.to_path_buf())
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
