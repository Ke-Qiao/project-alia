use super::*;
use alia_plugin_api::{ProcessorRequest, ProcessorResponse};

pub(super) struct WasmOwner {
    pub(super) plugins: Arc<RwLock<HashMap<String, PluginEntry>>>,
    pub(super) events: broadcast::Sender<PluginEventRecord>,
    pub(super) name: String,
    pub(super) instance: String,
    pub(super) stop: CancellationToken,
    pub(super) deployment: Arc<Deployment>,
}
impl WasmOwner {
    pub(super) async fn run(self, mut commands: mpsc::Receiver<Vec<u8>>) {
        loop {
            let envelope = tokio::select! {
                biased;
                _ = self.stop.cancelled() => break,
                command = commands.recv() => match command { Some(command) => command, None => break },
            };
            let Ok(request) = serde_json::from_slice::<ProcessorRequest>(&envelope) else {
                break;
            };
            let output = self
                .deployment
                .invoke(
                    "alia_processor",
                    &envelope,
                    tokio::time::Instant::now() + std::time::Duration::from_secs(30),
                    self.stop.clone(),
                )
                .await
                .and_then(|bytes| Ok(serde_json::from_slice::<ProcessorResponse>(&bytes)?));
            let mut plugins = self.plugins.write().await;
            let Some(entry) = plugins.get_mut(&self.name) else {
                break;
            };
            if !entry
                .run
                .as_ref()
                .is_some_and(|run| run.instance_id == self.instance)
            {
                break;
            }
            if let Some(record) = entry.commands.get_mut(&request.command_id) {
                // This runtime exposes no external-effect host functions. A trap
                // cannot conceal device/network writes, but it yields no success.
                let (state, result) = match output {
                    Ok(response) => (
                        if response.success {
                            "completed"
                        } else {
                            "failed"
                        },
                        response.result,
                    ),
                    Err(_) => (
                        "failed",
                        serde_json::json!({"error":"processor failed or exceeded its budget"}),
                    ),
                };
                record.receipt.state = state.into();
                record.receipt.result_json = serde_json::to_vec(&result).expect("JSON serializes");
                emit(
                    &self.events,
                    &self.name,
                    &self.instance,
                    &request.command_id,
                    "command_result",
                    serde_json::json!({"instance_id":self.instance,"command_id":request.command_id,"state":state,"result":result}),
                );
            }
        }
        // The original owner remains installed until every blocking engine has exited.
        let _ = self.deployment.shutdown().await;
        let mut plugins = self.plugins.write().await;
        if let Some(entry) = plugins.get_mut(&self.name) {
            if entry
                .run
                .as_ref()
                .is_some_and(|run| run.instance_id == self.instance)
            {
                for record in entry.commands.values_mut() {
                    if record.receipt.instance_id == self.instance
                        && record.receipt.state == "unknown"
                    {
                        record.receipt.state = "cancelled".into();
                    }
                }
                entry.run = None;
                entry.runtime_capabilities.clear();
                entry.state = PluginState::Stopped;
                emit(
                    &self.events,
                    &self.name,
                    &self.instance,
                    "",
                    "lifecycle",
                    serde_json::json!({"state":"stopped"}),
                );
            }
        }
    }
}
