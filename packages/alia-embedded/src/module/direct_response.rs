use std::path::PathBuf;
use std::sync::RwLock;

use anyhow::{Context, Result};
use rand::random;

use crate::config::DirectResponseConfig;

use super::lifecycle::ModuleLifecycle;
use super::traits::{DirectResponseAction, DirectResponseEngine};

#[derive(Debug, Clone)]
struct KeywordRule {
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

        let audio = choose_audio(&rule.audio_variants)?;

        Some(DirectResponseAction {
            audio_path: audio.display().to_string(),
            action: rule.action.clone(),
        })
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for KeywordMatcher {
    fn name(&self) -> &'static str {
        "keyword_direct_response"
    }

    async fn init(&mut self, ctx: &super::context::ModuleContext) -> Result<()> {
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
}

fn build_rules(configs: &[DirectResponseConfig]) -> Vec<KeywordRule> {
    configs
        .iter()
        .map(|config| KeywordRule {
            keywords: config
                .keywords
                .iter()
                .map(|keyword| normalize(keyword))
                .filter(|keyword| !keyword.is_empty())
                .collect(),
            audio_variants: build_audio_variants(config),
            action: config.action.clone(),
        })
        .filter(|rule| !rule.keywords.is_empty() && !rule.audio_variants.is_empty())
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

    if variants.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DirectResponseAudioConfig;

    #[test]
    fn matches_normalized_keyword() {
        let matcher = KeywordMatcher::new(&[DirectResponseConfig {
            keywords: vec!["现在几点".to_string(), "what time".to_string()],
            audio: PathBuf::from("assets/responses/time.wav"),
            audio_variants: Vec::new(),
            action: Some("report_time".to_string()),
        }]);

        let action = matcher.match_keyword("Hey, what time is it?").unwrap();
        assert_eq!(action.audio_path, "assets/responses/time.wav");
        assert_eq!(action.action.as_deref(), Some("report_time"));
    }

    #[test]
    fn supports_weighted_audio_variants() {
        let matcher = KeywordMatcher::new(&[DirectResponseConfig {
            keywords: vec!["hello".to_string()],
            audio: PathBuf::from("fallback.wav"),
            audio_variants: vec![DirectResponseAudioConfig {
                audio: PathBuf::from("variant.wav"),
                weight: 1.0,
            }],
            action: None,
        }]);

        let action = matcher.match_keyword("hello").unwrap();
        assert_eq!(action.audio_path, "variant.wav");
    }
}
