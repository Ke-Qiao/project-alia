use anyhow::Result;

/// Health status of a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleHealth {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Shared module lifecycle trait.
///
/// All modules (both pluggable slots and core infrastructure) implement this
/// to get unified init → start → stop lifecycle management.
#[tonic::async_trait]
pub trait ModuleLifecycle: Send + Sync {
    /// Human-readable name of this module (used in logs and health reports).
    fn name(&self) -> &'static str;

    /// Initialize the module with shared context. Called once before `start`.
    async fn init(&mut self, ctx: &super::context::ModuleContext) -> Result<()> {
        let _ = ctx;
        Ok(())
    }

    /// Start the module. Called after all modules have been initialized.
    async fn start(&self) -> Result<()> {
        Ok(())
    }

    /// Stop the module gracefully.
    async fn stop(&self) -> Result<()> {
        Ok(())
    }

    /// Report current health status.
    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}
