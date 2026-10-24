//! Installation grants and all catalog entries share one deployment identity.
use crate::{
    expression::{ExpressionRuntime, abi::WasmExpressionPlugin},
    providers::adapter::ToolRegistry,
};
use alia_plugin_runtime::{
    Deployment,
    package::{PackageManifest, PackageSnapshot, PackageStore},
};
use anyhow::{Result, ensure};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Mutex;

pub struct WasmPlugins {
    store: PackageStore,
    tools: Arc<dyn ToolRegistry>,
    expression: Arc<ExpressionRuntime>,
    active: Mutex<HashSet<String>>,
    closed: AtomicBool,
}

impl WasmPlugins {
    pub fn open(
        path: PathBuf,
        tools: Arc<dyn ToolRegistry>,
        expression: Arc<ExpressionRuntime>,
    ) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            store: PackageStore::open(path)?,
            tools,
            expression,
            active: Mutex::new(HashSet::new()),
            closed: AtomicBool::new(false),
        }))
    }
    pub async fn restore(&self) -> Result<()> {
        let mut active = self.active.lock().await;
        for snapshot in self.store.list().await {
            if snapshot.enabled {
                let (manifest, deployment) = self.store.get(&snapshot.manifest.name).await?;
                if let Err(error) = self.publish(&manifest, deployment).await {
                    self.store
                        .revoke(&manifest.name, &snapshot.implementation_pin)
                        .await?;
                    return Err(error);
                }
                active.insert(manifest.name);
            }
        }
        Ok(())
    }
    pub async fn list(&self) -> Vec<PackageSnapshot> {
        self.store.list().await
    }
    pub async fn install(
        &self,
        manifest: PackageManifest,
        module: Vec<u8>,
    ) -> Result<PackageSnapshot> {
        let _gate = self.active.lock().await;
        self.check_open()?;
        self.validate(&manifest, &module)?;
        self.store.install(manifest, module).await
    }
    pub async fn enable(&self, name: &str, pin: &str) -> Result<()> {
        let mut active = self.active.lock().await;
        self.check_open()?;
        let (manifest, current) = self.check_pin(name, pin).await?;
        if active.contains(name) {
            if current.is_live() {
                return Ok(());
            }
            // A failed durable revocation can leave closed entries in the catalog.
            self.remove(name).await?;
            active.remove(name);
        }
        let deployment = self.store.enable(name, pin).await?;
        if let Err(error) = self.publish(&manifest, deployment).await {
            self.store.revoke(name, pin).await?;
            return Err(error);
        }
        active.insert(name.to_owned());
        Ok(())
    }
    pub async fn revoke(&self, name: &str, pin: &str) -> Result<()> {
        let mut active = self.active.lock().await;
        self.check_open()?;
        self.check_pin(name, pin).await?;
        self.store.revoke(name, pin).await?;
        self.remove(name).await?;
        active.remove(name);
        Ok(())
    }
    pub async fn upgrade(
        &self,
        manifest: PackageManifest,
        module: Vec<u8>,
        pin: &str,
    ) -> Result<PackageSnapshot> {
        let mut active = self.active.lock().await;
        self.check_open()?;
        self.validate(&manifest, &module)?;
        self.check_pin(&manifest.name, pin).await?;
        self.store.revoke(&manifest.name, pin).await?;
        self.remove(&manifest.name).await?;
        active.remove(&manifest.name);
        self.store.upgrade(manifest, module, pin).await
    }
    pub async fn uninstall(&self, name: &str, pin: &str) -> Result<()> {
        let mut active = self.active.lock().await;
        self.check_open()?;
        self.check_pin(name, pin).await?;
        self.store.revoke(name, pin).await?;
        self.remove(name).await?;
        active.remove(name);
        self.store.uninstall(name, pin).await
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.close();
        let mut active = self.active.lock().await;
        self.store.shutdown().await?;
        let mut errors = Vec::new();
        for name in active.clone() {
            match self.remove(&name).await {
                Ok(()) => {
                    active.remove(&name);
                }
                Err(error) => errors.push(format!("{name}: {error}")),
            }
        }
        ensure!(
            errors.is_empty(),
            "WASM catalog cleanup incomplete: {}",
            errors.join("; ")
        );
        Ok(())
    }
    fn check_open(&self) -> Result<()> {
        ensure!(
            !self.closed.load(Ordering::SeqCst),
            "WASM package manager is stopping"
        );
        Ok(())
    }
    async fn check_pin(&self, name: &str, pin: &str) -> Result<(PackageManifest, Arc<Deployment>)> {
        let (manifest, deployment) = self.store.get(name).await?;
        ensure!(
            deployment.implementation_pin() == pin,
            "package identity changed; review its current pin"
        );
        Ok((manifest, deployment))
    }
    fn validate(&self, manifest: &PackageManifest, module: &[u8]) -> Result<()> {
        let deployment = Deployment::new(
            module.to_vec(),
            manifest.limits.clone(),
            &serde_json::to_value(manifest)?,
        )?;
        crate::tools::wasm::entries(&manifest.name, &manifest.tools, deployment.clone())?;
        if manifest.expression.is_some() {
            WasmExpressionPlugin::from_package(manifest, deployment)?;
        }
        Ok(())
    }
    async fn publish(&self, manifest: &PackageManifest, deployment: Arc<Deployment>) -> Result<()> {
        let entries =
            crate::tools::wasm::entries(&manifest.name, &manifest.tools, deployment.clone())?;
        let expression = manifest
            .expression
            .as_ref()
            .map(|_| WasmExpressionPlugin::from_package(manifest, deployment))
            .transpose()?;
        let registered_expression = expression.is_some();
        if let Some(plugin) = expression {
            self.expression.register_external_plugin(plugin)?;
        }
        if let Err(error) = self
            .tools
            .replace_source_tools(&format!("wasm:{}", manifest.name), entries)
            .await
        {
            if registered_expression {
                self.expression
                    .unregister_external_plugin(&manifest.name)
                    .await?;
            }
            return Err(error);
        }
        Ok(())
    }
    async fn remove(&self, name: &str) -> Result<()> {
        crate::tools::wasm::remove(self.tools.as_ref(), name).await?;
        let (manifest, _) = self.store.get(name).await?;
        if manifest.expression.is_some() {
            self.expression.unregister_external_plugin(name).await?;
        }
        Ok(())
    }
}
