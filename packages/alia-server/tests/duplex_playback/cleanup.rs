//! Deterministic test-listener ownership checks; no database or hardware.
use super::*;
use std::net::SocketAddr;
use tokio::sync::oneshot;

struct ListenerOwner {
    listener: Option<tokio::net::TcpListener>,
    released: Option<oneshot::Sender<()>>,
}
impl Drop for ListenerOwner {
    fn drop(&mut self) {
        self.listener.take();
        if let Some(released) = self.released.take() {
            let _ = released.send(());
        }
    }
}

struct AbortOnDrop(tokio::task::AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn rpc_probe(
    panic_after_entry: bool,
) -> (RpcServer, SocketAddr, oneshot::Receiver<()>, AbortOnDrop) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (entered, entry) = oneshot::channel();
    let (released, release) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _listener = ListenerOwner {
            listener: Some(listener),
            released: Some(released),
        };
        entered.send(()).unwrap();
        if panic_after_entry {
            panic!("injected rpc task panic");
        }
        std::future::pending::<()>().await;
    });
    let fallback = AbortOnDrop(task.abort_handle());
    entry.await.unwrap();
    (
        RpcServer {
            stop: CancellationToken::new(),
            task: Some(task),
        },
        address,
        release,
        fallback,
    )
}

fn panic_text(error: &(dyn std::any::Any + Send)) -> &str {
    error
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| error.downcast_ref::<&'static str>().copied())
        .unwrap_or("non-string panic")
}

#[tokio::test(start_paused = true)]
async fn rpc_close_timeout_joins_task_before_reporting_failure() {
    let (server, address, mut released, fallback) = rpc_probe(false).await;
    let started = tokio::time::Instant::now();
    let result = AssertUnwindSafe(server.close()).catch_unwind().await;
    let elapsed = started.elapsed();
    let released_before_return = released.try_recv().is_ok();
    // The old implementation detaches the task; keep the RED run bounded too.
    if !released_before_return {
        fallback.0.abort();
        tokio::time::timeout(Duration::from_secs(1), &mut released)
            .await
            .unwrap()
            .unwrap();
    }
    let _rebound = tokio::net::TcpListener::bind(address).await.unwrap();
    assert_eq!(elapsed, Duration::from_secs(3));
    assert!(panic_text(result.unwrap_err().as_ref()).contains("Elapsed"));
    assert!(
        released_before_return,
        "RPC timeout returned before the task released its listener"
    );
}

#[tokio::test(start_paused = true)]
async fn cancelling_rpc_close_keeps_task_abort_owned() {
    let (server, address, mut released, fallback) = rpc_probe(false).await;
    let mut close = Box::pin(server.close());
    assert!(futures::poll!(close.as_mut()).is_pending());
    drop(close);
    let cancellation = tokio::time::timeout(Duration::from_secs(1), &mut released).await;
    let cancelled_without_fallback = matches!(cancellation, Ok(Ok(())));
    if cancellation.is_err() {
        fallback.0.abort();
        tokio::time::timeout(Duration::from_secs(1), &mut released)
            .await
            .unwrap()
            .unwrap();
    }
    let _rebound = tokio::net::TcpListener::bind(address).await.unwrap();
    assert!(
        cancelled_without_fallback,
        "cancelling RPC close detached the listener task"
    );
}

#[tokio::test(start_paused = true)]
async fn rpc_task_panic_is_reported_after_listener_release() {
    let (server, address, mut released, _fallback) = rpc_probe(true).await;
    let result = AssertUnwindSafe(server.close()).catch_unwind().await;
    assert!(released.try_recv().is_ok());
    let _rebound = tokio::net::TcpListener::bind(address).await.unwrap();
    assert!(panic_text(result.unwrap_err().as_ref()).contains("injected rpc task panic"));
}
