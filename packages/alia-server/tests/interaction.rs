use alia_server::config::{CachePolicyResourceSection, CacheSection};
use alia_server::modules::delivery::audio::AudioBuffer;
use alia_server::modules::delivery::transport::AudioFormat;
use alia_server::modules::interaction::cache::{
    CACHE_METADATA_SCHEMA, CacheStatus, CacheVersion, CachedResponse,
};
use alia_server::modules::interaction::streaming::{
    pcm16_silence_payload, tts_bytes_to_pcm_payload,
};
use alia_server::modules::interaction::tools::parse_text_tool_calls;
use alia_server::modules::interaction::{
    cache::{CacheContext, CacheWrite, cache_write_for_response, cached_response_is_safe_direct},
    llm::escalation_reason,
    persistence::{PendingTextPersistence, cache_entry_for_pending},
    prompt::{RuntimePromptContext, render_runtime_context_system_prompt},
};
use alia_server::modules::memory::context::ContextBinding;
use uuid::Uuid;

fn cached_fixture() -> CachedResponse {
    CachedResponse {
        version: CacheVersion {
            cache_id: Uuid::new_v4(),
            revision: 1,
            context_id: Uuid::new_v4(),
        },
        owner_id: Uuid::new_v4(),
        scope: "default".into(),
        query_digest: String::new(),
        content_digest: String::new(),
        origin_response_id: Uuid::new_v4(),
        query_text: "query".into(),
        response_text: String::new(),
        metadata: serde_json::json!({}),
        status: CacheStatus::Active,
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
    }
}

fn test_cache_config() -> CacheSection {
    CacheSection {
        enabled: true,
        exact_match_enabled: true,
        semantic_match_enabled: false,
        semantic_threshold: 0.92,
        default_ttl_seconds: 3600,
        skill_cache_ttl_seconds: 300,
    }
}

fn test_cache_policy() -> CachePolicyResourceSection {
    CachePolicyResourceSection {
        dynamic_query_keywords: vec!["time".to_string(), "weather".to_string()],
        memory_query_keywords: vec!["remember".to_string(), "my ".to_string()],
        action_query_keywords: vec!["set ".to_string(), "volume".to_string()],
    }
}

#[test]
fn parses_text_tool_call_envelope() {
    let calls = parse_text_tool_calls(
        r#"{"schema":"alia.tool_calls.v1","tool_calls":[{"name":"get_current_time","arguments":{"format":"human"}}]}"#,
    )
    .unwrap();

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].tool_name, "get_current_time");
    assert_eq!(calls[0].arguments["format"], "human");
}

#[test]
fn parses_fenced_text_tool_call_envelope() {
    let calls = parse_text_tool_calls(
        "```json\n{\"schema\":\"alia.tool_calls.v1\",\"tool_calls\":[{\"tool_name\":\"sensor_list\",\"arguments\":\"{}\"}]}\n```",
    )
    .unwrap();

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].tool_name, "sensor_list");
}

#[test]
fn ignores_plain_assistant_text() {
    let calls = parse_text_tool_calls("Sure, I can help with that.").unwrap();
    assert!(calls.is_empty());
}

#[test]
fn detects_minor_llm_escalation_marker() {
    assert_eq!(
        escalation_reason("[ESCALATE] This needs deeper reasoning."),
        Some("This needs deeper reasoning.")
    );
    assert_eq!(
        escalation_reason("  [ESCALATE]"),
        Some("minor model requested escalation")
    );
    assert!(escalation_reason("Plain response").is_none());
}

#[test]
fn pipeline_cache_entry_uses_default_lookup_scope() {
    let owner = Uuid::new_v4();
    let pending = PendingTextPersistence {
        context: ContextBinding {
            context_id: Uuid::new_v4(),
            owner_id: owner,
            request_id: Uuid::new_v4(),
            source_event_id: None,
            source_privacy_epoch: 0,
        },
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: 0,
        source_event_id: None,
        response_id: uuid::Uuid::new_v4(),
        session_id: uuid::Uuid::new_v4(),
        user_id: owner,
        user_id_text: "test-user".to_string(),
        query_text: "hello".to_string(),
        response_text: "hi".to_string(),
        channel_id: "test-channel".to_string(),
        cache_write: Some(CacheWrite {
            scope: alia_server::modules::interaction::cache::DEFAULT_CACHE_SCOPE.to_string(),
            metadata: serde_json::json!({"schema": alia_server::modules::interaction::cache::CACHE_METADATA_SCHEMA}),
            ttl_seconds: 3600,
            store_embedding: false,
        }),
    };

    let entry = cache_entry_for_pending(&pending).unwrap();
    assert_eq!(
        entry.scope,
        alia_server::modules::interaction::cache::DEFAULT_CACHE_SCOPE
    );
    assert_eq!(entry.query_text, "hello");
    assert_eq!(entry.response_text, "hi");
}

#[test]
fn cache_policy_allows_safe_memory_independent_exact_entries() {
    let entry = cache_write_for_response(
        &test_cache_config(),
        &test_cache_policy(),
        "What is MQTT?",
        "MQTT is a lightweight messaging protocol.",
        CacheContext {
            memory_context_used: false,
            used_tools: false,
            skill_cacheable: None,
            response_was_fallback: false,
            route: "minor_llm",
            skill_catalog: Some("catalog-a"),
        },
    )
    .expect("safe static answer should be cacheable");

    let cached = CachedResponse {
        response_text: "MQTT answer".to_string(),
        metadata: serde_json::json!({
            "schema": CACHE_METADATA_SCHEMA,
            "policy": "private_exact",
            "status": "active",
            "safe_direct_return": true,
            "memory_independent": true,
            "tool_free": true,
            "dynamic_free": true,
            "skill_catalog": "catalog-a",
        }),
        ..cached_fixture()
    };
    assert_eq!(entry.metadata["status"], "pending_insights_review");
    assert_eq!(entry.metadata["safe_direct_return"], false);
    assert!(cached_response_is_safe_direct(&cached, "catalog-a"));
    assert_eq!(entry.metadata["skill_catalog"], "catalog-a");
    assert!(!cached_response_is_safe_direct(&cached, "catalog-b"));
    let mut legacy = cached.clone();
    legacy
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("skill_catalog");
    assert!(!cached_response_is_safe_direct(&legacy, "catalog-a"));
}

#[test]
fn cache_policy_rejects_tool_memory_dynamic_and_legacy_entries() {
    let config = test_cache_config();
    let base = CacheContext {
        memory_context_used: false,
        used_tools: false,
        skill_cacheable: None,
        response_was_fallback: false,
        route: "major_llm",
        skill_catalog: Some("catalog-a"),
    };

    let policy = test_cache_policy();
    assert!(
        cache_write_for_response(
            &config,
            &policy,
            "What is MQTT?",
            "Answer",
            CacheContext {
                skill_catalog: None,
                ..base
            }
        )
        .is_none()
    );
    assert!(
        cache_write_for_response(&config, &policy, "What time is it?", "It is noon.", base)
            .is_none()
    );
    assert!(
        cache_write_for_response(
            &config,
            &policy,
            "What is MQTT?",
            "Answer",
            CacheContext {
                used_tools: true,
                ..base
            },
        )
        .is_none()
    );
    assert!(
        cache_write_for_response(
            &config,
            &policy,
            "What is MQTT?",
            "Answer",
            CacheContext {
                memory_context_used: true,
                ..base
            },
        )
        .is_none()
    );
    assert!(!cached_response_is_safe_direct(
        &CachedResponse {
            response_text: "old cache".to_string(),
            metadata: serde_json::json!({}),
            ..cached_fixture()
        },
        "catalog-a"
    ));
    assert!(!cached_response_is_safe_direct(
        &CachedResponse {
            response_text: "pending cache".to_string(),
            metadata: serde_json::json!({
                "schema": CACHE_METADATA_SCHEMA,
                "policy": "private_exact",
                "status": "pending_insights_review",
                "safe_direct_return": false,
                "memory_independent": true,
                "tool_free": true,
                "dynamic_free": true,
                "skill_catalog": "catalog-a",
            }),
            ..cached_fixture()
        },
        "catalog-a"
    ));
}

#[test]
fn ignores_tool_call_without_schema() {
    let calls =
        parse_text_tool_calls(r#"{"tool_calls":[{"name":"get_current_time","arguments":{}}]}"#)
            .unwrap();

    assert!(calls.is_empty());
}

#[test]
fn ignores_embedded_tool_call_examples() {
    let calls = parse_text_tool_calls(
        r#"Tool use rules:
- If native tool calling is unavailable and a tool is needed, reply with exactly one JSON object and no prose:
  {"tool_calls":[{"name":"tool_name","arguments":{}}]}

Available tools:
[
  {"name":"get_current_time","parameters":{"type":"object"}}
]"#,
    )
    .unwrap();

    assert!(calls.is_empty());
}

#[test]
fn ignores_json_with_trailing_text() {
    let calls = parse_text_tool_calls(
        r#"{"tool_calls":[{"name":"get_current_time","arguments":{}}]}
This is an example, not a call."#,
    )
    .unwrap();

    assert!(calls.is_empty());
}

#[test]
fn ignores_envelope_with_user_facing_content() {
    let calls = parse_text_tool_calls(
        r#"{"schema":"alia.tool_calls.v1","tool_calls":[{"name":"get_current_time","arguments":{}}],"content":"I will call a tool."}"#,
    )
    .unwrap();

    assert!(calls.is_empty());
}

#[test]
fn ignores_text_tool_call_with_non_object_arguments() {
    let calls = parse_text_tool_calls(
        r#"{"schema":"alia.tool_calls.v1","tool_calls":[{"name":"get_current_time","arguments":"not json"}]}"#,
    )
    .unwrap();

    assert!(calls.is_empty());
}

#[test]
fn parses_pseudo_function_text_tool_call() {
    let calls = parse_text_tool_calls(
        r#"memory_store(content="KeQiao likes tsukuyomi", importance:1.0, layer: "core") I remembered it."#,
    )
    .unwrap();

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].tool_name, "memory_store");
    assert_eq!(calls[0].arguments["content"], "KeQiao likes tsukuyomi");
    assert_eq!(calls[0].arguments["layer"], "core");
    assert_eq!(calls[0].arguments["importance"], 1.0);
}

#[test]
fn parses_pseudo_function_search_call() {
    let calls =
        parse_text_tool_calls(r#"memory_search(query="KeQiao favorite musician")"#).unwrap();

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].tool_name, "memory_search");
    assert_eq!(calls[0].arguments["query"], "KeQiao favorite musician");
}

#[test]
fn renders_runtime_context_prompt_outside_persona() {
    let prompt = render_runtime_context_system_prompt(&RuntimePromptContext {
        assistant_name: "玲珑".to_string(),
        core_memories: vec!["The user likes coffee.".to_string()],
        memory_context: None,
        materialized_memory_prompt: None,
        relationship_summary: Some("developing rapport; concise interaction style".to_string()),
        relationship_preference_summary: Some("Prefers concise responses".to_string()),
        user_preferences: vec![("Temperature".to_string(), "26 C".to_string())],
        current_time: "2026-05-16 16:30".to_string(),
        user_presence: "At home".to_string(),
        device_status_summary: "Living room speaker is online".to_string(),
        available_skills: vec!["Climate control".to_string()],
    })
    .unwrap();

    assert!(prompt.contains("## Runtime Context"));
    assert!(prompt.contains("The user likes coffee."));
    assert!(!prompt.contains("## Relationship Context"));
    assert!(!prompt.contains("developing rapport"));
    assert!(!prompt.contains("Prefers concise responses"));
    assert!(prompt.contains("## Skill Routing Context"));
    assert!(prompt.contains("They are not functions to call directly"));
}

#[test]
fn decodes_tts_wav_to_pcm_payload() {
    let pcm = [1i16, -2i16, 3i16, -4i16]
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect::<Vec<_>>();
    let wav = test_wav(&pcm, 16_000, 2);

    let payload = tts_bytes_to_pcm_payload(AudioBuffer::input(wav)).unwrap();

    assert_eq!(payload.format, AudioFormat::Pcm16);
    assert_eq!(payload.sample_rate, 16_000);
    assert_eq!(payload.channels, 2);
    assert_eq!(&*payload.data, &pcm);
}

#[test]
fn builds_pcm_silence_gap() {
    let payload = pcm16_silence_payload(16_000, 1, 80).unwrap();
    assert_eq!(payload.format, AudioFormat::Pcm16);
    assert_eq!(payload.data.len(), 16_000 * 80 / 1_000 * 2);
    assert!(payload.data.iter().all(|byte| *byte == 0));
}

fn test_wav(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let mut wav = Vec::new();
    let data_len = pcm.len() as u32;
    let byte_rate = sample_rate * u32::from(channels) * 2;
    let block_align = channels * 2;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}
