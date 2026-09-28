//! Server runtime assembly, independent of process logging and signal handling.
use crate::config::ServerConfig;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

mod runtime;
mod setup;
pub use crate::module::lifecycle::{ShutdownHandle, ShutdownIncomplete};
pub(crate) use runtime::RuntimeWiring;
pub use runtime::ServerRuntime;
pub use setup::run_setup;

pub async fn build(
    config: Arc<ServerConfig>,
    shutdown: CancellationToken,
) -> anyhow::Result<ServerRuntime> {
    crate::module::ServerBuilder::from_config(config, shutdown)
        .build()
        .await
}

pub async fn run(config: Arc<ServerConfig>, shutdown: CancellationToken) -> anyhow::Result<()> {
    build(config, shutdown).await?.run().await
}
