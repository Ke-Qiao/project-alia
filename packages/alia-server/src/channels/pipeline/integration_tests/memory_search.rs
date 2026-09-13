use super::*;
use crate::modules::execution::{
    ExecutionService, ExecutionToolRegistry, repository::ExecutionRepository,
};
use crate::modules::memory::context::ContextUse;
use crate::providers::adapter::{LlmToolCall, ToolContext, ToolDefinition, ToolExecutor};
use crate::tools::{BuiltinToolRegistry, builtin::MemorySearch};
use futures::FutureExt;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::json;
use std::collections::HashMap;

async fn with_search_fixture<F, Fut>(test: F)
where
    F: FnOnce(Fixture, HashMap<String, ToolDefinition>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    with_linked_fixture(|mut fixture| async move {
        let db = fixture.conversation.db.as_ref().unwrap().clone();
        let store = fixture.modules.context.policy_store();
        let mut policy = store.current().await.unwrap();
        policy.policy.memory_enabled = true;
        policy.policy.retrieval_layers = vec!["core".into(), "episodic".into()];
        policy.policy.similarity_threshold = 0.0;
        store.replace(policy.version, policy.policy).await.unwrap();

        // The AI branch only requests an authorized context read. Supplying a
        // Noop backend proves the returned material comes from canonical SQL.
        let executor = Arc::new(MemorySearch::new(Some(Arc::new(
            crate::providers::memory::NoopMemory,
        ))));
        let definition = executor.definition();
        db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO tools(name,description,parameters,source,require_confirmation,enabled) VALUES($1,$2,$3,'builtin',false,true)",
            vec![definition.name.clone().into(),definition.description.clone().into(),definition.parameters.clone().into()],
        )).await.unwrap();
        let raw = Arc::new(BuiltinToolRegistry::new());
        raw.register_executor(executor);
        let service = ExecutionService::new(
            ExecutionRepository::new(db),
            raw,
            &fixture.policy.tools,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        service.start().await.unwrap();
        Arc::get_mut(&mut fixture.modules).unwrap().tools =
            Arc::new(ExecutionToolRegistry::new(service.clone()));
        let result = std::panic::AssertUnwindSafe(test(
            fixture,
            HashMap::from([(definition.name.clone(), definition)]),
        ))
        .catch_unwind()
        .await;
        service.shutdown().await.unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}

async fn search_turn(fixture: &Fixture) -> VoiceTurn {
    let mut turn = VoiceTurn::request(CancellationToken::new());
    turn.output_channel = Some(fixture.output.clone());
    turn.source_privacy_epoch = Some(41);
    ensure_turn_context(
        &fixture.modules,
        &turn,
        "test",
        &Uuid::from_u128(42).to_string(),
        "Begin a new tool request.",
    )
    .await
    .unwrap();
    assert!(turn.generation_context().await.unwrap().memory().is_empty());
    turn
}

async fn invoke_search(
    fixture: &Fixture,
    turn: &VoiceTurn,
    tools: &HashMap<String, ToolDefinition>,
    invocation: &str,
    arguments: serde_json::Value,
) -> anyhow::Result<crate::providers::adapter::ToolResult> {
    let generation = turn.generation_context().await?;
    let source = generation.source();
    tools::execute_pipeline_tool_call(
        &fixture.modules,
        &LlmToolCall {
            id: format!("call-{invocation}"),
            tool_name: "memory_search".into(),
            arguments,
        },
        tools,
        &ToolContext {
            user_id: source.owner_id,
            context_id: Some(generation.lease().id()),
            request_id: source.request_id,
            source_received_at: source.source_received_at,
            source_privacy_epoch: Some(source.source_privacy_epoch),
            source_event_id: source.source_event_id,
            invocation_key: invocation.into(),
            conversation_id: source.conversation_id,
            source: "ai".into(),
            channel_id: Some(source.channel_id.clone()),
            cancel: Some(turn.token.clone()),
        },
        turn,
    )
    .await
}

async fn seed_memory(fixture: &Fixture, layer: &str, text: &str) -> Uuid {
    let id = Uuid::new_v4();
    fixture.conversation.db.as_ref().unwrap().execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO memory_items(id,tenant_id,user_id,kind,scope,memory_layer,node_type,content) VALUES($1,$2,$3,'semantic','user',$4,'fact',$5)",
        vec![id.into(),Uuid::nil().into(),Uuid::from_u128(42).into(),layer.into(),text.into()],
    )).await.unwrap();
    id
}

async fn effect_count(fixture: &Fixture) -> i64 {
    fixture
        .conversation
        .db
        .as_ref()
        .unwrap()
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS count FROM execution_effects",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "count")
        .unwrap()
}

#[tokio::test]
async fn successful_memory_search_reads_canonical_sql_with_limits_and_retains_prior_roots() {
    with_search_fixture(|fixture, tools| async move {
        let turn = search_turn(&fixture).await;
        let initial = turn.generation_context().await.unwrap();
        seed_memory(&fixture, "core", "Blueberry preference number one.").await;
        seed_memory(&fixture, "core", "Blueberry preference number two.").await;
        let episode = seed_memory(&fixture, "episodic", "Blueberry trip episode.").await;
        let first = invoke_search(
            &fixture,
            &turn,
            &tools,
            "0:0",
            json!({"query":"Blueberry", "max_results":1, "layers":["core"]}),
        )
        .await
        .unwrap();
        assert!(first.success);
        let first_material = turn.generation_context().await.unwrap();
        assert_eq!(first_material.source(), initial.source());
        assert_eq!(first_material.memory().recall_events.len(), 1);
        assert!(!first_material.memory_prompt().contains("trip episode"));
        let first_id = &first_material.memory().recall_events[0].memory_id;
        assert!(
            first.output["memory_context"]
                .as_str()
                .unwrap()
                .contains("Blueberry")
        );
        let canonical = &first.output["execution"];
        assert_eq!(canonical["status"], "succeeded");
        assert!(!canonical["completion"].to_string().contains("Blueberry"));
        assert_eq!(
            canonical["completion"]["receipt"]["output"],
            json!({"context_read_requested":true})
        );

        let second = invoke_search(
            &fixture,
            &turn,
            &tools,
            "1:0",
            json!({"query":"Blueberry", "max_results":1, "layers":["episodic"]}),
        )
        .await
        .unwrap();
        assert!(second.success);
        let complete = turn.generation_context().await.unwrap();
        assert_eq!(
            complete.memory().recall_events[0].memory_id,
            episode.to_string()
        );
        assert_eq!(complete.all_materialized_memory().recall_events.len(), 2);
        assert_eq!(complete.source(), initial.source());
        assert_eq!(effect_count(&fixture).await, 2);

        fixture
            .conversation
            .db
            .as_ref()
            .unwrap()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE memory_items SET revision=revision+1 WHERE id=$1",
                [Uuid::parse_str(first_id).unwrap().into()],
            ))
            .await
            .unwrap();
        assert!(
            fixture
                .modules
                .context
                .validate(&complete.lease().binding(), ContextUse::OutputAdmission,)
                .await
                .is_err(),
            "a later search discarded the first search's roots"
        );
    })
    .await;
}

async fn catalog_restriction(restriction: &str) {
    let restriction = restriction.to_string();
    with_search_fixture(move |fixture, tools| async move {
        let turn = search_turn(&fixture).await;
        let initial = turn.generation_context().await.unwrap();
        seed_memory(&fixture, "core", "Blueberry private recall.").await;
        // Change the persistent catalog after the model's tool list snapshot.
        let update = match restriction.as_str() {
            "disabled" => "UPDATE tools SET enabled=false WHERE name='memory_search'",
            "role" => "UPDATE tools SET required_role='admin' WHERE name='memory_search'",
            "confirmation" => {
                "UPDATE tools SET require_confirmation=true WHERE name='memory_search'"
            }
            _ => unreachable!(),
        };
        fixture
            .conversation
            .db
            .as_ref()
            .unwrap()
            .execute_unprepared(update)
            .await
            .unwrap();
        let result =
            invoke_search(&fixture, &turn, &tools, "0:0", json!({"query":"Blueberry"})).await;
        let after = turn.generation_context().await.unwrap();
        assert!(after.all_materialized_memory().is_empty());
        assert_eq!(after.source(), initial.source());
        if restriction == "confirmation" {
            let result = result.unwrap();
            assert!(!result.success);
            assert_eq!(result.output["status"], "proposed");
            assert!(result.output["completion"].is_null());
            assert!(!result.output.to_string().contains("private recall"));
            assert_eq!(effect_count(&fixture).await, 1);
        } else {
            assert!(result.is_err());
            assert_eq!(after.lease().id(), initial.lease().id());
            assert_eq!(effect_count(&fixture).await, 0);
        }
    })
    .await;
}

#[tokio::test]
async fn disabled_memory_search_cannot_read_through_a_stale_tool_list() {
    catalog_restriction("disabled").await;
}

#[tokio::test]
async fn memory_search_required_role_is_enforced_before_reading() {
    catalog_restriction("role").await;
}

#[tokio::test]
async fn memory_search_confirmation_exposes_only_pending_execution_fact() {
    catalog_restriction("confirmation").await;
}

#[tokio::test]
async fn response_freeze_prevents_even_authorized_memory_search_dispatch() {
    with_search_fixture(|fixture, tools| async move {
        let turn = search_turn(&fixture).await;
        let initial = turn.generation_context().await.unwrap();
        delivery::prepare_response(
            &fixture.modules,
            Some(&turn),
            "test",
            &Uuid::from_u128(42).to_string(),
            initial.source().conversation_id,
            "text",
        )
        .await
        .unwrap();
        let result =
            invoke_search(&fixture, &turn, &tools, "0:0", json!({"query":"Blueberry"})).await;
        assert!(result.unwrap_err().to_string().contains("frozen"));
        assert_eq!(effect_count(&fixture).await, 0);
        assert_eq!(
            turn.context_binding().await.unwrap().context_id,
            initial.lease().id()
        );
    })
    .await;
}

#[tokio::test]
async fn empty_or_invalid_memory_search_filter_never_widens_the_read() {
    with_search_fixture(|fixture, tools| async move {
        let turn = search_turn(&fixture).await;
        seed_memory(&fixture, "core", "Blueberry private recall.").await;
        let empty = invoke_search(
            &fixture,
            &turn,
            &tools,
            "0:0",
            json!({"query":"Blueberry", "layers":[]}),
        )
        .await
        .unwrap();
        assert!(empty.success);
        assert!(turn.generation_context().await.unwrap().memory().is_empty());
        let denied_layer = invoke_search(
            &fixture,
            &turn,
            &tools,
            "1:0",
            json!({"query":"Blueberry", "layers":["project"]}),
        )
        .await
        .unwrap();
        assert!(denied_layer.success);
        assert!(turn.generation_context().await.unwrap().memory().is_empty());
        for (index, arguments) in [
            json!({"query":"Blueberry", "max_results":0}),
            json!({"query":"Blueberry", "max_results":-1}),
            json!({"query":"Blueberry", "layers":["core", 1]}),
            json!({"query":"Blueberry", "layers":["unknown"]}),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                invoke_search(&fixture, &turn, &tools, &format!("2:{index}"), arguments)
                    .await
                    .is_err()
            );
            assert!(
                turn.generation_context()
                    .await
                    .unwrap()
                    .all_materialized_memory()
                    .is_empty()
            );
        }
    })
    .await;
}
