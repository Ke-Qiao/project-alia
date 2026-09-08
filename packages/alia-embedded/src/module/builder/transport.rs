//! Tonic spawns connection tasks internally. Cancelling its listener future alone
//! does not terminate open HTTP/2 streams, so the node also owns each socket's I/O.
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::Result;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};
use tonic::transport::server::{Connected, TcpConnectInfo};

use super::super::lifecycle::ModuleLifecycle;

pub(super) struct Connections {
    cancel: CancellationToken,
    active: watch::Sender<usize>,
}

impl Connections {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            cancel: CancellationToken::new(),
            active: watch::channel(0).0,
        })
    }
    pub(super) fn close(&self) {
        self.cancel.cancel();
    }
    pub(super) fn wrap(self: &Arc<Self>, stream: TcpStream) -> Connection {
        self.active.send_modify(|active| *active += 1);
        Connection {
            stream,
            owner: self.clone(),
            read_cancel: Box::pin(self.cancel.clone().cancelled_owned()),
            write_cancel: Box::pin(self.cancel.clone().cancelled_owned()),
        }
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for Connections {
    fn name(&self) -> &'static str {
        "grpc_connections"
    }
    async fn stop(&self) -> Result<()> {
        self.close();
        let mut active = self.active.subscribe();
        loop {
            if *active.borrow_and_update() == 0 {
                return Ok(());
            }
            active.changed().await?;
        }
    }
}

pub(super) struct Connection {
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

fn closed() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::ConnectionAborted,
        "embedded runtime stopped",
    )
}
impl AsyncRead for Connection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
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
    ) -> Poll<std::io::Result<usize>> {
        if self.write_cancel.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(closed()));
        }
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.write_cancel.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(closed()));
        }
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.write_cancel.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
