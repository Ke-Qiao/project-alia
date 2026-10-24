//! MCP is a client transport into the one Execution catalog. Remote metadata
//! never confers local authority and protocol request IDs never replace effects.
#[cfg(feature = "tools-http")]
mod http;
mod stdio;

use crate::providers::adapter::{ToolContext, ToolEntry, ToolRegistry, ToolResult};
use crate::tools::entry::tool_error;
use futures::FutureExt;
use rig_core::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use rmcp::{
    ClientHandler, Peer, RoleClient,
    model::*,
    service::{
        ClientCacheConfig, ClientLifecycleMode, NotificationContext, PeerRequestOptions,
        RunningService, serve_client_with_lifecycle_and_ct,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(super) const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_TOOLS: usize = 128;
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    pub id: String,
    #[serde(default)]
    pub enabled: bool,
    /// The configured credential belongs to this Alia principal only.
    pub user_id: Uuid,
    #[serde(default = "confirmation_default")]
    pub require_confirmation: bool,
    #[serde(default)]
    pub required_role: Option<String>,
    #[serde(default)]
    pub legacy: bool,
    pub transport: McpTransportConfig,
}
fn confirmation_default() -> bool {
    true
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpTransportConfig {
    Stdio {
        program: PathBuf,
        executable_sha256: String,
        /// Explicit administrator grant: this process has host account powers.
        #[serde(default)]
        allow_host_execution: bool,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        #[serde(default)]
        token_env: Option<String>,
    },
}
#[derive(Debug, Clone, Serialize)]
pub struct McpStatus {
    pub id: String,
    pub state: String,
    pub epoch: Option<Uuid>,
    pub tools: usize,
    pub error: Option<String>,
}
struct Slot {
    config: McpServerConfig,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    status: Arc<std::sync::RwLock<McpStatus>>,
}
pub struct McpManager {
    registry: Arc<dyn ToolRegistry>,
    slots: Mutex<HashMap<String, Slot>>,
    closed: AtomicBool,
}
impl McpManager {
    pub fn new(
        registry: Arc<dyn ToolRegistry>,
        configs: &[McpServerConfig],
    ) -> anyhow::Result<Arc<Self>> {
        anyhow::ensure!(configs.len() <= 16, "too many configured MCP services");
        let mut slots = HashMap::new();
        for config in configs {
            anyhow::ensure!(
                !config.id.is_empty()
                    && config.id.len() <= 64
                    && config
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "invalid MCP service id"
            );
            anyhow::ensure!(
                !config.user_id.is_nil(),
                "MCP credential principal is required"
            );
            anyhow::ensure!(!slots.contains_key(&config.id), "duplicate MCP service id");
            let status = McpStatus {
                id: config.id.clone(),
                state: "stopped".into(),
                epoch: None,
                tools: 0,
                error: None,
            };
            slots.insert(
                config.id.clone(),
                Slot {
                    config: config.clone(),
                    stop: CancellationToken::new(),
                    task: None,
                    status: Arc::new(std::sync::RwLock::new(status)),
                },
            );
        }
        Ok(Arc::new(Self {
            registry,
            slots: Mutex::new(slots),
            closed: AtomicBool::new(false),
        }))
    }
    pub async fn start_configured(&self) -> anyhow::Result<()> {
        let ids = self
            .slots
            .lock()
            .await
            .values()
            .filter(|s| s.config.enabled)
            .map(|s| s.config.id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            self.restart(&id).await?;
        }
        Ok(())
    }
    pub async fn statuses(&self) -> Vec<McpStatus> {
        let mut statuses = self
            .slots
            .lock()
            .await
            .values()
            .map(|s| s.status.read().unwrap().clone())
            .collect::<Vec<_>>();
        statuses.sort_by(|a, b| a.id.cmp(&b.id));
        statuses
    }
    pub async fn restart(&self, id: &str) -> anyhow::Result<()> {
        let mut slots = self.slots.lock().await;
        anyhow::ensure!(
            !self.closed.load(Ordering::Acquire),
            "MCP manager is shut down"
        );
        let slot = slots
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("unknown MCP service"))?;
        stop_slot(slot).await?;
        anyhow::ensure!(
            !self.closed.load(Ordering::Acquire),
            "MCP manager is shut down"
        );
        slot.stop = CancellationToken::new();
        let (config, registry, stop, status) = (
            slot.config.clone(),
            self.registry.clone(),
            slot.stop.clone(),
            slot.status.clone(),
        );
        *status.write().unwrap() = McpStatus {
            id: id.into(),
            state: "connecting".into(),
            epoch: None,
            tools: 0,
            error: None,
        };
        slot.task = Some(tokio::spawn(async move {
            let outcome =
                std::panic::AssertUnwindSafe(run(config, registry, stop.clone(), status.clone()))
                    .catch_unwind()
                    .await;
            let result = outcome.unwrap_or_else(|_| Err(anyhow::anyhow!("MCP owner panicked")));
            stop.cancel();
            let mut snapshot = status.write().unwrap();
            snapshot.state = if result.is_ok() { "stopped" } else { "failed" }.into();
            snapshot.error = result
                .as_ref()
                .err()
                .map(|e| e.to_string().chars().take(512).collect());
            snapshot.tools = 0;
            result
        }));
        Ok(())
    }
    pub async fn stop(&self, id: &str) -> anyhow::Result<()> {
        let mut slots = self.slots.lock().await;
        stop_slot(
            slots
                .get_mut(id)
                .ok_or_else(|| anyhow::anyhow!("unknown MCP service"))?,
        )
        .await
    }
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.closed.store(true, Ordering::Release);
        let mut slots = self.slots.lock().await;
        for slot in slots.values() {
            slot.stop.cancel();
        }
        let mut error = None;
        for slot in slots.values_mut() {
            if let Err(e) = stop_slot(slot).await {
                error = Some(e);
            }
        }
        error.map_or(Ok(()), Err)
    }
}
async fn stop_slot(slot: &mut Slot) -> anyhow::Result<()> {
    slot.stop.cancel();
    if let Some(task) = &mut slot.task {
        slot.status.write().unwrap().state = "stopping".into();
        let result = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .map_err(|_| anyhow::anyhow!("MCP cleanup pending; original owner retained"))?;
        slot.task.take();
        // A completed owner has finished cleanup. Its previous connection error
        // remains in status but must not prevent an explicit fresh connection.
        match result {
            Ok(_) => slot.status.write().unwrap().state = "stopped".into(),
            Err(error) => {
                let mut status = slot.status.write().unwrap();
                status.state = "failed".into();
                status.error = Some(error.to_string().chars().take(512).collect());
                return Err(error.into());
            }
        }
    }
    Ok(())
}
#[derive(Clone)]
struct Handler {
    live: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    changed: Arc<Notify>,
}
impl ClientHandler for Handler {
    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.live.store(false, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.changed.notify_one();
    }
}
struct Connection {
    peer: Peer<RoleClient>,
    stop: CancellationToken,
    permits: Arc<Semaphore>,
    user_id: Uuid,
    epoch: Uuid,
    service_id: String,
}
impl Connection {
    async fn invoke(
        &self,
        name: String,
        args: serde_json::Value,
        output_validator: Option<Arc<crate::tools::entry::CompiledSchema>>,
        context: &mut rig_core::tool::ToolContext,
    ) -> Result<ToolOutput, ToolExecutionError> {
        let invocation = context.require::<ToolContext>().map_err(tool_error)?;
        if invocation.user_id != self.user_id {
            return Err(ToolExecutionError::invalid_args(
                "MCP credential belongs to another principal",
            ));
        }
        let _permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| ToolExecutionError::other("MCP service concurrency limit"))?;
        if self.stop.is_cancelled() {
            return Err(ToolExecutionError::other("MCP service stopping"));
        }
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(
            CallToolRequestParams::new(name).with_arguments(args.as_object().cloned().ok_or_else(
                || ToolExecutionError::invalid_args("MCP arguments must be an object"),
            )?),
        ));
        let sent = tokio::time::timeout(
            CALL_TIMEOUT,
            self.peer
                .send_cancellable_request(request, PeerRequestOptions::no_options()),
        )
        .await
        .map_err(|_| ToolExecutionError::other("MCP send deadline; outcome unknown"))
        .and_then(|result| result.map_err(tool_error));
        let mut handle = match sent {
            Ok(handle) => handle,
            Err(error) => {
                self.stop.cancel();
                return Err(error);
            }
        };
        let request_id = handle.id.to_string();
        let result = tokio::select! {
            response = tokio::time::timeout(CALL_TIMEOUT, &mut handle.rx) => match response { Ok(Ok(result)) => result.map_err(tool_error), _ => Err(ToolExecutionError::other("MCP response interrupted or deadline; outcome unknown")) },
            _ = self.stop.cancelled() => Err(ToolExecutionError::other("MCP service stopped after dispatch; outcome unknown")),
        };
        if result.is_err() {
            // Retain original handle until this bounded cancellation attempt ends.
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                handle.cancel(Some("Alia execution ended; effects are unverified".into())),
            )
            .await;
            // Cancellation is not proof that the remote operation stopped. Seal
            // this connection so timed-out handles cannot accumulate or acquire
            // fresh authority; an administrator may establish a new epoch.
            self.stop.cancel();
        }
        let result = result?;
        let ServerResult::CallToolResult(result) = result else {
            return Err(ToolExecutionError::other(
                "MCP returned MRTR/task instead of completed result; outcome unknown; request was not repeated",
            ));
        };
        if serde_json::to_vec(&result).map_err(tool_error)?.len() > MAX_MESSAGE_BYTES {
            return Err(ToolExecutionError::other(
                "MCP receipt budget; outcome unknown",
            ));
        }
        if let Some(validator) = output_validator {
            let structured = result.structured_content.as_ref().ok_or_else(|| {
                ToolExecutionError::other(
                    "MCP output schema requires structured content; outcome unknown",
                )
            })?;
            validator.validate(structured).map_err(|_| {
                ToolExecutionError::other("MCP output schema rejected receipt; outcome unknown")
            })?;
        }
        let output = super::mcp::result::presentation(&result)?;
        let receipt = ToolResult {
            presentation: Some(output),
            call_id: String::new(),
            success: !result.is_error.unwrap_or(false),
            output: serde_json::json!({"service_id":self.service_id,"connection_epoch":self.epoch,"protocol_request_id":request_id,"result":result}),
            display: None,
        };
        receipt.publish(context)
    }
}
async fn run(
    config: McpServerConfig,
    registry: Arc<dyn ToolRegistry>,
    stop: CancellationToken,
    status: Arc<std::sync::RwLock<McpStatus>>,
) -> anyhow::Result<()> {
    let epoch = Uuid::new_v4();
    status.write().unwrap().epoch = Some(epoch);
    let live = Arc::new(AtomicBool::new(false));
    let handler = Handler {
        live: live.clone(),
        generation: Arc::new(AtomicU64::new(0)),
        changed: Arc::new(Notify::new()),
    };
    let broken = CancellationToken::new();
    let lifecycle = if config.legacy {
        ClientLifecycleMode::Initialize
    } else {
        ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            legacy_version: Some(ProtocolVersion::V_2025_11_25),
        }
    };
    let mut child = None;
    let handshake = std::panic::AssertUnwindSafe(async {
        match &config.transport {
            McpTransportConfig::Stdio {
                program,
                executable_sha256,
                allow_host_execution,
                args,
                env,
            } => {
                anyhow::ensure!(
                    *allow_host_execution,
                    "MCP stdio requires an explicit allow_host_execution grant"
                );
                let (owner, transport) =
                    stdio::ChildOwner::spawn(program, executable_sha256, args, env, broken.clone())
                        .await?;
                child = Some(owner);
                tokio::time::timeout(
                    Duration::from_secs(20),
                    serve_client_with_lifecycle_and_ct(
                        handler.clone(),
                        transport,
                        lifecycle,
                        stop.child_token(),
                    ),
                )
                .await
                .map_err(anyhow::Error::from)
                .and_then(|r| r.map_err(anyhow::Error::from))
            }
            McpTransportConfig::Http { url, token_env } => {
                #[cfg(feature = "tools-http")]
                {
                    use rmcp::transport::{
                        StreamableHttpClientTransport, common::client_side_sse::ExponentialBackoff,
                        streamable_http_client::StreamableHttpClientTransportConfig,
                    };
                    let client = http::BoundedHttp::new(url)?;
                    let mut options = StreamableHttpClientTransportConfig::with_uri(url.clone())
                        .reinit_on_expired_session(false)
                        .max_concurrent_requests(4)
                        .max_sse_event_size(MAX_MESSAGE_BYTES);
                    options.channel_buffer_capacity = 16;
                    let mut retry = ExponentialBackoff::default();
                    retry.max_times = Some(0);
                    retry.max_delay = Some(Duration::from_secs(1));
                    options.retry_config = Arc::new(retry);
                    if let Some(env) = token_env {
                        options.auth_header = Some(std::env::var(env).map_err(|_| {
                            anyhow::anyhow!("configured MCP credential is unavailable")
                        })?);
                    }
                    let transport = StreamableHttpClientTransport::with_client(client, options);
                    tokio::time::timeout(
                        Duration::from_secs(20),
                        serve_client_with_lifecycle_and_ct(
                            handler.clone(),
                            transport,
                            lifecycle,
                            stop.child_token(),
                        ),
                    )
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(|r| r.map_err(anyhow::Error::from))
                }
                #[cfg(not(feature = "tools-http"))]
                {
                    let _ = (url, token_env);
                    Err(anyhow::anyhow!("MCP HTTP requires tools-http feature"))
                }
            }
        }
    })
    .catch_unwind()
    .await
    .unwrap_or_else(|_| Err(anyhow::anyhow!("MCP handshake panicked")));
    let mut service = match handshake {
        Ok(s) => s,
        Err(error) => {
            if let Some(child) = child {
                child.close().await?;
            }
            return Err(error);
        }
    };
    let connection = Arc::new(Connection {
        peer: service.peer().clone(),
        stop: stop.clone(),
        permits: Arc::new(Semaphore::new(4)),
        user_id: config.user_id,
        epoch,
        service_id: config.id.clone(),
    });
    let result = std::panic::AssertUnwindSafe(catalog_loop(
        &config,
        registry,
        &mut service,
        connection.clone(),
        handler,
        broken,
        status,
    ))
    .catch_unwind()
    .await
    .unwrap_or_else(|_| Err(anyhow::anyhow!("MCP catalog owner panicked")));
    live.store(false, Ordering::Release);
    stop.cancel();
    // Active calls hold their original request handles and finish cancellation
    // before the transport/process can be destroyed underneath them.
    let _all = connection.permits.acquire_many(4).await?;
    let closing = service.close().await;
    let child_result = if let Some(child) = child {
        child.close().await
    } else {
        Ok(())
    };
    closing.map_err(anyhow::Error::from)?;
    child_result?;
    result
}
async fn catalog_loop(
    config: &McpServerConfig,
    registry: Arc<dyn ToolRegistry>,
    service: &mut RunningService<RoleClient, Handler>,
    connection: Arc<Connection>,
    handler: Handler,
    broken: CancellationToken,
    status: Arc<std::sync::RwLock<McpStatus>>,
) -> anyhow::Result<()> {
    let peer = service.peer();
    peer.set_response_cache_config(ClientCacheConfig::disabled())
        .await;
    anyhow::ensure!(
        peer.peer_info()
            .is_some_and(|info| info.capabilities.tools.is_some()),
        "MCP service does not advertise tools"
    );
    let modern = peer
        .peer_info()
        .map(|info| info.protocol_version.clone())
        .is_some_and(|v| v >= ProtocolVersion::V_2026_07_28);
    let mut subscription = if modern {
        Some(
            tokio::time::timeout(
                Duration::from_secs(10),
                peer.listen(SubscriptionFilter::builder().tools_list_changed().build()),
            )
            .await??,
        )
    } else {
        None
    };
    let mut poll = tokio::time::interval(Duration::from_secs(1));
    loop {
        handler.live.store(false, Ordering::Release);
        let entries = tokio::time::timeout(
            Duration::from_secs(10),
            discover(
                config,
                connection.clone(),
                handler.live.clone(),
                handler.generation.clone(),
            ),
        )
        .await??;
        let count = entries.len();
        registry
            .replace_source_tools(&format!("mcp:{}", config.id), entries)
            .await?;
        handler.live.store(true, Ordering::Release);
        {
            let mut state = status.write().unwrap();
            state.state = "ready".into();
            state.tools = count;
            state.error = None;
        }
        loop {
            tokio::select! {
                _ = connection.stop.cancelled() => return Ok(()),
                _ = broken.cancelled() => anyhow::bail!("MCP stdio closed or exceeded budget"),
                _ = handler.changed.notified() => break,
                notification = async { match &mut subscription { Some(s) => s.next().await, None => std::future::pending().await } } => {
                    match notification? { Some(_) => { handler.live.store(false, Ordering::Release); handler.generation.fetch_add(1, Ordering::AcqRel); break; }, None => anyhow::bail!("MCP catalog subscription ended") }
                }
                _ = poll.tick() => if peer.is_transport_closed() { anyhow::bail!("MCP connection closed"); },
            }
        }
    }
}
async fn discover(
    config: &McpServerConfig,
    connection: Arc<Connection>,
    live: Arc<AtomicBool>,
    catalog_generation: Arc<AtomicU64>,
) -> anyhow::Result<Vec<Arc<ToolEntry>>> {
    let observed_generation = catalog_generation.load(Ordering::Acquire);
    let mut cursor = None;
    let mut cursors = HashSet::new();
    let mut names = HashSet::new();
    let mut tools = Vec::new();
    let mut bytes = 0usize;
    for _ in 0..16 {
        let page = connection
            .peer
            .list_tools(
                cursor
                    .take()
                    .map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor))),
            )
            .await?;
        bytes += serde_json::to_vec(&page)?.len();
        anyhow::ensure!(
            bytes <= MAX_MESSAGE_BYTES && tools.len() + page.tools.len() <= MAX_TOOLS,
            "MCP catalog budget exceeded"
        );
        for tool in page.tools {
            anyhow::ensure!(
                names.insert(tool.name.to_string()),
                "duplicate MCP tool name"
            );
            tools.push(tool);
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        anyhow::ensure!(
            cursors.insert(cursor.clone()),
            "MCP catalog cursor repeated"
        );
    }
    anyhow::ensure!(cursor.is_none(), "MCP catalog page budget exceeded");
    let generation = Uuid::new_v4();
    let config_digest = format!("{:x}", Sha256::digest(serde_json::to_vec(config)?));
    tools.into_iter().map(|tool| {
        let remote_name = tool.name.to_string();
        let alias = format!("mcp_{}_{}", &config.id[..config.id.len().min(32)], &format!("{:x}", Sha256::digest(format!("{}\0{}", config.id, remote_name).as_bytes()))[..16]);
        let identity = serde_json::json!({"service_id":config.id,"principal":config.user_id,"config_digest":config_digest,"connection_epoch":connection.epoch,"catalog_revision":generation,"protocol_version":connection.peer.peer_info().map(|info| info.protocol_version.clone()),"tool":tool});
        let output = tool.output_schema.as_ref().map(|schema| crate::tools::entry::compile_schema(&serde_json::to_value(schema)?)).transpose()?.map(Arc::new);
        let owner = connection.clone(); let live_probe = live.clone(); let probe_owner = owner.clone(); let catalog_generation = catalog_generation.clone();
        let dynamic = DynamicTool::new_with_context(rig_core::message::ToolName::new(alias)?, tool.description.clone().unwrap_or_default().to_string(), serde_json::to_value(&tool.input_schema)?, move |context, args| {
            let owner = owner.clone(); let name = remote_name.clone(); let output = output.clone();
            Box::pin(async move { owner.invoke(name, args, output, context).await })
        }).with_liveness(move || catalog_generation.load(Ordering::Acquire) == observed_generation && live_probe.load(Ordering::Acquire) && !probe_owner.stop.is_cancelled() && !probe_owner.peer.is_transport_closed());
        ToolEntry::dynamic(dynamic, format!("mcp:{}", config.id), generation.to_string(), config.require_confirmation, config.required_role.clone(), identity).map(|entry| entry.for_user(config.user_id))
    }).collect()
}
mod result;

#[cfg(all(test, feature = "tools-http"))]
mod tests;
