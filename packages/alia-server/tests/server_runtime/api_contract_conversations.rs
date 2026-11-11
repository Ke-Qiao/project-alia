//! Conversation contracts through an owned HTTP listener and disposable PostgreSQL schema.
use super::*;
use alia_server::openapi::ApiDoc;
use sea_orm::{DatabaseBackend, Statement};
use serde_json::{Value, json};
use utoipa::OpenApi;

async fn json_response(
    request: reqwest::RequestBuilder,
    expected: u16,
    component: &str,
    array: bool,
) -> Value {
    let response = request.send().await.unwrap();
    let url = response.url().to_string();
    let status = response.status();
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    let text = response.text().await.unwrap();
    assert_eq!(status.as_u16(), expected, "{url}: {text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
    let reference = json!({"$ref": format!("#/components/schemas/{component}")});
    let mut schema = if array {
        json!({"type":"array", "items":reference})
    } else {
        reference
    };
    schema["components"] = document["components"].clone();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .offline()
        .build(&schema)
        .unwrap();
    let errors: Vec<_> = validator
        .iter_errors(&body)
        .map(|error| error.to_string())
        .collect();
    assert!(errors.is_empty(), "{url}: {errors:?}");
    eprintln!(
        "[conversation-http] {url} status={status} component={component} bytes={} items={:?}",
        text.len(),
        body.as_array().map(Vec::len)
    );
    body
}

async fn bad_request(request: reqwest::RequestBuilder) {
    let response = request.send().await.unwrap();
    let url = response.url().to_string();
    assert_eq!(response.status(), 400, "{url}");
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    let body = response.text().await.unwrap();
    assert!(!body.is_empty());
    eprintln!("[conversation-http] {url} status=400 text={body}");
}

async fn session(db: &DatabaseConnection, owner: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'http-contract',$2)",
        [id.into(), owner.map(|id| id.to_string()).into()],
    ))
    .await
    .unwrap();
    id
}

async fn event(
    db: &DatabaseConnection,
    session: Uuid,
    turn: Uuid,
    sequence: i32,
    kind: &str,
    payload: Value,
) {
    db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "INSERT INTO conversation_events(id,session_id,turn_id,sequence,event_type,actor,payload,created_at) VALUES($1,$2,$3,$4,$5,'fixture',$6,'2026-01-01T00:00:00Z')",
        [Uuid::new_v4().into(), session.into(), turn.into(), sequence.into(), kind.into(), payload.into()],
    )).await.unwrap();
}

#[tokio::test]
async fn conversations_http_authority_rejections_and_owned_delete() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let (other, other_token) = delivery::account(&state, "user").await;
        let (_, admin) = delivery::account(&state, "admin").await;
        let own = session(&state.db, Some(owner)).await;
        let foreign = session(&state.db, Some(other)).await;
        let anonymous = session(&state.db, None).await;
        let missing = Uuid::new_v4();
        for id in [own, foreign] {
            state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "INSERT INTO chat_messages(id,session_id,role,content) VALUES($1,$2,'assistant','fixture-only message')",
                [Uuid::new_v4().into(), id.into()],
            )).await.unwrap();
            event(&state.db, id, Uuid::new_v4(), 0, "sentence_expression_llm", json!(null)).await;
        }
        let base = format!("http://{}/api/v1/conversations", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = skills::client();
            let own_list = json_response(client.get(&base).bearer_auth(&token), 200, "Conversation", true).await;
            assert_eq!(own_list.as_array().unwrap().len(), 1);
            assert_eq!(own_list[0]["id"], own.to_string());
            assert_eq!(own_list[0]["title"], "New Chat");
            assert_eq!(own_list[0]["creator_id"], owner.to_string());
            let all = json_response(client.get(&base).bearer_auth(&admin), 200, "Conversation", true).await;
            assert_eq!(all.as_array().unwrap().len(), 3);
            assert!(all.as_array().unwrap().iter().any(|row| row["id"] == anonymous.to_string() && row["creator_id"].is_null()));

            for (method, suffix, component, array) in [
                (reqwest::Method::GET, "", "Conversation", false),
                (reqwest::Method::GET, "/messages", "Message", true),
                (reqwest::Method::GET, "/events", "AgentEvent", true),
                (reqwest::Method::GET, "/expression-traces", "ExpressionTrace", true),
                (reqwest::Method::DELETE, "", "DeleteConversationResult", false),
            ] {
                for id in [foreign, anonymous, missing] {
                    let expected = if id == missing { 404 } else { 403 };
                    let body = json_response(client.request(method.clone(), format!("{base}/{id}{suffix}")).bearer_auth(&token), expected, "ErrorResponse", false).await;
                    assert_eq!(body["error"]["code"], if expected == 404 { "NOT_FOUND" } else { "AUTH_FORBIDDEN" });
                    assert!(body["error"].get("reason").is_none());
                }
                bad_request(client.request(method.clone(), format!("{base}/not-a-uuid{suffix}")).bearer_auth(&token)).await;
                let denied = json_response(client.request(method.clone(), format!("{base}/{own}{suffix}")), 401, "ErrorResponse", false).await;
                assert_eq!(denied["error"]["reason"], "token_missing");
                if method == reqwest::Method::GET {
                    json_response(client.get(format!("{base}/{foreign}{suffix}")).bearer_auth(&admin), 200, component, array).await;
                    json_response(client.get(format!("{base}/{own}{suffix}")).bearer_auth(&token), 200, component, array).await;
                }
            }
            for path in [base.clone(), format!("{base}/{own}/messages"), format!("{base}/{own}/events"), format!("{base}/{own}/expression-traces")] {
                for query in ["limit=-1", "limit=18446744073709551616", "limit=one"] {
                    bad_request(client.get(format!("{path}?{query}")).bearer_auth(&token)).await;
                }
            }
            bad_request(client.get(format!("{base}/{own}/expression-traces?turn_id=bad")).bearer_auth(&token)).await;

            let deleted = json_response(client.delete(format!("{base}/{own}")).bearer_auth(&token), 200, "DeleteConversationResult", false).await;
            assert_eq!(deleted, json!({"success":true}));
            json_response(client.delete(format!("{base}/{own}")).bearer_auth(&token), 404, "ErrorResponse", false).await;
            for table in ["chat_sessions", "chat_messages", "conversation_events"] {
                let column = if table == "chat_sessions" { "id" } else { "session_id" };
                let row = state.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                    format!("SELECT count(*) FILTER (WHERE {column}=$1) AS own, count(*) FILTER (WHERE {column}=$2) AS other FROM {table}"),
                    [own.into(), foreign.into()],
                )).await.unwrap().unwrap();
                assert_eq!(row.try_get::<i64>("", "own").unwrap(), 0, "{table}");
                assert_eq!(row.try_get::<i64>("", "other").unwrap(), 1, "{table}");
            }
            json_response(client.get(format!("{base}/{foreign}")).bearer_auth(&other_token), 200, "Conversation", false).await;
            json_response(client.delete(format!("{base}/{anonymous}")).bearer_auth(&admin), 200, "DeleteConversationResult", false).await;
        }).catch_unwind().await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving).await.unwrap().unwrap().unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }).await;
}

#[tokio::test]
async fn conversations_http_preserves_json_and_filters_trace_sources() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let own = session(&state.db, Some(owner)).await;
        let foreign = session(&state.db, Some(owner)).await;
        let early_turn = Uuid::from_u128(1);
        let late_turn = Uuid::from_u128(2);
        let payloads = [json!(null), json!([1, "二", false, null]), json!("scalar"), json!(42), json!(true)];
        for (sequence, payload) in payloads.iter().enumerate().rev() {
            event(&state.db, own, late_turn, sequence as i32, "sentence_expression_llm", payload.clone()).await;
        }
        let request = json!(["arbitrary request", null]);
        let adjustments = json!([false, 7, {"path":"speech.style"}]);
        event(&state.db, own, early_turn, 8, "tts_provider_adjustments", json!({"request":request,"adjustments":adjustments})).await;
        event(&state.db, own, early_turn, 9, "tts_provider_adjustments", json!(null)).await;
        event(&state.db, own, early_turn, -1, "affect_state", json!({"private":"not a trace"})).await;
        event(&state.db, foreign, late_turn, -2, "sentence_expression_llm", json!("other-session")).await;
        for (index, content) in ["最早\nmessage", "", "latest"].into_iter().enumerate().rev() {
            state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "INSERT INTO chat_messages(id,session_id,user_id,role,content,created_at) VALUES($1,$2,$3,'user',$4,'2026-01-01T00:00:00Z'::timestamptz + $5 * interval '1 second')",
                [Uuid::new_v4().into(), own.into(), owner.to_string().into(), content.into(), (index as i32).into()],
            )).await.unwrap();
        }
        let base = format!("http://{}/api/v1/conversations/{own}", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = skills::client();
            let messages = json_response(client.get(format!("{base}/messages")).bearer_auth(&token), 200, "Message", true).await;
            assert_eq!(messages.as_array().unwrap().iter().map(|row| row["content"].as_str().unwrap()).collect::<Vec<_>>(), ["最早\nmessage", "", "latest"]);
            let events = json_response(client.get(format!("{base}/events")).bearer_auth(&token), 200, "AgentEvent", true).await;
            let rows = events.as_array().unwrap();
            assert_eq!(rows.len(), 8);
            assert_eq!(rows.iter().map(|row| row["sequence"].as_i64().unwrap()).collect::<Vec<_>>(), [-1, 0, 1, 2, 3, 4, 8, 9]);
            for (row, payload) in rows[1..6].iter().zip(&payloads) {
                assert_eq!(&row["payload"], payload);
                assert!(row["summary"].is_null());
            }
            let traces = json_response(client.get(format!("{base}/expression-traces")).bearer_auth(&token), 200, "ExpressionTrace", true).await;
            assert_eq!(traces.as_array().unwrap().len(), 2);
            assert_eq!(traces[0]["turn_id"], early_turn.to_string());
            assert_eq!(traces[0]["session_id"], own.to_string());
            assert_eq!(traces[0]["tts_requests"], json!([request]));
            assert_eq!(traces[0]["tts_provider_adjustments"], adjustments);
            assert_eq!(traces[0]["source_events"].as_array().unwrap().len(), 2);
            assert!(traces[0]["source_events"][0].get("summary").is_none());
            assert_eq!(traces[1]["sentence_expression_llm"], json!(payloads));
            let filtered = json_response(client.get(format!("{base}/expression-traces?turn_id={late_turn}")).bearer_auth(&token), 200, "ExpressionTrace", true).await;
            assert_eq!(filtered, json!([traces[1]]));
            let missing = json_response(client.get(format!("{base}/expression-traces?turn_id={}", Uuid::new_v4())).bearer_auth(&token), 200, "ExpressionTrace", true).await;
            assert_eq!(missing, json!([]));
            let limited = json_response(client.get(format!("{base}/expression-traces?limit=1")).bearer_auth(&token), 200, "ExpressionTrace", true).await;
            assert_eq!(limited.as_array().unwrap().len(), 1);
            assert_eq!(limited[0]["turn_id"], late_turn.to_string());
            assert_eq!(limited[0]["sentence_expression_llm"], json!([null]));
            assert_eq!(limited[0]["source_events"].as_array().unwrap().len(), 1);
        }).catch_unwind().await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving).await.unwrap().unwrap().unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }).await;
}

#[tokio::test]
async fn conversations_http_zero_default_and_clamped_limits() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let own = session(&state.db, Some(owner)).await;
        state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_sessions(id,channel_id,creator_id,name,updated_at) SELECT md5('session-' || n::text)::uuid,'http-contract',$1,n::text,'2026-01-01T00:00:00Z'::timestamptz + n * interval '1 second' FROM generate_series(1,501) n",
            [owner.to_string().into()],
        )).await.unwrap();
        state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,role,content,created_at) SELECT md5('message-' || n::text)::uuid,$1,'assistant',n::text,'2026-01-01T00:00:00Z'::timestamptz + n * interval '1 second' FROM generate_series(1,5001) n",
            [own.into()],
        )).await.unwrap();
        state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO conversation_events(id,session_id,turn_id,sequence,event_type,actor,payload,created_at) SELECT md5('event-' || n::text)::uuid,$1,$2,n,'sentence_expression_llm','fixture',to_jsonb(n),'2026-01-01T00:00:00Z'::timestamptz + n * interval '1 second' FROM generate_series(1,5001) n",
            [own.into(), Uuid::new_v4().into()],
        )).await.unwrap();
        let base = format!("http://{}/api/v1/conversations", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = skills::client();
            for (path, component, default, cap) in [
                (base.clone(), "Conversation", 100, 500),
                (format!("{base}/{own}/messages"), "Message", 1000, 5000),
                (format!("{base}/{own}/events"), "AgentEvent", 1000, 5000),
                (format!("{base}/{own}/expression-traces"), "ExpressionTrace", 1000, 5000),
            ] {
                for (query, expected) in [("?limit=0", 0), ("", default), ("?limit=18446744073709551615", cap)] {
                    let body = json_response(client.get(format!("{path}{query}")).bearer_auth(&token), 200, component, true).await;
                    let rows = body.as_array().unwrap();
                    if component == "ExpressionTrace" && expected > 0 {
                        assert_eq!(rows.len(), 1);
                        assert_eq!(rows[0]["source_events"].as_array().unwrap().len(), expected);
                        assert_eq!(rows[0]["sentence_expression_llm"][0], 1);
                        assert_eq!(rows[0]["sentence_expression_llm"][expected - 1], expected);
                    } else {
                        assert_eq!(rows.len(), expected, "{path}{query}");
                    }
                    if expected > 0 && component == "Message" {
                        assert_eq!(rows[0]["content"], "1");
                        assert_eq!(rows[expected - 1]["content"], expected.to_string());
                    }
                    if expected > 0 && component == "Conversation" {
                        assert_eq!(rows[0]["id"], own.to_string());
                        assert_eq!(rows[1]["title"], "501");
                    }
                }
            }
        }).catch_unwind().await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving).await.unwrap().unwrap().unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }).await;
}
