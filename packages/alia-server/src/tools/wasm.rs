//! Static WASM descriptors enter the same Rig/Execution catalog as native tools.
use super::entry::tool_error;
use crate::providers::adapter::{ToolEntry, ToolRegistry};
use alia_plugin_api::ToolDescriptor;
use alia_plugin_runtime::Deployment;
use rig_core::tool::{DynamicTool, ToolOutput};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub fn entries(
    package_id: &str,
    descriptors: &[ToolDescriptor],
    deployment: Arc<Deployment>,
) -> anyhow::Result<Vec<Arc<ToolEntry>>> {
    anyhow::ensure!(descriptors.len() <= 128, "WASM tool count budget");
    anyhow::ensure!(
        !package_id.is_empty()
            && package_id.len() <= 48
            && package_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "WASM package id must be a provider-safe identifier"
    );
    let mut names = std::collections::HashSet::new();
    descriptors.iter().map(|descriptor| {
        anyhow::ensure!(!descriptor.name.is_empty() && descriptor.name.len() <= 64 && descriptor.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && names.insert(descriptor.name.clone()), "invalid or duplicate WASM tool name");
        deployment.require_export(&descriptor.export)?;
        let owner = deployment.clone(); let probe = deployment.clone(); let export = descriptor.export.clone();
        let full_name = format!("wasm_{package_id}_{}", descriptor.name);
        let name = if full_name.len() <= 64 { full_name } else { format!("{}_{}", &full_name[..47], &format!("{:x}", Sha256::digest(full_name.as_bytes()))[..16]) };
        let dynamic = DynamicTool::new(rig_core::message::ToolName::new(name)?, descriptor.description.clone(), descriptor.parameters.clone(), move |args| {
            let owner = owner.clone(); let export = export.clone();
            Box::pin(async move {
                let input = serde_json::to_vec(&args).map_err(tool_error)?;
                let bytes = owner.invoke(&export, &input, tokio::time::Instant::now() + std::time::Duration::from_secs(60), tokio_util::sync::CancellationToken::new()).await.map_err(tool_error)?;
                let output: serde_json::Value = serde_json::from_slice(&bytes).map_err(tool_error)?;
                Ok(ToolOutput::json(output))
            })
        }).with_liveness(move || probe.is_live());
        ToolEntry::dynamic(dynamic, format!("wasm:{package_id}"), deployment.implementation_pin(), descriptor.require_confirmation, descriptor.required_role.clone(), serde_json::json!({"package":package_id,"descriptor":descriptor,"implementation_pin":deployment.implementation_pin()}))
    }).collect()
}

pub async fn register(
    registry: &dyn ToolRegistry,
    package_id: &str,
    descriptors: &[ToolDescriptor],
    deployment: Arc<Deployment>,
) -> anyhow::Result<()> {
    registry
        .replace_source_tools(
            &format!("wasm:{package_id}"),
            entries(package_id, descriptors, deployment)?,
        )
        .await
}

pub async fn remove(registry: &dyn ToolRegistry, package_id: &str) -> anyhow::Result<()> {
    registry
        .replace_source_tools(&format!("wasm:{package_id}"), Vec::new())
        .await
}
