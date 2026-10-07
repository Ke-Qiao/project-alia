//! Server runtime assembly, independent of process logging and signal handling.
use crate::config::ServerConfig;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub mod builder;
pub mod factory;
pub mod registry;
mod runtime;
mod setup;
pub use crate::modules::platform::lifecycle::{ShutdownHandle, ShutdownIncomplete};
pub use builder::{ServerBuilder, ServerStartup};
pub use registry::ModuleRegistry;
pub(crate) use runtime::RuntimeWiring;
pub use runtime::ServerRuntime;
pub use setup::run_setup;

pub async fn build(
    config: Arc<ServerConfig>,
    shutdown: CancellationToken,
) -> anyhow::Result<ServerRuntime> {
    ServerBuilder::from_config(config, shutdown).build().await
}

pub async fn run(config: Arc<ServerConfig>, shutdown: CancellationToken) -> anyhow::Result<()> {
    build(config, shutdown).await?.run().await
}
