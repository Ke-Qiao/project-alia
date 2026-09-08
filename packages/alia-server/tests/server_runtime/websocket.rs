use super::*;
use alia_server::event::ServerEvent;
use alia_server::state::AppState;
use sea_orm::{DatabaseBackend, Statement};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn user(state: &AppState, role: &str, status: &str) -> (Uuid, String) {
    let id = Uuid::new_v4();
    state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Diagnostics test','not-a-password',$3,$4)",
        vec![id.into(), format!("{id}@test.invalid").into(), role.into(), status.into()],
    )).await.unwrap();
    // An admin claim must not override the current database role or status.
    let token = state
        .jwt_keys
        .encode(id, "test@test.invalid", "Test", "admin", 1)
        .unwrap();
    (id, token)
}

async fn handshake(address: std::net::SocketAddr, token: &str) -> (TcpStream, String) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "GET /api/v1/ws?token={token} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut header = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !header.ends_with(b"\r\n\r\n") {
            assert!(header.len() < 8192, "oversized handshake response");
            header.push(stream.read_u8().await.unwrap());
        }
    })
    .await
    .unwrap();
    (stream, String::from_utf8(header).unwrap())
}

async fn actors_started(state: &AppState, count: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while state.runtime_tasks.active() < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn diagnostics_requires_active_database_admin_and_shutdown_joins_upgraded_socket() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let address = runtime.http_addr();
        let state = runtime.state().clone();
        let stop = state.cancel_token.clone();
        let _stop_on_drop = stop.clone().drop_guard();
        let handle = runtime.shutdown_handle();
        let (_, admin) = user(&state, "admin", "active").await;
        let (_, ordinary) = user(&state, "user", "active").await;
        let (_, disabled) = user(&state, "admin", "disabled").await;
        let serving = tokio::spawn(runtime.run());
        assert!(
            handshake(address, &ordinary)
                .await
                .1
                .starts_with("HTTP/1.1 403")
        );
        assert!(
            handshake(address, &disabled)
                .await
                .1
                .starts_with("HTTP/1.1 401")
        );
        assert!(
            handshake(address, "invalid")
                .await
                .1
                .starts_with("HTTP/1.1 401")
        );
        let (mut socket, response) = handshake(address, &admin).await;
        assert!(response.starts_with("HTTP/1.1 101"), "{response}");
        actors_started(&state, 1).await;
        // A peer that does not consume events may leave a write in progress.
        state.event_bus.publish(ServerEvent::PluginEvent {
            plugin_name: "diagnostics-test".into(),
            event_type: "large-event".into(),
            data: serde_json::json!({"payload": "x".repeat(2 * 1024 * 1024)}),
        });
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        handle.wait().await.unwrap();
        assert_eq!(state.runtime_tasks.active(), 0);
        let mut remaining = Vec::new();
        // The kernel may already contain a prefix; neither EOF nor reset may hang.
        let _ = tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut remaining))
            .await
            .unwrap();
        assert!(
            state
                .runtime_tasks
                .try_spawn("late_websocket", async { Ok(()) })
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn open_diagnostics_streams_recheck_account_status_and_role() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let address = runtime.http_addr();
        let state = runtime.state().clone();
        let stop = state.cancel_token.clone();
        let _stop_on_drop = stop.clone().drop_guard();
        let (disabled_id, disabled_token) = user(&state, "admin", "active").await;
        let (demoted_id, demoted_token) = user(&state, "admin", "active").await;
        let serving = tokio::spawn(runtime.run());
        let (mut disabled, response) = handshake(address, &disabled_token).await;
        assert!(response.starts_with("HTTP/1.1 101"));
        let (mut demoted, response) = handshake(address, &demoted_token).await;
        assert!(response.starts_with("HTTP/1.1 101"));
        actors_started(&state, 2).await;
        state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE users SET status=CASE WHEN id=$1 THEN 'disabled' ELSE status END,role=CASE WHEN id=$2 THEN 'user' ELSE role END WHERE id IN($1,$2)",
            vec![disabled_id.into(), demoted_id.into()],
        )).await.unwrap();
        tokio::time::timeout(Duration::from_secs(35), async {
            let (left, right) = tokio::join!(disabled.read_u8(), demoted.read_u8());
            assert!(left.is_err(), "disabled user still received data");
            assert!(right.is_err(), "demoted user still received data");
        }).await.unwrap();
        stop.cancel();
        serving.await.unwrap().unwrap();
        assert_eq!(state.runtime_tasks.active(), 0);
    }).await;
}
