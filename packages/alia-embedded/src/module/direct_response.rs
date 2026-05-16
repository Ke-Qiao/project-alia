use std::path::PathBuf;
use std::sync::RwLock;

use anyhow::{Context, Result};
use rand::random;

use crate::config::DirectResponseConfig;

use super::lifecycle::ModuleLifecycle;
use super::traits::{DirectResponseAction, DirectResponseEngine};

#[derive(Debug, Clone)]
struct KeywordRule {
    slug: String,
    keywords: Vec<String>,
    audio_variants: Vec<WeightedAudio>,
    action: Option<String>,
}

#[derive(Debug, Clone)]
struct WeightedAudio {
    audio: PathBuf,
    weight: f32,
}

pub struct KeywordMatcher {
    config_rules: Vec<KeywordRule>,
    rules: RwLock<Vec<KeywordRule>>,
}

impl KeywordMatcher {
    pub fn new(configs: &[DirectResponseConfig]) -> Self {
        let rules = build_rules(configs);

        Self {
            config_rules: rules.clone(),
            rules: RwLock::new(rules),
        }
    }

    fn find_match(&self, text: &str) -> Option<DirectResponseAction> {
        let normalized = normalize(text);
        if normalized.is_empty() {
            return None;
        }

        let rules = self.rules.read().ok()?;
        let rule = rules.iter().find(|rule| {
            rule.keywords
                .iter()
                .any(|keyword| normalized.contains(keyword))
        })?;

        Some(DirectResponseAction {
            audio_path: choose_audio(&rule.audio_variants).map(|audio| audio.display().to_string()),
            action: rule.action.clone(),
        })
    }

    fn find_action(&self, action: &str) -> Option<DirectResponseAction> {
        let requested = normalize_action_name(action);
        if requested.is_empty() {
            return None;
        }

        let rules = self.rules.read().ok()?;
        let rule = rules.iter().find(|rule| {
            rule.action
                .as_deref()
                .map(normalize_action_name)
                .is_some_and(|candidate| candidate == requested)
                || normalize_action_name(&rule.slug) == requested
        })?;

        Some(DirectResponseAction {
            audio_path: choose_audio(&rule.audio_variants).map(|audio| audio.display().to_string()),
            action: rule
                .action
                .clone()
                .or_else(|| (!rule.slug.trim().is_empty()).then_some(rule.slug.clone())),
        })
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for KeywordMatcher {
    fn name(&self) -> &'static str {
        "keyword_direct_response"
    }

    async fn init(&mut self, ctx: &super::context::ModuleContext) -> Result<()> {
        if let Err(error) = ctx
            .state_store
            .import_direct_response_bundles(&ctx.config.direct_response_defaults.audio_dir)
            .await
        {
            tracing::warn!(
                error = ?error,
                root = %ctx.config.direct_response_defaults.audio_dir.display(),
                "direct response bundle import failed; continuing with existing SQLite rules"
            );
        }

        let stored_rules = ctx
            .state_store
            .load_direct_responses()
            .await
            .context("loading direct response rules from SQLite")?;

        if !stored_rules.is_empty() {
            let mut rules = self
                .rules
                .write()
                .map_err(|_| anyhow::anyhow!("direct response rules lock poisoned"))?;
            *rules = build_rules(&stored_rules);
        } else if !self.config_rules.is_empty() {
            let mut rules = self
                .rules
                .write()
                .map_err(|_| anyhow::anyhow!("direct response rules lock poisoned"))?;
            *rules = self.config_rules.clone();
        }

        Ok(())
    }
}

impl DirectResponseEngine for KeywordMatcher {
    fn match_keyword(&self, text: &str) -> Option<DirectResponseAction> {
        self.find_match(text)
    }

    fn match_action(&self, action: &str) -> Option<DirectResponseAction> {
        self.find_action(action)
    }
}

fn build_rules(configs: &[DirectResponseConfig]) -> Vec<KeywordRule> {
    configs
        .iter()
        .map(|config| KeywordRule {
            slug: config.slug.clone(),
            keywords: config
                .keywords
                .iter()
                .map(|keyword| normalize(keyword))
                .filter(|keyword| !keyword.is_empty())
                .collect(),
            audio_variants: build_audio_variants(config),
            action: config.action.clone(),
        })
        .filter(|rule| {
            let has_action = rule
                .action
                .as_deref()
                .is_some_and(|action| !action.trim().is_empty());
            let has_audio = !rule.audio_variants.is_empty();
            let has_keywords = !rule.keywords.is_empty();
            let has_slug = !rule.slug.trim().is_empty();

            (has_keywords && (has_action || has_audio)) || has_action || (has_slug && has_audio)
        })
        .collect()
}

fn build_audio_variants(config: &DirectResponseConfig) -> Vec<WeightedAudio> {
    let mut variants = config
        .audio_variants
        .iter()
        .map(|variant| WeightedAudio {
            audio: variant.audio.clone(),
            weight: variant.weight.max(0.0),
        })
        .filter(|variant| variant.weight > 0.0)
        .collect::<Vec<_>>();

    if variants.is_empty() && !config.audio.as_os_str().is_empty() {
        variants.push(WeightedAudio {
            audio: config.audio.clone(),
            weight: 1.0,
        });
    }

    variants
}

fn choose_audio(variants: &[WeightedAudio]) -> Option<&PathBuf> {
    if variants.is_empty() {
        return None;
    }

    let total = variants
        .iter()
        .map(|variant| variant.weight.max(0.0))
        .sum::<f32>();
    if total <= f32::EPSILON {
        return variants.first().map(|variant| &variant.audio);
    }

    let mut threshold = random::<f32>() * total;
    for variant in variants {
        threshold -= variant.weight.max(0.0);
        if threshold <= 0.0 {
            return Some(&variant.audio);
        }
    }

    variants.last().map(|variant| &variant.audio)
}

fn normalize(input: &str) -> String {
    input
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|ch| !ch.is_whitespace() && !ch.is_ascii_punctuation())
        .collect()
}

fn normalize_action_name(input: &str) -> String {
    input
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|ch| if ch == '-' { '_' } else { ch })
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DirectResponseAudioConfig;

    #[test]
    fn matches_normalized_keyword() {
        let matcher = KeywordMatcher::new(&[DirectResponseConfig {
            slug: String::new(),
            keywords: vec!["现在几点".to_string(), "what time".to_string()],
            audio: PathBuf::from("assets/responses/time.wav"),
            audio_variants: Vec::new(),
            action: Some("report_time".to_string()),
        }]);

        let action = matcher.match_keyword("Hey, what time is it?").unwrap();
        assert_eq!(
            action.audio_path.as_deref(),
            Some("assets/responses/time.wav")
        );
        assert_eq!(action.action.as_deref(), Some("report_time"));
    }

    #[test]
    fn supports_weighted_audio_variants() {
        let matcher = KeywordMatcher::new(&[DirectResponseConfig {
            slug: String::new(),
            keywords: vec!["hello".to_string()],
            audio: PathBuf::from("fallback.wav"),
            audio_variants: vec![DirectResponseAudioConfig {
                audio: PathBuf::from("variant.wav"),
                weight: 1.0,
            }],
            action: None,
        }]);

        let action = matcher.match_keyword("hello").unwrap();
        assert_eq!(action.audio_path.as_deref(), Some("variant.wav"));
    }

    #[test]
    fn supports_action_only_response() {
        let matcher = KeywordMatcher::new(&[DirectResponseConfig {
            slug: "cancel".to_string(),
            keywords: vec!["退下".to_string()],
            audio: PathBuf::new(),
            audio_variants: Vec::new(),
            action: Some("cancel".to_string()),
        }]);

        let action = matcher.match_keyword("退下").unwrap();
        assert_eq!(action.audio_path, None);
        assert_eq!(action.action.as_deref(), Some("cancel"));
        let action = matcher.match_action("cancel").unwrap();
        assert_eq!(action.audio_path, None);
        assert_eq!(action.action.as_deref(), Some("cancel"));
    }

    #[test]
    fn ignores_keyword_only_rule_without_audio_or_action() {
        let matcher = KeywordMatcher::new(&[DirectResponseConfig {
            slug: "empty".to_string(),
            keywords: vec!["empty".to_string()],
            audio: PathBuf::new(),
            audio_variants: Vec::new(),
            action: None,
        }]);

        assert!(matcher.match_keyword("empty").is_none());
    }

    #[test]
    fn resolves_named_response_by_slug() {
        let matcher = KeywordMatcher::new(&[DirectResponseConfig {
            slug: "listening-continue".to_string(),
            keywords: Vec::new(),
            audio: PathBuf::from("continue.wav"),
            audio_variants: Vec::new(),
            action: None,
        }]);

        let action = matcher.match_action("listening_continue").unwrap();
        assert_eq!(action.audio_path.as_deref(), Some("continue.wav"));
        assert_eq!(action.action.as_deref(), Some("listening-continue"));
    }
}
