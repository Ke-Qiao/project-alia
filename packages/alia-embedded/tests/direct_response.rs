use std::path::PathBuf;

use alia_embedded::config::{DirectResponseAudioConfig, DirectResponseConfig};
use alia_embedded::module::direct_response::KeywordMatcher;
use alia_embedded::module::traits::DirectResponseEngine;

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
