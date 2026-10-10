use super::*;
use serde_json::json;

#[tokio::test]
async fn disabled_memory_still_erases_and_can_be_hot_enabled_then_restarted() {
    use alia_server::modules::memory::context::{ContextBinding, ContextUse};
    use alia_server::{
        modules::{interaction::cache::CacheEntry, memory::MemoryNode},
        providers::adapter::ChatMessage,
    };
    with_fixture(|mut f| async move {
        assert!(!f.config.policies.memory.enabled);
        f.config.policies.cache.enabled = true;
        f.config.policies.cache.exact_match_enabled = true;
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, admin) = delivery::account(&state, "admin").await;
        let response = delivery::draft(&state, owner).await;
        let binding = ContextBinding {
            context_id: response.context_id.unwrap(),
            owner_id: owner,
            request_id: response.key.request_id,
            source_event_id: response.source_event_id,
            source_privacy_epoch: response.source_privacy_epoch.unwrap(),
        };
        state
            .modules
            .responses
            .commit_response(
                owner,
                response.conversation_id.unwrap(),
                response.response_id,
                ChatMessage::new("assistant", "cached reply"),
            )
            .await
            .unwrap();
        state
            .modules
            .cache
            .put(CacheEntry {
                context: binding.clone(),
                origin_response_id: response.response_id,
                scope: "default".into(),
                query_text: "请回应".into(),
                response_text: "cached reply".into(),
                metadata: json!({}),
                ttl_seconds: 60,
            })
            .await
            .unwrap();
        assert_eq!(
            state
                .modules
                .cache
                .load_pending(owner, None, 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(state.modules.memory.clear(owner).await.unwrap(), 0);
        assert!(
            state
                .modules
                .context
                .validate(&binding, ContextUse::OutputAdmission)
                .await
                .is_err()
        );
        assert!(
            state
                .modules
                .cache
                .load_pending(owner, None, 10)
                .await
                .unwrap()
                .is_empty()
        );

        let url = format!("http://{}/api/v1/settings", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let _stop_on_drop = stop.clone().drop_guard();
        let serving = tokio::spawn(runtime.run());
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .patch(&url)
            .bearer_auth(&admin)
            .json(&json!({"core.policies.memory.enabled":true}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let id = state
            .modules
            .memory
            .store(
                owner,
                MemoryNode::new("core", "fact", "a durable preference", 0.8),
            )
            .await
            .unwrap();
        assert!(
            state
                .modules
                .memory
                .get(owner, &id)
                .await
                .unwrap()
                .is_some()
        );
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let restarted = f.builder().build().await.unwrap();
        assert!(
            restarted
                .state()
                .modules
                .context
                .policy_store()
                .current()
                .await
                .unwrap()
                .policy
                .memory_enabled
        );
        assert_eq!(
            restarted
                .state()
                .modules
                .memory
                .get(owner, &id)
                .await
                .unwrap()
                .unwrap()
                .content,
            "a durable preference"
        );
        restarted.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn embedding_identity_changes_need_explicit_policy_commit_after_restart() {
    with_fixture(|mut f| async move {
        f.config.llm.roles.embedding = "embedding".into();
        // Omitted dimensions are valid deployment configuration; the registry
        // must expose the inferred value rather than an invalid PATCH null.
        f.config
            .profiles
            .llm
            .get_mut("embedding")
            .unwrap()
            .dimensions = None;
        let alia_server::config::LlmProviderProfileSection::OpenAi(provider) =
            f.config.providers.llm.get_mut("openai").unwrap()
        else {
            panic!("OpenAI fixture")
        };
        provider.api_key.clear();
        provider.base_url = "http://127.0.0.1:1/v1".into();
        let first = f.builder().build().await.unwrap();
        let original = first
            .state()
            .modules
            .context
            .policy_store()
            .current()
            .await
            .unwrap();
        assert!(original.policy.active_embedding_profile.is_some());
        assert_eq!(
            original.policy.active_embedding_profile,
            original.policy.cache_embedding_profile_id
        );
        first.shutdown().await.unwrap();

        let model = "replacement-embedding-model";
        f.config.profiles.llm.get_mut("embedding").unwrap().model = model.into();
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let observed = state
            .modules
            .context
            .policy_store()
            .current()
            .await
            .unwrap();
        assert_eq!(observed.version, original.version);
        assert_eq!(
            observed.policy.cache_embedding_profile_id,
            original.policy.cache_embedding_profile_id
        );
        assert_eq!(
            observed.policy.active_embedding_profile,
            original.policy.active_embedding_profile
        );
        let actual = state
            .modules
            .embedding
            .preview_profile_settings(&Default::default())
            .await
            .unwrap()
            .unwrap()
            .id()
            .unwrap();
        assert_ne!(
            Some(&actual),
            original.policy.cache_embedding_profile_id.as_ref()
        );
        let (_, admin) = delivery::account(&state, "admin").await;
        let url = format!("http://{}/api/v1/settings", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let _stop_on_drop = stop.clone().drop_guard();
        let serving = tokio::spawn(runtime.run());
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        let credentials = client
            .patch(&url)
            .bearer_auth(&admin)
            .json(&json!({"core.providers.llm.openai.api_key":"isolated-http-fixture-key"}))
            .send()
            .await
            .unwrap();
        assert_eq!(credentials.status(), 200);
        assert_eq!(
            state
                .modules
                .context
                .policy_store()
                .current()
                .await
                .unwrap()
                .version,
            original.version
        );
        let invalid = client
            .patch(&url)
            .bearer_auth(&admin)
            .json(&json!({"core.profiles.llm.embedding.dimensions":true}))
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), 400);
        let forged_pin = client
            .patch(&url)
            .bearer_auth(&admin)
            .json(&json!({"core.policies.memory.active_embedding_profile": actual}))
            .send()
            .await
            .unwrap();
        assert_eq!(forged_pin.status(), 400);
        assert_eq!(
            state
                .modules
                .context
                .policy_store()
                .current()
                .await
                .unwrap()
                .version,
            original.version
        );
        let reconciled = client
            .patch(&url)
            .bearer_auth(&admin)
            .json(&json!({"core.profiles.llm.embedding.model":model}))
            .send()
            .await
            .unwrap();
        assert_eq!(reconciled.status(), 200);
        let durable = state
            .modules
            .context
            .policy_store()
            .current()
            .await
            .unwrap();
        assert_eq!(durable.version, original.version + 1);
        assert_eq!(
            durable.policy.cache_embedding_profile_id,
            Some(actual.clone())
        );
        assert_eq!(durable.policy.active_embedding_profile, Some(actual));

        // Reproduce interruption after durable policy + desired settings were
        // published but before the provider acknowledged its runtime change.
        let retry = std::collections::BTreeMap::from([(
            "core.profiles.llm.embedding.model".into(),
            json!("third-embedding-model"),
        )]);
        let intended = state
            .modules
            .embedding
            .preview_profile_settings(&retry)
            .await
            .unwrap()
            .unwrap();
        let mut partial_policy = durable.policy;
        partial_policy.cache_embedding_profile_id = Some(intended.id().unwrap());
        partial_policy.active_embedding_profile = partial_policy.cache_embedding_profile_id.clone();
        state
            .modules
            .context
            .policy_store()
            .replace(durable.version, partial_policy)
            .await
            .unwrap();
        state.settings.batch_update(&retry).await;
        assert_ne!(
            state
                .modules
                .embedding
                .preview_profile_settings(&Default::default())
                .await
                .unwrap(),
            Some(intended.clone())
        );
        let repaired = client
            .patch(&url)
            .bearer_auth(&admin)
            .json(&retry)
            .send()
            .await
            .unwrap();
        assert_eq!(repaired.status(), 200);
        let result: serde_json::Value = repaired.json().await.unwrap();
        assert_eq!(result["success"], true);
        assert!(
            result["applied"]
                .as_array()
                .unwrap()
                .contains(&json!("core.profiles.llm.embedding.model"))
        );
        assert_eq!(
            state
                .modules
                .embedding
                .preview_profile_settings(&Default::default())
                .await
                .unwrap(),
            Some(intended)
        );

        let changed = client
            .patch(&url)
            .bearer_auth(&admin)
            .json(&json!({"core.profiles.llm.embedding.model":"text-embedding-3-large"}))
            .send()
            .await
            .unwrap();
        assert_eq!(changed.status(), 200);
        let current = state
            .modules
            .embedding
            .preview_profile_settings(&Default::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.dimensions, 3072);
        let current_policy = state
            .modules
            .context
            .policy_store()
            .current()
            .await
            .unwrap();
        assert_eq!(
            current_policy.policy.active_embedding_profile,
            Some(current.id().unwrap())
        );
        assert_eq!(
            state
                .settings
                .get("core.profiles.llm.embedding.dimensions")
                .await,
            Some(json!(3072))
        );
        assert_eq!(
            state
                .modules
                .context
                .policy_store()
                .current()
                .await
                .unwrap()
                .policy
                .cache_embedding_profile_id,
            Some(current.id().unwrap())
        );
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let restarted = f.builder().build().await.unwrap();
        let restored = restarted
            .state()
            .modules
            .context
            .policy_store()
            .current()
            .await
            .unwrap();
        assert_eq!(
            restored.policy.active_embedding_profile,
            Some(current.id().unwrap())
        );
        assert_eq!(
            restarted
                .state()
                .modules
                .embedding
                .preview_profile_settings(&Default::default())
                .await
                .unwrap(),
            Some(current.clone())
        );
        assert_eq!(
            restarted
                .state()
                .modules
                .context
                .policy_store()
                .current()
                .await
                .unwrap()
                .policy
                .cache_embedding_profile_id,
            Some(current.id().unwrap())
        );
        restarted.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn policy_patch_commits_before_ram_and_requires_current_admin_and_version() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (_, admin) = delivery::account(&state, "admin").await;
        let (_, user) = delivery::account(&state, "user").await;
        let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap();
        let url = format!("http://{}/api/v1/settings", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let _stop_on_drop = stop.clone().drop_guard();
        let serving = tokio::spawn(runtime.run());
        assert_eq!(client.get(&url).bearer_auth(&user).send().await.unwrap().status(), 403);
        let response = client.get(&url).bearer_auth(&admin).send().await.unwrap();
        assert_eq!(response.status(), 200);
        let tag = response.headers()["etag"].to_str().unwrap().to_owned();
        let initial = state.modules.context.policy_store().current().await.unwrap();
        let before = state.interaction_policies.read().await.conversation.max_context_messages;
        let next = if before == 8 { 9 } else { 8 };
        let change = json!({"core.policies.conversation.max_context_messages":next});
        assert_eq!(client.patch(&url).bearer_auth(&user).json(&change).send().await.unwrap().status(), 403);
        assert_eq!(client.patch(&url).bearer_auth(&admin).json(&json!({"core.policies.memory.capture_mode":"typo"})).send().await.unwrap().status(), 400);
        state.db.execute_unprepared("CREATE FUNCTION fail_context_policy() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected policy write failure'; END $$; CREATE TRIGGER fail_context_policy BEFORE UPDATE ON context_policy FOR EACH ROW EXECUTE FUNCTION fail_context_policy()").await.unwrap();
        let failed = client.patch(&url).bearer_auth(&admin).header("if-match", &tag).json(&change).send().await.unwrap();
        assert_eq!(failed.status(), 503);
        assert!(!failed.text().await.unwrap().contains("injected"));
        assert_eq!(state.interaction_policies.read().await.conversation.max_context_messages, before);
        assert_eq!(state.settings.get("core.policies.conversation.max_context_messages").await, Some(json!(before)));
        assert_eq!(state.modules.context.policy_store().current().await.unwrap().version, initial.version);
        state.db.execute_unprepared("DROP TRIGGER fail_context_policy ON context_policy; DROP FUNCTION fail_context_policy()").await.unwrap();
        let success = client.patch(&url).bearer_auth(&admin).header("if-match", &tag).json(&change).send().await.unwrap();
        assert_eq!(success.status(), 200);
        let result: serde_json::Value = success.json().await.unwrap();
        assert_eq!(result["context_policy_version"], initial.version+1);
        assert_eq!(state.interaction_policies.read().await.conversation.max_context_messages, next);
        assert_eq!(client.patch(&url).bearer_auth(&admin).header("if-match", &tag).json(&json!({"core.policies.memory.capture_mode":"none"})).send().await.unwrap().status(), 409);
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving).await.unwrap().unwrap().unwrap();
        let restarted = f.builder().build().await.unwrap();
        assert_eq!(restarted.state().interaction_policies.read().await.conversation.max_context_messages, next);
        restarted.shutdown().await.unwrap();
    }).await;
}

#[tokio::test]
async fn independent_server_patch_preserves_other_instances_policy_changes() {
    with_fixture(|f| async move {
        let first = f.builder().build().await.unwrap();
        let second = f.builder().build().await.unwrap();
        let first_state = first.state().clone();
        let second_state = second.state().clone();
        let (_, admin) = delivery::account(&first_state, "admin").await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let url_a = format!("http://{}/api/v1/settings", first.http_addr());
        let url_b = format!("http://{}/api/v1/settings", second.http_addr());
        let stop_a = first_state.cancel_token.clone();
        let stop_b = second_state.cancel_token.clone();
        let _stop_a_on_drop = stop_a.clone().drop_guard();
        let _stop_b_on_drop = stop_b.clone().drop_guard();
        let serving_a = tokio::spawn(first.run());
        let serving_b = tokio::spawn(second.run());
        let old = client.get(&url_b).bearer_auth(&admin).send().await.unwrap();
        let old_tag = old.headers()["etag"].to_str().unwrap().to_owned();
        let changed = client
            .patch(&url_a)
            .bearer_auth(&admin)
            .json(&json!({"core.policies.conversation.max_context_messages":7}))
            .send()
            .await
            .unwrap();
        assert_eq!(changed.status(), 200);
        let stale = client
            .patch(&url_b)
            .bearer_auth(&admin)
            .header("if-match", old_tag)
            .json(&json!({"core.policies.memory.capture_mode":"none"}))
            .send()
            .await
            .unwrap();
        assert_eq!(stale.status(), 409);
        // Without If-Match the server still reads current SQL policy and uses
        // CAS; the unrelated value may never come from its older RAM snapshot.
        assert_eq!(
            client
                .patch(&url_b)
                .bearer_auth(&admin)
                .json(&json!({"core.policies.memory.capture_mode":"none"}))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let actual = first_state
            .modules
            .context
            .policy_store()
            .current()
            .await
            .unwrap();
        assert_eq!(actual.policy.max_history_messages, 7);
        assert_eq!(
            actual.policy.capture_mode,
            alia_server::config::MemoryCaptureMode::None
        );
        let projected = client
            .get(&url_a)
            .bearer_auth(&admin)
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(
            projected["core.policies.memory.capture_mode"]["value"],
            "none"
        );
        stop_b.cancel();
        stop_a.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving_b)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), serving_a)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    })
    .await;
}
