//! Authentication transactions are exercised against real PostgreSQL locks and failures.
use super::*;
use alia_server::api::{auth, users};
use alia_server::auth::AuthUser;
use alia_server::entity::{refresh_token, user, user_identity_link};
use alia_server::error::AppError;
use alia_server::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use sea_orm::{
    ColumnTrait, DatabaseBackend, EntityTrait, PaginatorTrait, QueryFilter, Statement,
    TransactionTrait,
};

async fn register(state: &AppState, email: &str) -> Result<auth::AuthResponse, AppError> {
    auth::register(
        State(state.clone()),
        HeaderMap::new(),
        Json(auth::RegisterRequest {
            email: email.into(),
            name: "Authentication test".into(),
            password: "test-password".into(),
        }),
    )
    .await
    .map(|Json(response)| response)
}

async fn refresh(state: &AppState, token: &str) -> Result<auth::TokenResponse, AppError> {
    auth::refresh(
        State(state.clone()),
        HeaderMap::new(),
        Json(auth::RefreshRequest {
            refresh_token: token.into(),
        }),
    )
    .await
    .map(|Json(response)| response)
}

async fn wait_for_two_lock_waiters(
    admin: &DatabaseConnection,
    blocker: &sea_orm::DatabaseTransaction,
) {
    let pid: i32 = blocker
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT pg_backend_pid() AS pid",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: i64 = admin.query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "WITH RECURSIVE blocked(pid) AS (SELECT pid FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) UNION SELECT a.pid FROM pg_stat_activity a JOIN blocked b ON b.pid=ANY(pg_blocking_pids(a.pid))) SELECT count(*) AS n FROM blocked",
                [pid.into()],
            )).await.unwrap().unwrap().try_get("", "n").unwrap();
            if waiting == 2 { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("both authentication requests must reach the database barrier");
}

#[tokio::test]
async fn concurrent_first_registrations_elect_one_admin_and_allow_later_promotion() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state();
        let blocker = state.db.begin().await.unwrap();
        blocker
            .execute_unprepared("LOCK TABLE users IN SHARE MODE")
            .await
            .unwrap();
        let requests = ["first@auth.test", "second@auth.test"].map(|email| {
            let state = state.clone();
            tokio::spawn(async move { register(&state, email).await })
        });
        wait_for_two_lock_waiters(&f.admin, &blocker).await;
        blocker.rollback().await.unwrap();
        let [first, second] = requests;
        let accounts = [
            first.await.unwrap().unwrap(),
            second.await.unwrap().unwrap(),
        ];
        assert_eq!(
            accounts
                .iter()
                .filter(|account| account.user.role == "admin")
                .count(),
            1
        );
        assert_eq!(user::Entity::find().count(&state.db).await.unwrap(), 2);
        assert_eq!(
            user_identity_link::Entity::find()
                .count(&state.db)
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            refresh_token::Entity::find()
                .count(&state.db)
                .await
                .unwrap(),
            2
        );

        let admin = accounts
            .iter()
            .find(|account| account.user.role == "admin")
            .unwrap();
        let ordinary = accounts
            .iter()
            .find(|account| account.user.role == "user")
            .unwrap();
        let Json(promoted) = users::update_user(
            State(state.clone()),
            AuthUser::from_token(state, &admin.access_token)
                .await
                .unwrap(),
            Path(Uuid::parse_str(&ordinary.user.id).unwrap()),
            Json(users::UpdateUserRequest {
                name: None,
                role: Some("admin".into()),
                status: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(promoted.role, "admin");
        assert_eq!(
            user::Entity::find()
                .filter(user::Column::Role.eq("admin"))
                .count(&state.db)
                .await
                .unwrap(),
            2
        );

        let requests = (0..2).map(|_| register(state, "duplicate@auth.test"));
        let results = futures::future::join_all(requests).await;
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(AppError::Validation(_))))
                .count(),
            1
        );
        runtime.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn registration_rolls_back_user_identity_and_token_on_insert_failure() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state();
        state.db.execute_unprepared(
            "CREATE FUNCTION fail_auth_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected auth insert failure'; END $$",
        ).await.unwrap();
        for table in ["user_identity_links", "refresh_tokens"] {
            state.db.execute_unprepared(&format!(
                "CREATE TRIGGER fail_auth_insert BEFORE INSERT ON {table} FOR EACH ROW EXECUTE FUNCTION fail_auth_insert()",
            )).await.unwrap();
            let error = register(state, "rollback@auth.test").await.err().expect("injected insert must fail registration");
            assert!(matches!(error, AppError::Database(_)), "{error:?}");
            assert_eq!(user::Entity::find().count(&state.db).await.unwrap(), 0);
            assert_eq!(user_identity_link::Entity::find().count(&state.db).await.unwrap(), 0);
            assert_eq!(refresh_token::Entity::find().count(&state.db).await.unwrap(), 0);
            state.db.execute_unprepared(&format!("DROP TRIGGER fail_auth_insert ON {table}")).await.unwrap();
        }
        assert_eq!(register(state, "rollback@auth.test").await.unwrap().user.role, "admin");
        runtime.shutdown().await.unwrap();
    }).await;
}

#[tokio::test]
async fn concurrent_refresh_consumes_token_once_and_successor_remains_usable() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state();
        let account = register(state, "rotation@auth.test").await.unwrap();
        let blocker = state.db.begin().await.unwrap();
        blocker
            .execute_unprepared("SELECT id FROM refresh_tokens FOR UPDATE")
            .await
            .unwrap();
        let requests = [0, 1].map(|_| {
            let state = state.clone();
            let token = account.refresh_token.clone();
            tokio::spawn(async move { refresh(&state, &token).await })
        });
        wait_for_two_lock_waiters(&f.admin, &blocker).await;
        blocker.rollback().await.unwrap();
        let [first, second] = requests;
        let results = [first.await.unwrap(), second.await.unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(AppError::Auth(_))))
                .count(),
            1
        );
        assert_eq!(
            refresh_token::Entity::find()
                .count(&state.db)
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            refresh_token::Entity::find()
                .filter(refresh_token::Column::RevokedAt.is_null())
                .count(&state.db)
                .await
                .unwrap(),
            1
        );
        assert!(matches!(
            refresh(state, &account.refresh_token).await,
            Err(AppError::Auth(_))
        ));
        let successor = results.into_iter().find_map(Result::ok).unwrap();
        let claims = state.jwt_keys.decode(&successor.access_token).unwrap();
        assert_eq!(claims.sub.to_string(), account.user.id);
        refresh(state, &successor.refresh_token).await.unwrap();
        runtime.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn refresh_failure_preserves_original_token_and_invalid_tokens_create_no_successor() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state();
        let account = register(state, "retry@auth.test").await.unwrap();
        state.db.execute_unprepared(
            "CREATE FUNCTION fail_refresh_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected refresh insert failure'; END $$; CREATE TRIGGER fail_refresh_insert BEFORE INSERT ON refresh_tokens FOR EACH ROW EXECUTE FUNCTION fail_refresh_insert()",
        ).await.unwrap();
        assert!(matches!(refresh(state, &account.refresh_token).await, Err(AppError::Database(_))));
        assert_eq!(refresh_token::Entity::find().count(&state.db).await.unwrap(), 1);
        assert!(refresh_token::Entity::find().one(&state.db).await.unwrap().unwrap().revoked_at.is_none());
        state.db.execute_unprepared("DROP TRIGGER fail_refresh_insert ON refresh_tokens").await.unwrap();
        state.db.execute_unprepared("UPDATE users SET status='disabled'").await.unwrap();
        assert!(matches!(refresh(state, &account.refresh_token).await, Err(AppError::Auth(_))));
        assert!(refresh_token::Entity::find().one(&state.db).await.unwrap().unwrap().revoked_at.is_none());
        state.db.execute_unprepared("UPDATE users SET status='active'").await.unwrap();
        let successor = refresh(state, &account.refresh_token).await.unwrap();
        state.db.execute_unprepared("UPDATE refresh_tokens SET expires_at=now() - interval '1 second' WHERE revoked_at IS NULL").await.unwrap();
        assert!(matches!(refresh(state, &successor.refresh_token).await, Err(AppError::Auth(_))));
        assert!(matches!(refresh(state, "unknown-token").await, Err(AppError::Auth(_))));
        assert_eq!(refresh_token::Entity::find().count(&state.db).await.unwrap(), 2);
        runtime.shutdown().await.unwrap();
    }).await;
}
