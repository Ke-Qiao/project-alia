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
fn ignores_action_only_rule_without_audio() {
    let matcher = KeywordMatcher::new(&[DirectResponseConfig {
        slug: "cancel".to_string(),
        keywords: vec!["退下".to_string()],
        audio: PathBuf::new(),
        audio_variants: Vec::new(),
        action: Some("cancel".to_string()),
    }]);

    assert!(matcher.match_keyword("退下").is_none());
    assert!(matcher.match_action("cancel").is_none());
}

#[test]
fn empty_configuration_does_not_invent_cancel_controls() {
    let matcher = KeywordMatcher::new(&[]);

    for text in ["再见", "别说了", "结束对话", "goodbye", "stop talking"] {
        assert!(matcher.match_keyword(text).is_none());
    }
}

#[test]
fn configured_cue_alias_resolves_only_its_audio_metadata() {
    let matcher = KeywordMatcher::new(&[DirectResponseConfig {
        slug: "custom-wake".to_string(),
        keywords: vec!["你好阿利亚".to_string()],
        audio: PathBuf::from("hello.wav"),
        audio_variants: Vec::new(),
        action: Some("wakeword".to_string()),
    }]);

    let action = matcher.match_action("wakeword").unwrap();
    assert_eq!(action.audio_path.as_deref(), Some("hello.wav"));
    assert_eq!(action.action.as_deref(), Some("wakeword"));
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
        slug: "wake-cue".to_string(),
        keywords: Vec::new(),
        audio: PathBuf::from("wake.wav"),
        audio_variants: Vec::new(),
        action: None,
    }]);

    let action = matcher.match_action("wake_cue").unwrap();
    assert_eq!(action.audio_path.as_deref(), Some("wake.wav"));
    assert_eq!(action.action.as_deref(), Some("wake-cue"));
}

#[test]
fn ignores_audio_variants_without_valid_path_or_weight() {
    for (audio, weight) in [
        (PathBuf::new(), 1.0),
        (PathBuf::from(" "), 1.0),
        (PathBuf::from("wake.wav"), 0.0),
        (PathBuf::from("wake.wav"), -1.0),
        (PathBuf::from("wake.wav"), f32::NAN),
        (PathBuf::from("wake.wav"), f32::INFINITY),
    ] {
        let matcher = KeywordMatcher::new(&[DirectResponseConfig {
            slug: "wakeword".into(),
            keywords: vec!["hey alia".into()],
            audio: PathBuf::new(),
            audio_variants: vec![DirectResponseAudioConfig { audio, weight }],
            action: Some("wakeword".into()),
        }]);
        assert!(matcher.match_keyword("hey alia").is_none());
        assert!(matcher.match_action("wakeword").is_none());
    }
}
