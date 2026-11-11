//! Auth contract checks use localhost HTTP and disposable PostgreSQL, never devices.
use super::*;
use alia_server::entity::{refresh_token, user, user_identity_link};
use alia_server::openapi::ApiDoc;
use alia_server::state::AppState;
use sea_orm::{
    ColumnTrait, DatabaseBackend, DatabaseTransaction, EntityTrait, PaginatorTrait, QueryFilter,
    Statement, TransactionTrait,
};
use serde_json::{Value, json};
use utoipa::OpenApi;

const PASSWORD: &str = "auth-contract-password";

async fn with_http<F, Fut>(test: F)
where
    F: FnOnce(Fixture, AppState, String, reqwest::Client) -> Fut,
    Fut: Future<Output = ()>,
{
    with_fixture(|f| async move {
        let runtime = f
            .builder()
            .with_llm(test_text("auth-contract-no-inference", |_| async {
                anyhow::bail!("auth contract must not invoke inference")
            }))
            .build()
            .await
            .unwrap();
        let state = runtime.state().clone();
        let base = format!("http://{}/api/v1", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let mut serving = tokio::spawn(runtime.run());
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(12))
            .build()
            .unwrap();
        let result = AssertUnwindSafe(tokio::time::timeout(
            Duration::from_secs(30),
            test(f, state, base, client),
        ))
        .catch_unwind()
        .await;
        stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        if joined.is_err() {
            serving.abort();
            let _ = serving.await;
        }
        joined
            .expect("auth HTTP runtime must stop")
            .unwrap()
            .unwrap();
        match result {
            Ok(Ok(())) => {}
            Ok(Err(_)) => panic!("auth HTTP case exceeded deadline"),
            Err(error) => std::panic::resume_unwind(error),
        }
    })
    .await;
}

fn spec() -> &'static Value {
    static SPEC: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    SPEC.get_or_init(|| serde_json::to_value(ApiDoc::openapi()).unwrap())
}

async fn wire(request: reqwest::RequestBuilder, expected: u16) -> Value {
    let metadata = request.try_clone().unwrap().build().unwrap();
    let path = metadata.url().path();
    let method = metadata.method().as_str().to_ascii_lowercase();
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let media = response.headers()["content-type"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body = response.text().await.unwrap();
    // Successful auth bodies contain credentials; log their shape, never their values.
    eprintln!("[auth-wire] {method} {path} status={status} content_type={media}");
    assert_eq!(
        status, expected,
        "unexpected HTTP status for {method} {path}"
    );
    let schema = &spec()["paths"][path][&method]["responses"][status.to_string()]["content"]
        [&media]["schema"];
    assert!(
        !schema.is_null(),
        "undeclared {method} {path} {status} {media}"
    );
    if media == "text/plain" {
        assert_eq!(schema["type"], "string");
        assert!(!body.is_empty());
        eprintln!("[auth-extractor] {body}");
        return Value::String(body);
    }
    assert_eq!(media, "application/json");
    let body: Value = serde_json::from_str(&body).unwrap();
    if status >= 400 {
        assert_eq!(schema["$ref"], "#/components/schemas/ErrorResponse");
        assert_eq!(body.as_object().unwrap().len(), 1);
        let error = body["error"].as_object().unwrap();
        assert!(error["message"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(
            spec()["components"]["schemas"]["ErrorCode"]["enum"]
                .as_array()
                .unwrap()
                .contains(&error["code"])
        );
        if let Some(reason) = error.get("reason") {
            assert!(
                spec()["components"]["schemas"]["ErrorReason"]["enum"]
                    .as_array()
                    .unwrap()
                    .contains(reason)
            );
        }
        assert_eq!(
            error.len(),
            if error.contains_key("reason") { 3 } else { 2 }
        );
        eprintln!("[auth-error] {body}");
    }
    body
}

fn error(body: &Value, code: &str, reason: Option<&str>) {
    assert_eq!(body["error"]["code"], code);
    assert_eq!(body["error"]["reason"].as_str(), reason);
}

fn registration(client: &reqwest::Client, base: &str, email: &str) -> reqwest::RequestBuilder {
    client.post(format!("{base}/auth/register")).json(&json!({
        "email": email, "name": "  Auth Contract  ", "password": PASSWORD
    }))
}

async fn register(client: &reqwest::Client, base: &str, email: &str) -> Value {
    let body = wire(registration(client, base, email), 200).await;
    assert!(!body["access_token"].as_str().unwrap().is_empty());
    assert!(!body["refresh_token"].as_str().unwrap().is_empty());
    assert_eq!(body["user"]["name"], "Auth Contract");
    assert_eq!(body["user"]["status"], "active");
    body
}

fn refresh(client: &reqwest::Client, base: &str, token: &str) -> reqwest::RequestBuilder {
    client
        .post(format!("{base}/auth/refresh"))
        .json(&json!({"refresh_token": token}))
}

async fn blocker_pid(blocker: &DatabaseTransaction) -> i32 {
    blocker
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT pg_backend_pid() AS pid",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap()
}

async fn wait_for_lock(admin: &DatabaseConnection, pid: i32, expected: i64) {
    let deadline = if expected == 1 { 1 } else { 10 };
    tokio::time::timeout(Duration::from_secs(deadline), async {
        loop {
            let waiting: i64 = admin.query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "WITH RECURSIVE blocked(pid) AS (SELECT pid FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) UNION SELECT a.pid FROM pg_stat_activity a JOIN blocked b ON b.pid=ANY(pg_blocking_pids(a.pid))) SELECT count(*) AS n FROM blocked",
                [pid.into()],
            )).await.unwrap().unwrap().try_get("", "n").unwrap();
            if waiting >= expected {
                eprintln!("[auth-lock] blocker_pid={pid} witnessed_waiters={waiting}");
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("HTTP requests must reach the PostgreSQL lock");
}

// Keep requests alive together until PostgreSQL proves both reached the barrier.
async fn race(
    f: &Fixture,
    blocker: DatabaseTransaction,
    requests: [reqwest::RequestBuilder; 2],
) -> [(u16, Value); 2] {
    let pid = blocker_pid(&blocker).await;
    let mut tasks = requests.map(|request| {
        tokio::spawn(async move {
            let response = request.send().await.unwrap();
            let status = response.status().as_u16();
            assert_eq!(response.headers()["content-type"], "application/json");
            (status, response.json::<Value>().await.unwrap())
        })
    });
    let witness = AssertUnwindSafe(wait_for_lock(&f.admin, pid, 2))
        .catch_unwind()
        .await;
    blocker.rollback().await.unwrap();
    let results = futures::future::join_all(tasks.iter_mut()).await;
    if let Err(error) = witness {
        std::panic::resume_unwind(error);
    }
    let mut results = results.into_iter().map(Result::unwrap);
    let results = [results.next().unwrap(), results.next().unwrap()];
    eprintln!("[auth-race] statuses={:?}", results.each_ref().map(|r| r.0));
    results
}

#[tokio::test]
async fn http_registration_normalizes_email_and_serializes_duplicate_writes() {
    with_http(|f, state, base, client| async move {
        let blocker = state.db.begin().await.unwrap();
        blocker
            .execute_unprepared("LOCK TABLE users IN SHARE MODE")
            .await
            .unwrap();
        let results = race(
            &f,
            blocker,
            [
                registration(&client, &base, " Same@Contract.test "),
                registration(&client, &base, "same@contract.test"),
            ],
        )
        .await;
        assert_eq!(results.iter().filter(|r| r.0 == 200).count(), 1);
        let success = &results.iter().find(|r| r.0 == 200).unwrap().1;
        assert_eq!(success["user"]["email"], "same@contract.test");
        assert_eq!(success["user"]["role"], "admin");
        let duplicate = results
            .iter()
            .find(|r| r.0 == 409)
            .expect("duplicate must return 409");
        error(
            &duplicate.1,
            "ALREADY_EXISTS",
            Some("email_already_registered"),
        );
        let repeated = wire(registration(&client, &base, "SAME@contract.test"), 409).await;
        error(
            &repeated,
            "ALREADY_EXISTS",
            Some("email_already_registered"),
        );
        assert_eq!(user::Entity::find().count(&state.db).await.unwrap(), 1);
        assert_eq!(
            user_identity_link::Entity::find()
                .count(&state.db)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            refresh_token::Entity::find()
                .count(&state.db)
                .await
                .unwrap(),
            1
        );
        let second = register(&client, &base, "member@contract.test").await;
        assert_eq!(second["user"]["role"], "user");
        let logged_in = wire(
            client
                .post(format!("{base}/auth/login"))
                .json(&json!({"email":" SAME@contract.test ", "password":PASSWORD})),
            200,
        )
        .await;
        assert_eq!(logged_in["user"]["id"], success["user"]["id"]);
    })
    .await;
}

#[tokio::test]
async fn http_auth_reasons_rotation_logout_and_inactive_accounts() {
    with_http(|f, state, base, client| async move {
        let account = register(&client, &base, "rotation@contract.test").await;
        let token = account["access_token"].as_str().unwrap();
        let refresh_token = account["refresh_token"].as_str().unwrap();
        for request in [client.get(format!("{base}/users/me")),
            client.get(format!("{base}/users/me")).header("authorization", "Basic invalid")] {
            error(&wire(request, 401).await, "AUTH_INVALID_CREDENTIALS", Some("token_missing"));
        }
        for invalid in ["not-a-jwt".to_owned(), "x".repeat(8193)] {
            error(&wire(client.get(format!("{base}/users/me")).bearer_auth(invalid), 401).await,
                "AUTH_INVALID_CREDENTIALS", Some("token_invalid"));
        }
        for (email, password) in [("rotation@contract.test", "wrong-password"),
            ("missing@contract.test", PASSWORD)] {
            error(&wire(client.post(format!("{base}/auth/login"))
                .json(&json!({"email":email,"password":password})), 401).await,
                "AUTH_INVALID_CREDENTIALS", Some("invalid_credentials"));
        }
        error(&wire(refresh(&client, &base, "unknown-refresh"), 401).await,
            "AUTH_INVALID_CREDENTIALS", Some("refresh_token_invalid"));

        let blocker = state.db.begin().await.unwrap();
        blocker.execute_unprepared("SELECT id FROM refresh_tokens FOR UPDATE").await.unwrap();
        let results = race(&f, blocker, [refresh(&client, &base, refresh_token),
            refresh(&client, &base, refresh_token)]).await;
        assert_eq!(results.iter().filter(|r| r.0 == 200).count(), 1);
        error(&results.iter().find(|r| r.0 == 401).unwrap().1,
            "AUTH_INVALID_CREDENTIALS", Some("refresh_token_invalid"));
        let successor = results.iter().find(|r| r.0 == 200).unwrap().1["refresh_token"]
            .as_str().unwrap();
        assert_ne!(successor, refresh_token);
        error(&wire(refresh(&client, &base, refresh_token), 401).await,
            "AUTH_INVALID_CREDENTIALS", Some("refresh_token_invalid"));
        let rotated = wire(refresh(&client, &base, successor), 200).await;
        let latest = rotated["refresh_token"].as_str().unwrap();
        for _ in 0..2 {
            assert_eq!(wire(client.post(format!("{base}/auth/logout"))
                .json(&json!({"refresh_token":latest})), 200).await, json!({"success":true}));
        }
        error(&wire(refresh(&client, &base, latest), 401).await,
            "AUTH_INVALID_CREDENTIALS", Some("refresh_token_invalid"));
        assert_eq!(refresh_token::Entity::find().filter(refresh_token::Column::RevokedAt.is_null())
            .count(&state.db).await.unwrap(), 0);

        let login = wire(client.post(format!("{base}/auth/login"))
            .json(&json!({"email":"rotation@contract.test","password":PASSWORD})), 200).await;
        state.db.execute_unprepared("UPDATE users SET status='disabled'").await.unwrap();
        error(&wire(client.get(format!("{base}/users/me")).bearer_auth(token), 401).await,
            "AUTH_INVALID_CREDENTIALS", Some("account_inactive"));
        error(&wire(refresh(&client, &base, login["refresh_token"].as_str().unwrap()), 401).await,
            "AUTH_INVALID_CREDENTIALS", Some("account_inactive"));
        error(&wire(client.post(format!("{base}/auth/login"))
            .json(&json!({"email":"rotation@contract.test","password":PASSWORD})), 401).await,
            "AUTH_INVALID_CREDENTIALS", Some("invalid_credentials"));
        assert_eq!(refresh_token::Entity::find().filter(refresh_token::Column::RevokedAt.is_null())
            .count(&state.db).await.unwrap(), 1, "inactive refresh must roll back consumption");
        state.db.execute_unprepared("UPDATE users SET status='active'; UPDATE refresh_tokens SET expires_at=now()-interval '1 second' WHERE revoked_at IS NULL").await.unwrap();
        error(&wire(refresh(&client, &base, login["refresh_token"].as_str().unwrap()), 401).await,
            "AUTH_INVALID_CREDENTIALS", Some("refresh_token_invalid"));
    }).await;
}

#[tokio::test]
async fn http_account_lookup_lock_timeout_is_503_and_recovers_after_unlock() {
    with_http(|f, state, base, client| async move {
        let account = register(&client, &base, "locked@contract.test").await;
        let token = account["access_token"].as_str().unwrap();
        let blocker = state.db.begin().await.unwrap();
        blocker
            .execute_unprepared("LOCK TABLE users IN ACCESS EXCLUSIVE MODE")
            .await
            .unwrap();
        let pid = blocker_pid(&blocker).await;
        let started = Instant::now();
        let outgoing = client.get(format!("{base}/users/me")).bearer_auth(token);
        let mut request = tokio::spawn(wire(outgoing, 503));
        let result = AssertUnwindSafe(async {
            wait_for_lock(&f.admin, pid, 1).await;
            let body = (&mut request).await.unwrap();
            error(
                &body,
                "SERVICE_UNAVAILABLE",
                Some("account_verification_unavailable"),
            );
            assert_eq!(body["error"]["message"], "account verification unavailable");
            assert!(started.elapsed() >= Duration::from_millis(1900));
            eprintln!(
                "[auth-lookup-timeout] elapsed_ms={}",
                started.elapsed().as_millis()
            );
        })
        .catch_unwind()
        .await;
        blocker.rollback().await.unwrap();
        if !request.is_finished() {
            request.abort();
            let _ = request.await;
        }
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
        let recovered = wire(
            client.get(format!("{base}/users/me")).bearer_auth(token),
            200,
        )
        .await;
        assert_eq!(recovered["id"], account["user"]["id"]);
    })
    .await;
}

#[tokio::test]
async fn http_json_extractors_and_patch_me_match_declared_wire_contract() {
    with_http(|_f, state, base, client| async move {
        let admin = register(&client, &base, "admin@contract.test").await;
        let member = register(&client, &base, "user@contract.test").await;
        let token = member["access_token"].as_str().unwrap();
        for path in [
            "/auth/login",
            "/auth/register",
            "/auth/refresh",
            "/auth/logout",
            "/users/me",
        ] {
            let method = if path == "/users/me" {
                reqwest::Method::PATCH
            } else {
                reqwest::Method::POST
            };
            for (body, media, expected) in [
                ("{".to_owned(), Some("application/json"), 400),
                ("{}".to_owned(), None, 415),
                ("{}".to_owned(), Some("text/plain"), 415),
                ("{\"name\":42}".to_owned(), Some("application/json"), 422),
                (
                    format!("{{\"padding\":\"{}\"}}", "x".repeat(2 * 1024 * 1024)),
                    Some("application/json"),
                    413,
                ),
            ] {
                let mut request = client
                    .request(method.clone(), format!("{base}{path}"))
                    .bearer_auth(token)
                    .body(body);
                if let Some(media) = media {
                    request = request.header("content-type", media);
                }
                assert!(wire(request, expected).await.is_string());
            }
        }
        let me = format!("{base}/users/me");
        let updated = wire(
            client
                .patch(&me)
                .bearer_auth(token)
                .json(&json!({"name":"  Updated Member  "})),
            200,
        )
        .await;
        assert_eq!(updated["name"], "Updated Member");
        assert_eq!(updated["id"], member["user"]["id"]);
        for body in [
            json!({"name":"must not persist", "role":"admin"}),
            json!({"name":"must not persist", "status":"disabled"}),
        ] {
            error(
                &wire(client.patch(&me).bearer_auth(token).json(&body), 403).await,
                "AUTH_FORBIDDEN",
                None,
            );
        }
        for name in [" ".to_owned(), "x".repeat(121)] {
            error(
                &wire(
                    client
                        .patch(&me)
                        .bearer_auth(token)
                        .json(&json!({"name":name})),
                    400,
                )
                .await,
                "VALIDATION_ERROR",
                None,
            );
        }
        let other = client
            .patch(format!(
                "{base}/users/{}",
                admin["user"]["id"].as_str().unwrap()
            ))
            .bearer_auth(token)
            .json(&json!({"name":"must not persist"}))
            .send()
            .await
            .unwrap();
        assert_eq!(other.status(), 403);
        error(
            &other.json::<Value>().await.unwrap(),
            "AUTH_FORBIDDEN",
            None,
        );
        let unchanged = wire(client.patch(&me).bearer_auth(token).json(&json!({})), 200).await;
        assert_eq!(unchanged["name"], "Updated Member");
        assert_eq!(unchanged["role"], "user");
        assert_eq!(unchanged["status"], "active");
        let saved = user::Entity::find_by_id(
            Uuid::parse_str(member["user"]["id"].as_str().unwrap()).unwrap(),
        )
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
        assert_eq!(saved.name, "Updated Member");
        assert_eq!(saved.role, "user");
        assert_eq!(saved.status, "active");
    })
    .await;
}
