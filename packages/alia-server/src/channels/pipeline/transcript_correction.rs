use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time;

use crate::config::TranscriptCorrectionPolicySection;
use crate::providers::adapter::{
    ChatMessage, LlmAdapter, SttCapabilities, TranscriptionAlternative, TranscriptionResult,
    TranscriptionSegment, TranscriptionWord,
};

const TRANSCRIPT_CORRECTION_SYSTEM_PROMPT: &str = r#"You correct speech-to-text transcripts before they are routed to an assistant.

Return exactly one JSON object with this schema:
{
  "corrected_text": "string",
  "confidence": 0.0,
  "changed": false,
  "corrections": [{"from": "string", "to": "string", "reason": "string"}],
  "ambiguities": ["string"],
  "requires_confirmation": false,
  "clarification_text": null,
  "risk": "low"
}

Rules:
- Correct only likely speech recognition errors, punctuation, segmentation, homophones, and obvious word substitutions.
- Preserve the user's intent, language, tone, and level of detail.
- Do not answer the user.
- Do not add new requests, facts, names, devices, numbers, dates, or commands.
- Be conservative with names, device names, room names, code, URLs, amounts, dates, and safety-sensitive commands.
- If a correction could change a command target or safety meaning, keep the original text, set requires_confirmation to true, and describe the ambiguity.
- When requires_confirmation is true, include a short clarification_text that asks what the user meant without performing the action.
- Use risk "low", "medium", or "high". Use "high" for destructive, financial, safety-critical, or irreversible commands.
- If the transcript is already good or uncertain, return the original text and changed false."#;

#[derive(Debug, Clone, Default)]
pub struct TranscriptCorrectionContext {
    pub channel_id: String,
    pub user_id: String,
    pub known_device_names: Vec<String>,
    pub room_names: Vec<String>,
    pub active_skill_names: Vec<String>,
    pub last_assistant_question: Option<String>,
}

impl TranscriptCorrectionContext {
    pub fn new(channel_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self {
            channel_id: channel_id.into(),
            user_id: user_id.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptCorrectionSttTrace {
    pub provider: String,
    pub capabilities: SttCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<TranscriptionSegment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<TranscriptionAlternative>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptCorrectionResult {
    pub raw_text: String,
    pub corrected_text: String,
    pub confidence: f32,
    pub changed: bool,
    #[serde(default)]
    pub corrections: Vec<TranscriptCorrectionEdit>,
    #[serde(default)]
    pub ambiguities: Vec<String>,
    pub requires_confirmation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clarification_text: Option<String>,
    pub risk: TranscriptCorrectionRisk,
    pub status: TranscriptCorrectionStatus,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt: Option<TranscriptCorrectionSttTrace>,
}

impl TranscriptCorrectionResult {
    pub fn text_for_processing(&self, policy: &TranscriptCorrectionPolicySection) -> &str {
        if self.should_replace(policy) {
            &self.corrected_text
        } else {
            &self.raw_text
        }
    }

    pub fn should_replace(&self, policy: &TranscriptCorrectionPolicySection) -> bool {
        self.status == TranscriptCorrectionStatus::Corrected
            && self.changed
            && !self.corrected_text.trim().is_empty()
            && self.confidence >= policy.min_confidence_to_replace.clamp(0.0, 1.0)
            && !(policy.require_confirmation_on_medium_risk
                && self.requires_confirmation
                && matches!(
                    self.risk,
                    TranscriptCorrectionRisk::Medium | TranscriptCorrectionRisk::High
                ))
    }

    pub fn requires_user_confirmation(&self, policy: &TranscriptCorrectionPolicySection) -> bool {
        policy.enabled
            && policy.apply_to_stt
            && policy.require_confirmation_on_medium_risk
            && self.requires_confirmation
            && matches!(
                self.risk,
                TranscriptCorrectionRisk::Medium | TranscriptCorrectionRisk::High
            )
    }

    pub fn clarification_prompt(&self) -> String {
        self.clarification_text
            .clone()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| {
                if self.ambiguities.is_empty() {
                    "I may have misheard that. Could you confirm what you meant?".to_string()
                } else {
                    format!(
                        "I may have misheard that. Could you confirm: {}",
                        self.ambiguities.join("; ")
                    )
                }
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptCorrectionEdit {
    pub from: String,
    pub to: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptCorrectionStatus {
    Corrected,
    Unchanged,
    Disabled,
    SkippedNoModel,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptCorrectionRisk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Deserialize)]
struct TranscriptCorrectionDraft {
    corrected_text: String,
    #[serde(default)]
    confidence: f32,
    #[serde(default)]
    changed: bool,
    #[serde(default)]
    corrections: Vec<TranscriptCorrectionEdit>,
    #[serde(default)]
    ambiguities: Vec<String>,
    #[serde(default)]
    requires_confirmation: bool,
    #[serde(default)]
    clarification_text: Option<String>,
    #[serde(default)]
    risk: Option<String>,
}

pub async fn correct_stt_transcript(
    transcription: &TranscriptionResult,
    stt_capabilities: &SttCapabilities,
    context: &TranscriptCorrectionContext,
    policy: &TranscriptCorrectionPolicySection,
    llm: Option<Arc<dyn LlmAdapter>>,
) -> TranscriptCorrectionResult {
    let raw_text = transcription.text.trim().to_string();
    let stt_trace = stt_trace(transcription, stt_capabilities);
    if !policy.enabled || !policy.apply_to_stt {
        return with_stt_trace(
            fallback_result(
                raw_text,
                TranscriptCorrectionStatus::Disabled,
                "disabled".to_string(),
            ),
            stt_trace,
        );
    }
    let Some(llm) = llm else {
        return with_stt_trace(
            fallback_result(
                raw_text,
                TranscriptCorrectionStatus::SkippedNoModel,
                policy.provider.clone(),
            ),
            stt_trace,
        );
    };
    if llm.name() == "noop_llm" {
        return with_stt_trace(
            fallback_result(
                raw_text,
                TranscriptCorrectionStatus::SkippedNoModel,
                llm.name().to_string(),
            ),
            stt_trace,
        );
    }

    let limited_text = limit_chars(&raw_text, policy.max_input_chars);
    let limited_transcription = TranscriptionResult {
        text: limited_text,
        provider: transcription.provider.clone(),
        confidence: transcription.confidence,
        language: transcription.language.clone(),
        segments: transcription
            .segments
            .iter()
            .take(8)
            .cloned()
            .map(limit_segment_words)
            .collect(),
        alternatives: transcription.alternatives.iter().take(5).cloned().collect(),
    };
    let messages = correction_messages(&limited_transcription, stt_capabilities, context);
    let provider = llm.name().to_string();
    let correction = time::timeout(
        Duration::from_secs(policy.timeout_sec.max(1)),
        llm.chat(&messages),
    )
    .await;

    let response = match correction {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            tracing::debug!(error = ?error, "STT transcript correction LLM failed");
            return with_stt_trace(
                fallback_result(raw_text, TranscriptCorrectionStatus::Failed, provider),
                stt_trace,
            );
        }
        Err(_) => {
            tracing::debug!("STT transcript correction timed out");
            return with_stt_trace(
                fallback_result(raw_text, TranscriptCorrectionStatus::Failed, provider),
                stt_trace,
            );
        }
    };

    match parse_correction_response(&response) {
        Some(mut result) => {
            result.raw_text = raw_text;
            result.provider = provider;
            result.stt = Some(stt_trace);
            result
        }
        None => with_stt_trace(
            fallback_result(raw_text, TranscriptCorrectionStatus::Failed, provider),
            stt_trace,
        ),
    }
}

fn with_stt_trace(
    mut result: TranscriptCorrectionResult,
    stt_trace: TranscriptCorrectionSttTrace,
) -> TranscriptCorrectionResult {
    result.stt = Some(stt_trace);
    result
}

fn stt_trace(
    transcription: &TranscriptionResult,
    capabilities: &SttCapabilities,
) -> TranscriptCorrectionSttTrace {
    TranscriptCorrectionSttTrace {
        provider: transcription.provider.clone(),
        capabilities: capabilities.clone(),
        confidence: transcription.confidence,
        language: transcription.language.clone(),
        segments: transcription
            .segments
            .iter()
            .take(8)
            .cloned()
            .map(limit_segment_words)
            .collect(),
        alternatives: transcription.alternatives.iter().take(5).cloned().collect(),
    }
}

fn limit_segment_words(mut segment: TranscriptionSegment) -> TranscriptionSegment {
    segment.words.truncate(16);
    segment
}

pub fn parse_correction_response(response: &str) -> Option<TranscriptCorrectionResult> {
    let json = json_object_slice(response)?;
    let draft: TranscriptCorrectionDraft = serde_json::from_str(json).ok()?;
    let corrected_text = draft.corrected_text.trim().to_string();
    let changed = draft.changed && !corrected_text.is_empty();
    let status = if changed {
        TranscriptCorrectionStatus::Corrected
    } else {
        TranscriptCorrectionStatus::Unchanged
    };
    Some(TranscriptCorrectionResult {
        raw_text: String::new(),
        corrected_text,
        confidence: draft.confidence.clamp(0.0, 1.0),
        changed,
        corrections: draft.corrections,
        ambiguities: draft.ambiguities,
        requires_confirmation: draft.requires_confirmation,
        clarification_text: draft.clarification_text,
        risk: parse_risk(draft.risk.as_deref()),
        status,
        provider: String::new(),
        stt: None,
    })
}

fn correction_messages(
    transcription: &TranscriptionResult,
    stt_capabilities: &SttCapabilities,
    context: &TranscriptCorrectionContext,
) -> Vec<ChatMessage> {
    vec![
        ChatMessage::new("system", TRANSCRIPT_CORRECTION_SYSTEM_PROMPT),
        ChatMessage::new(
            "user",
            correction_user_prompt(transcription, stt_capabilities, context),
        ),
    ]
}

fn correction_user_prompt(
    transcription: &TranscriptionResult,
    stt_capabilities: &SttCapabilities,
    context: &TranscriptCorrectionContext,
) -> String {
    let mut prompt = format!(
        "Channel: {}\nUser id: {}\nSTT provider: {}\nRaw STT transcript:\n{}",
        context.channel_id, context.user_id, transcription.provider, transcription.text
    );

    prompt.push_str("\n\nSTT capabilities:");
    prompt.push_str(&format!(
        "\n- streaming: {}\n- partial results: {}\n- overall confidence: {}\n- word confidence: {}\n- segment quality: {}\n- segments: {}\n- alternatives: {}\n- language detection: {}\n- language hint: {}\n- timestamps: {}\n- vad: {}\n- diarization: {}\n- translation: {}\n- punctuation: {}\n- text normalization: {}",
        stt_capabilities.supports_streaming,
        stt_capabilities.supports_partial_results,
        stt_capabilities.supports_confidence,
        stt_capabilities.supports_word_confidence,
        stt_capabilities.supports_segment_quality,
        stt_capabilities.supports_segments,
        stt_capabilities.supports_alternatives,
        stt_capabilities.supports_language,
        stt_capabilities.supports_language_hint,
        stt_capabilities.supports_timestamps,
        stt_capabilities.supports_vad,
        stt_capabilities.supports_diarization,
        stt_capabilities.supports_translation,
        stt_capabilities.supports_punctuation,
        stt_capabilities.supports_text_normalization,
    ));
    if !stt_capabilities.supported_input_encodings.is_empty() {
        prompt.push_str(&format!(
            "\n- input encodings: {:?}",
            stt_capabilities.supported_input_encodings
        ));
    }
    if !stt_capabilities.supported_languages.is_empty() {
        prompt.push_str(&format!(
            "\n- supported languages: {}",
            stt_capabilities.supported_languages.join(", ")
        ));
    }

    if let Some(confidence) = transcription.confidence {
        prompt.push_str(&format!("\n\nOverall STT confidence: {confidence:.2}"));
    } else {
        prompt.push_str("\n\nOverall STT confidence: unavailable");
    }
    if let Some(language) = transcription.language.as_deref() {
        prompt.push_str(&format!("\nDetected language: {language}"));
    }

    append_segments(&mut prompt, &transcription.segments);
    append_alternatives(&mut prompt, &transcription.alternatives);
    append_context_list(
        &mut prompt,
        "Known device names",
        &context.known_device_names,
    );
    append_context_list(&mut prompt, "Known room names", &context.room_names);
    append_context_list(
        &mut prompt,
        "Active skill names",
        &context.active_skill_names,
    );
    if let Some(question) = context.last_assistant_question.as_deref() {
        prompt.push_str("\n\nLast assistant question:\n");
        prompt.push_str(question);
    }

    prompt.push_str(
        "\n\nUse the metadata only to correct likely STT errors. Do not invent commands from context.",
    );
    prompt
}

fn append_segments(prompt: &mut String, segments: &[TranscriptionSegment]) {
    if segments.is_empty() {
        prompt.push_str("\n\nSegments: unavailable");
        return;
    }
    prompt.push_str("\n\nSegments:");
    for segment in segments.iter().take(8) {
        let confidence = segment
            .confidence
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "unknown".to_string());
        let timing = match (segment.start_ms, segment.end_ms) {
            (Some(start), Some(end)) => format!("{start}-{end}ms"),
            _ => "no timestamp".to_string(),
        };
        prompt.push_str(&format!(
            "\n- [{timing}, confidence {confidence}] {}",
            segment.text
        ));
        if let Some(avg_logprob) = segment.avg_logprob {
            prompt.push_str(&format!("\n  avg_logprob: {avg_logprob:.2}"));
        }
        if let Some(no_speech_prob) = segment.no_speech_prob {
            prompt.push_str(&format!("\n  no_speech_prob: {no_speech_prob:.2}"));
        }
        append_words(prompt, &segment.words);
    }
}

fn append_words(prompt: &mut String, words: &[TranscriptionWord]) {
    if words.is_empty() {
        return;
    }
    prompt.push_str("\n  words:");
    for word in words.iter().take(16) {
        let confidence = word
            .confidence
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "unknown".to_string());
        let timing = match (word.start_ms, word.end_ms) {
            (Some(start), Some(end)) => format!("{start}-{end}ms"),
            _ => "no timestamp".to_string(),
        };
        prompt.push_str(&format!(
            "\n  - [{timing}, confidence {confidence}] {}",
            word.text
        ));
    }
}

fn append_alternatives(prompt: &mut String, alternatives: &[TranscriptionAlternative]) {
    if alternatives.is_empty() {
        prompt.push_str("\nAlternatives: unavailable");
        return;
    }
    prompt.push_str("\nAlternatives:");
    for alternative in alternatives.iter().take(5) {
        let confidence = alternative
            .confidence
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "unknown".to_string());
        prompt.push_str(&format!(
            "\n- [confidence {confidence}] {}",
            alternative.text
        ));
    }
}

fn append_context_list(prompt: &mut String, label: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    prompt.push_str(&format!("\n\n{label}:"));
    for value in values.iter().take(24) {
        prompt.push_str(&format!("\n- {value}"));
    }
}

fn fallback_result(
    raw_text: String,
    status: TranscriptCorrectionStatus,
    provider: String,
) -> TranscriptCorrectionResult {
    TranscriptCorrectionResult {
        corrected_text: raw_text.clone(),
        raw_text,
        confidence: 0.0,
        changed: false,
        corrections: Vec::new(),
        ambiguities: Vec::new(),
        requires_confirmation: false,
        clarification_text: None,
        risk: TranscriptCorrectionRisk::Low,
        status,
        provider,
        stt: None,
    }
}

fn parse_risk(value: Option<&str>) -> TranscriptCorrectionRisk {
    match value.unwrap_or("low").trim().to_ascii_lowercase().as_str() {
        "medium" => TranscriptCorrectionRisk::Medium,
        "high" => TranscriptCorrectionRisk::High,
        _ => TranscriptCorrectionRisk::Low,
    }
}

fn limit_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

fn json_object_slice(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (offset, ch) in text[start..].char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&text[start..start + offset + ch.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::AudioEncoding;

    fn test_policy() -> TranscriptCorrectionPolicySection {
        TranscriptCorrectionPolicySection {
            enabled: true,
            apply_to_stt: true,
            provider: "minor_llm".to_string(),
            timeout_sec: 2,
            max_input_chars: 500,
            min_confidence_to_replace: 0.55,
            require_confirmation_on_medium_risk: true,
            trace_enabled: true,
        }
    }

    #[test]
    fn parses_structured_transcript_correction() {
        let result = parse_correction_response(
            r#"
            ```json
            {
              "corrected_text": "Turn on the living room light.",
              "confidence": 0.91,
              "changed": true,
              "corrections": [{"from": "lite", "to": "light", "reason": "homophone"}],
              "ambiguities": [],
              "requires_confirmation": false,
              "clarification_text": null,
              "risk": "low"
            }
            ```
            "#,
        )
        .expect("valid JSON correction should parse");

        assert_eq!(result.corrected_text, "Turn on the living room light.");
        assert_eq!(result.status, TranscriptCorrectionStatus::Corrected);
        assert!(result.should_replace(&test_policy()));
    }

    #[test]
    fn medium_risk_confirmation_keeps_raw_transcript() {
        let mut result = parse_correction_response(
            r#"{
              "corrected_text": "Delete the kitchen device.",
              "confidence": 0.88,
              "changed": true,
              "corrections": [],
              "ambiguities": ["The target device is uncertain."],
              "requires_confirmation": true,
              "clarification_text": "Did you mean the kitchen device or something else?",
              "risk": "medium"
            }"#,
        )
        .expect("valid JSON correction should parse");
        result.raw_text = "delete the chicken device".to_string();

        let policy = test_policy();
        assert!(!result.should_replace(&policy));
        assert!(result.requires_user_confirmation(&policy));
        assert_eq!(
            result.clarification_prompt(),
            "Did you mean the kitchen device or something else?"
        );
        assert_eq!(
            result.text_for_processing(&policy),
            "delete the chicken device"
        );
    }

    #[test]
    fn correction_prompt_includes_stt_capabilities_and_domain_context() {
        let transcription = TranscriptionResult {
            text: "turn on the living room lite".to_string(),
            provider: "test_whisper".to_string(),
            confidence: Some(0.71),
            language: Some("en".to_string()),
            segments: vec![TranscriptionSegment {
                text: "living room lite".to_string(),
                start_ms: Some(400),
                end_ms: Some(1100),
                confidence: Some(0.42),
                avg_logprob: Some(-0.92),
                no_speech_prob: Some(0.03),
                words: vec![TranscriptionWord {
                    text: "lite".to_string(),
                    start_ms: Some(900),
                    end_ms: Some(1100),
                    confidence: Some(0.31),
                }],
            }],
            alternatives: vec![TranscriptionAlternative {
                text: "turn on the living room light".to_string(),
                confidence: Some(0.66),
            }],
        };
        let capabilities = SttCapabilities {
            provider: "test_whisper".to_string(),
            supported_input_encodings: vec![AudioEncoding::Pcm16kMono, AudioEncoding::Wav],
            supports_streaming: true,
            supports_partial_results: true,
            supports_confidence: true,
            supports_word_confidence: true,
            supports_segment_quality: true,
            supports_segments: true,
            supports_alternatives: true,
            supports_language: true,
            supports_language_hint: true,
            supports_timestamps: true,
            supports_vad: false,
            supports_diarization: false,
            supports_translation: false,
            supports_punctuation: true,
            supports_text_normalization: false,
            supported_languages: vec!["en".to_string()],
            max_audio_duration_ms: Some(60_000),
            max_audio_bytes: Some(2_000_000),
            provider_specific: Default::default(),
        };
        let mut context = TranscriptCorrectionContext::new("voice", "user-1");
        context.known_device_names = vec!["Living Room Light".to_string()];
        context.room_names = vec!["living room".to_string()];
        context.active_skill_names = vec!["Home Control (home_control)".to_string()];
        context.last_assistant_question = Some("Which room should I use?".to_string());

        let prompt = correction_user_prompt(&transcription, &capabilities, &context);

        assert!(prompt.contains("STT provider: test_whisper"));
        assert!(prompt.contains("- streaming: true"));
        assert!(prompt.contains("- word confidence: true"));
        assert!(prompt.contains("- segment quality: true"));
        assert!(prompt.contains("- alternatives: true"));
        assert!(prompt.contains("- punctuation: true"));
        assert!(prompt.contains("supported languages: en"));
        assert!(prompt.contains("[400-1100ms, confidence 0.42] living room lite"));
        assert!(prompt.contains("avg_logprob: -0.92"));
        assert!(prompt.contains("no_speech_prob: 0.03"));
        assert!(prompt.contains("[900-1100ms, confidence 0.31] lite"));
        assert!(prompt.contains("[confidence 0.66] turn on the living room light"));
        assert!(prompt.contains("Known device names:"));
        assert!(prompt.contains("Living Room Light"));
        assert!(prompt.contains("Last assistant question:"));
    }
}
