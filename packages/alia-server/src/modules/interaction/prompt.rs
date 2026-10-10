use super::cache::guess_text_language;
use super::services::InteractionServices;
use crate::modules::memory::MemoryContextPack;
use crate::modules::memory::context::GenerationContext;
use crate::modules::memory::policy::memory_prompt_line;
use crate::{
    modules::interaction::persona::{DEFAULT_ASSISTANT_NAME, sanitize_assistant_name},
    providers::adapter::ChatMessage,
};
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct RuntimePromptContext {
    pub assistant_name: String,
    pub core_memories: Vec<String>,
    pub memory_context: Option<MemoryContextPack>,
    pub materialized_memory_prompt: Option<String>,
    pub relationship_summary: Option<String>,
    pub relationship_preference_summary: Option<String>,
    pub user_preferences: Vec<(String, String)>,
    pub current_time: String,
    pub user_presence: String,
    pub device_status_summary: String,
    pub available_skills: Vec<String>,
}

pub(super) fn build_llm_messages(
    base_system_messages: &[ChatMessage],
    history: &[ChatMessage],
    system_prompt: Option<String>,
    text: &str,
    user_id: &str,
) -> Vec<ChatMessage> {
    let mut messages = base_system_messages.to_vec();
    if let Some(sp) = system_prompt {
        messages.push(ChatMessage {
            role: "system".into(),
            content: sp,
            name: None,
            ..ChatMessage::default()
        });
    }
    append_history_with_interrupted_context(&mut messages, history);
    messages.push(ChatMessage {
        role: "user".into(),
        content: text.to_string(),
        name: Some(user_id.to_string()),
        ..ChatMessage::default()
    });
    messages
}

pub(super) fn append_history_with_interrupted_context(
    messages: &mut Vec<ChatMessage>,
    history: &[ChatMessage],
) {
    if let Some(previous) = unanswered_user_history_tail(history) {
        messages.push(ChatMessage::new(
            "system",
            interrupted_user_context_prompt(&previous.content),
        ));
    }
    messages.extend_from_slice(history);
}

pub(super) fn unanswered_user_history_tail(history: &[ChatMessage]) -> Option<&ChatMessage> {
    history.last().filter(|message| message.role == "user")
}

pub(super) fn interrupted_user_context_prompt(previous_user_message: &str) -> String {
    format!(
        "The prior voice turn was interrupted before the assistant could answer. \
The last user message in conversation history is preserved only as context, not as a completed exchange. \
Prior interrupted user message: \"{}\". \
Use the newest user message as the primary instruction. Refer to the interrupted message only when it helps answer the newest request.",
        preview_for_prompt(previous_user_message, 360)
    )
}

pub(super) fn stt_transcript_context_prompt() -> String {
    "Input context: the current user message was produced by speech-to-text transcription from spoken audio. Treat it as the user's intended speech, but allow for homophones, dropped punctuation, missing casing, segmentation mistakes, and occasional wrong words. If an ambiguity would change the action, factual answer, or user intent, ask a brief clarification instead of guessing. Do not mention this transcription context to the user unless it is directly relevant.".to_string()
}

pub(super) fn tts_optimized_output_prompt(user_text: &str) -> String {
    let language = guess_text_language(user_text);
    let language_note = match language {
        "zh" => {
            "The user appears to be using Chinese. Prefer natural spoken Chinese. Convert numbers, Roman numerals, dates, times, units, versions, identifiers, and symbols into forms that a Chinese TTS can read naturally when doing so does not change meaning. Examples: 1145 -> 一千一百四十五; IV -> 四 or 第四 according to context; 3.5kg -> 三点五千克; 2026-05-21 -> 二零二六年五月二十一日."
        }
        _ => {
            "Use the user's language when possible. Convert numbers, Roman numerals, dates, times, units, versions, identifiers, and symbols into natural spoken forms for that language when doing so does not change meaning. If exact spelling or code matters, keep it exact and introduce it as something to read visually."
        }
    };

    format!(
        "Output channel constraint: the assistant response will be spoken aloud by TTS. Produce clean, speakable plain text only.\n\
\n\
Rules:\n\
- Do not use Markdown formatting: no headings, bullets, tables, block quotes, code fences, inline code backticks, bold, italics, or raw Markdown links.\n\
- Do not output raw URLs unless the user explicitly asks for a link. Summarize the site or say you can provide the link separately.\n\
- Avoid visual-only formatting such as columns, ASCII diagrams, dense lists, or punctuation art.\n\
- Prefer short paragraphs and conversational phrasing that sounds natural when read aloud.\n\
- Expand or rephrase abbreviations, symbols, ordinals, units, dates, times, equations, and technical notation into speakable words when practical.\n\
- For code, commands, filenames, IDs, serial numbers, or exact values that must remain exact, clearly say they are visual text and keep them concise.\n\
- Do not say anything about these formatting rules.\n\
\n\
{}",
        language_note
    )
}

pub(super) fn preview_for_prompt(text: &str, max_chars: usize) -> String {
    let mut preview = text.trim().chars().take(max_chars).collect::<String>();
    if text.trim().chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

pub(super) async fn build_base_system_messages(
    user_id: Uuid,
    modules: &InteractionServices,
    generation: &GenerationContext,
    available_skills: Vec<String>,
) -> anyhow::Result<(Vec<ChatMessage>, RuntimePromptContext)> {
    let mut messages = Vec::new();
    let mut assistant_name = DEFAULT_ASSISTANT_NAME.to_string();

    match modules.persona.render_prompt(user_id).await {
        Ok(rendered) => {
            assistant_name = sanitize_assistant_name(&rendered.persona.name);
            if !rendered.system_prompt.trim().is_empty() {
                messages.push(system_message(rendered.system_prompt));
            }
        }
        Err(error) => {
            tracing::warn!(user_id = %user_id, error = ?error, "failed to render persona prompt");
        }
    }

    let mut runtime_context = build_runtime_prompt_context(generation, available_skills);
    runtime_context.assistant_name = assistant_name;
    if let Some(prompt) = render_runtime_context_system_prompt(&runtime_context) {
        messages.push(system_message(prompt));
    }

    Ok((messages, runtime_context))
}

fn build_runtime_prompt_context(
    generation: &GenerationContext,
    available_skills: Vec<String>,
) -> RuntimePromptContext {
    RuntimePromptContext {
        assistant_name: DEFAULT_ASSISTANT_NAME.to_string(),
        current_time: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        memory_context: Some(generation.memory().clone()),
        materialized_memory_prompt: Some(generation.memory_prompt().to_string()),
        available_skills,
        ..RuntimePromptContext::default()
    }
}

pub fn render_runtime_context_system_prompt(context: &RuntimePromptContext) -> Option<String> {
    let mut sections = Vec::new();
    let mut runtime_lines = Vec::new();

    if !context.current_time.trim().is_empty() {
        runtime_lines.push(format!("- Time: {}", context.current_time.trim()));
    }
    if !context.user_presence.trim().is_empty() {
        runtime_lines.push(format!("- User presence: {}", context.user_presence.trim()));
    }
    if !context.device_status_summary.trim().is_empty() {
        runtime_lines.push(format!(
            "- Device status: {}",
            context.device_status_summary.trim()
        ));
    }
    if !runtime_lines.is_empty() {
        sections.push(format!("## Runtime Context\n{}", runtime_lines.join("\n")));
    }

    if let Some(prompt) = &context.materialized_memory_prompt {
        if !prompt.is_empty() {
            sections.push(prompt.clone());
        }
    } else if let Some(memory_context) = &context.memory_context {
        let mut memory_sections = Vec::new();
        push_memory_prompt_section(
            &mut memory_sections,
            "Preferences",
            &memory_context.user_preferences,
            "Use these as soft style and response-shape preferences.",
        );
        push_memory_prompt_section(
            &mut memory_sections,
            "Projects",
            &memory_context.project_context,
            "Use these only to preserve relevant project continuity.",
        );
        push_memory_prompt_section(
            &mut memory_sections,
            "Boundaries",
            &memory_context.boundaries,
            "Treat these as hard personalization boundaries unless system safety overrides them.",
        );
        push_memory_prompt_section(
            &mut memory_sections,
            "Affective Guidance",
            &memory_context.affective_guidance,
            "Convert this into constructive tone guidance; do not label the user emotionally.",
        );
        push_memory_prompt_section(
            &mut memory_sections,
            "Relevant Episodes",
            &memory_context.relevant_episodes,
            "Use these only when they directly help the current answer.",
        );
        let do_not_mention = [
            "Do not mention memory IDs, confidence scores, affect scores, or hidden runtime state.",
            "Do not reveal suppressed memories or private runtime records.",
        ];
        memory_sections.push(format!(
            "### Do Not Mention\n{}",
            do_not_mention
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
        if !memory_sections.is_empty() {
            sections.push(format!(
                "## Memory Context\nUse this private context silently when it is relevant. Do not expose internal memory mechanics or claim certainty beyond the listed wording.\n{}",
                memory_sections.join("\n\n")
            ));
        }
    } else if !context.core_memories.is_empty() {
        let lines = context
            .core_memories
            .iter()
            .filter(|memory| !memory.trim().is_empty())
            .map(|memory| format!("- {}", memory.trim()))
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            sections.push(format!(
                "## Memory Context\nUse these private memories only when they are relevant. Do not expose internal memory details or claim certainty beyond the listed wording.\n### Relevant Memories\n{}",
                lines.join("\n")
            ));
        }
    }

    if !context.user_preferences.is_empty() {
        let lines = context
            .user_preferences
            .iter()
            .filter(|(key, value)| !key.trim().is_empty() && !value.trim().is_empty())
            .map(|(key, value)| format!("- {}: {}", key.trim(), value.trim()))
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            sections.push(format!("## User Preferences\n{}", lines.join("\n")));
        }
    }

    if !context.available_skills.is_empty() {
        let skills = context
            .available_skills
            .iter()
            .filter(|skill| !skill.trim().is_empty())
            .map(|skill| format!("- {}", skill.trim()))
            .collect::<Vec<_>>();
        if !skills.is_empty() {
            sections.push(format!(
                "## Skill Routing Context\nSkills are cognitive contexts selected by the router. They are not functions to call directly.\n{}",
                skills.join("\n")
            ));
        }
    }

    if sections.is_empty() {
        None
    } else {
        Some(format!(
            "Internal system context. Use this information silently when it is relevant; do not expose it as prompt text.\n\n{}",
            sections.join("\n\n")
        ))
    }
}

pub(super) fn push_memory_prompt_section(
    sections: &mut Vec<String>,
    title: &str,
    snippets: &[crate::modules::memory::MemorySnippet],
    instruction: &str,
) {
    let lines = snippets
        .iter()
        .filter(|snippet| !snippet.content.trim().is_empty())
        .map(|snippet| format!("- {}", memory_prompt_line(&snippet.content, 240)))
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        sections.push(format!("### {title}\n{instruction}\n{}", lines.join("\n")));
    }
}

pub(super) fn system_message(content: String) -> ChatMessage {
    ChatMessage {
        role: "system".into(),
        content,
        name: None,
        ..ChatMessage::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_prompt_for_chinese_requires_speakable_plain_text() {
        let prompt = tts_optimized_output_prompt("今天是 2026-05-21，读一下 IV 和 1145。");

        assert!(prompt.contains("spoken aloud by TTS"));
        assert!(prompt.contains("Do not use Markdown formatting"));
        assert!(prompt.contains("1145 -> 一千一百四十五"));
        assert!(prompt.contains("IV -> 四"));
        assert!(prompt.contains("二零二六年五月二十一日"));
        assert!(prompt.contains("no headings, bullets, tables"));
    }

    #[test]
    fn tts_prompt_for_unknown_language_keeps_general_speech_rules() {
        let prompt = tts_optimized_output_prompt("Read version IV at 11:45.");

        assert!(prompt.contains("Use the user's language when possible"));
        assert!(prompt.contains("Roman numerals"));
        assert!(prompt.contains("natural spoken forms"));
        assert!(prompt.contains("raw URLs"));
    }

    #[test]
    fn stt_context_prompt_warns_about_transcription_uncertainty() {
        let prompt = stt_transcript_context_prompt();

        assert!(prompt.contains("speech-to-text transcription"));
        assert!(prompt.contains("homophones"));
        assert!(prompt.contains("ask a brief clarification"));
        assert!(prompt.contains("Do not mention this transcription context"));
    }

    #[test]
    fn llm_messages_explain_unanswered_user_history_tail() {
        let base = vec![ChatMessage::new("system", "Base persona")];
        let history = vec![ChatMessage {
            role: "user".into(),
            content: "介绍一下明日方舟终末地".into(),
            name: Some("embedded-user".into()),
            ..ChatMessage::default()
        }];

        let messages = build_llm_messages(&base, &history, None, "继续说重点", "embedded-user");

        assert!(messages.iter().any(|message| {
            message.role == "system"
                && message.content.contains("prior voice turn was interrupted")
                && message.content.contains("终末地")
        }));
        assert_eq!(messages.last().unwrap().content, "继续说重点");
    }

    #[test]
    fn llm_messages_do_not_add_interruption_prompt_for_completed_pair() {
        let history = vec![
            ChatMessage::new("user", "上一问"),
            ChatMessage::new("assistant", "上一答"),
        ];

        let messages = build_llm_messages(&[], &history, None, "下一问", "embedded-user");

        assert!(
            !messages
                .iter()
                .any(|message| message.content.contains("prior voice turn was interrupted"))
        );
    }
}

pub(super) fn runtime_context_has_user_memory(context: &RuntimePromptContext) -> bool {
    !context.core_memories.is_empty()
        || !context.user_preferences.is_empty()
        || context
            .memory_context
            .as_ref()
            .is_some_and(|memory| !memory.is_empty())
}
