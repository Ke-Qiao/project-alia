//! Own sockets when the protocol server spawns its own connection tasks.
//! Closing a listener alone does not close an accepted HTTP/2 stream.
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};
use tonic::transport::server::{Connected, TcpConnectInfo};

pub struct Connections {
    accepting: Mutex<bool>,
    cancel: CancellationToken,
    active: watch::Sender<usize>,
}

impl Connections {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            accepting: Mutex::new(true),
            cancel: CancellationToken::new(),
            active: watch::channel(0).0,
        })
    }

    /// Seal admission and wake pending I/O. A concurrent wrap either registers
    /// before this boundary or returns an error.
    pub fn close(&self) {
        let mut accepting = self.accepting.lock().unwrap_or_else(|e| e.into_inner());
        *accepting = false;
        self.cancel.cancel();
    }

    pub fn active_count(&self) -> usize {
        *self.active.borrow()
    }

    pub fn wrap(self: &Arc<Self>, stream: TcpStream) -> io::Result<Connection> {
        let accepting = self.accepting.lock().unwrap_or_else(|e| e.into_inner());
        if !*accepting {
            return Err(closed());
        }
        self.active.send_modify(|active| *active += 1);
        Ok(Connection {
            stream,
            owner: self.clone(),
            read_cancel: Box::pin(self.cancel.clone().cancelled_owned()),
            write_cancel: Box::pin(self.cancel.clone().cancelled_owned()),
        })
    }

    /// Cancelling this wait retains the count. Success means all registered
    /// socket values have dropped, with no later admission possible.
    pub async fn wait_closed(&self) -> io::Result<()> {
        self.close();
        let mut active = self.active.subscribe();
        loop {
            if *active.borrow_and_update() == 0 {
                return Ok(());
            }
            active.changed().await.map_err(io::Error::other)?;
        }
    }
}

pub struct Connection {
    stream: TcpStream,
    owner: Arc<Connections>,
    read_cancel: Pin<Box<WaitForCancellationFutureOwned>>,
    write_cancel: Pin<Box<WaitForCancellationFutureOwned>>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.owner.active.send_modify(|active| *active -= 1);
    }
}

impl Connected for Connection {
    type ConnectInfo = TcpConnectInfo;
    fn connect_info(&self) -> Self::ConnectInfo {
        self.stream.connect_info()
    }
}

fn closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "server connections closed",
    )
}

impl AsyncRead for Connection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.read_cancel.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(closed()));
        }
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.write_cancel.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(closed()));
        }
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.write_cancel.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(closed()));
        }
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.write_cancel.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    async fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (peer, accepted) = tokio::join!(
            TcpStream::connect(listener.local_addr().unwrap()),
            listener.accept(),
        );
        (peer.unwrap(), accepted.unwrap().0)
    }

    #[tokio::test]
    async fn close_wakes_idle_read_and_waits_for_the_actual_socket_drop() {
        let (_peer, stream) = pair().await;
        let owner = Connections::new();
        let mut connection = owner.wrap(stream).unwrap();
        let read = tokio::spawn(async move {
            let result = connection.read_u8().await;
            (connection, result)
        });
        tokio::task::yield_now().await;
        owner.close();
        let (connection, result) = tokio::time::timeout(std::time::Duration::from_secs(1), read)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(owner.active_count(), 1);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), owner.wait_closed())
                .await
                .is_err()
        );
        drop(connection);
        owner.wait_closed().await.unwrap();
        assert_eq!(owner.active_count(), 0);
    }

    #[tokio::test]
    async fn sealed_owner_rejects_new_connections_even_after_drain() {
        let (_peer, stream) = pair().await;
        let owner = Connections::new();
        owner.wait_closed().await.unwrap();
        assert!(
            matches!(owner.wrap(stream), Err(e) if e.kind() == io::ErrorKind::ConnectionAborted)
        );
        assert_eq!(owner.active_count(), 0);
    }
}
