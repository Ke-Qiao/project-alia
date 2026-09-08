//! Adapt shared socket ownership to the device's module cleanup stack.
use std::sync::Arc;

use alia_transport::connections::{Connection, Connections as SocketConnections};
use tokio::net::TcpStream;

use super::super::lifecycle::ModuleLifecycle;

pub(super) struct Connections(Arc<SocketConnections>);

impl Connections {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self(SocketConnections::new()))
    }

    pub(super) fn close(&self) {
        self.0.close();
    }

    pub(super) fn wrap(&self, stream: TcpStream) -> std::io::Result<Connection> {
        self.0.wrap(stream)
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for Connections {
    fn name(&self) -> &'static str {
        "grpc_connections"
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(self.0.wait_closed().await?)
    }
}
