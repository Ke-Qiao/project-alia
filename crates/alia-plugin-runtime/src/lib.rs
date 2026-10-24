//! Owned Extism invocations. Each call has its own Engine, Store and guest memory.
//! No WASI, outbound HTTP, filesystem, environment or application host functions.
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::{Semaphore, oneshot};
use tokio::time::Instant;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub use alia_plugin_api::Limits;
pub mod package;

pub const MAX_MODULE_BYTES: usize = 16 * 1024 * 1024;
const RUNTIME_ID: &str = "alia-wasm-v1/extism-1.41.0-error-cap1/wasmtime-49";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Admission {
    Installed,
    Enabled,
    Revoked,
}

pub struct Deployment {
    module: Arc<[u8]>,
    exports: BTreeSet<String>,
    limits: Limits,
    pin: String,
    admission: Mutex<Admission>,
    stop: CancellationToken,
    tasks: TaskTracker,
    // One compilation/execution at a time per deployment. Independent Engines
    // provide isolation; this permit only bounds per-deployment resource use.
    permit: Arc<Semaphore>,
    outstanding: Arc<Semaphore>,
}

impl Deployment {
    pub fn new(module: Vec<u8>, limits: Limits, identity: &serde_json::Value) -> Result<Arc<Self>> {
        validate_limits(&limits)?;
        let exports = validated_exports(&module, &limits)?;
        let mut hash = Sha256::new();
        for bytes in [
            RUNTIME_ID.as_bytes(),
            host_digest()?.as_bytes(),
            module.as_slice(),
            &serde_json::to_vec(&limits)?,
            &serde_json::to_vec(identity)?,
        ] {
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        Ok(Arc::new(Self {
            module: module.into(),
            exports,
            limits,
            pin: format!("{:x}", hash.finalize()),
            admission: Mutex::new(Admission::Installed),
            stop: CancellationToken::new(),
            tasks: TaskTracker::new(),
            permit: Arc::new(Semaphore::new(1)),
            outstanding: Arc::new(Semaphore::new(17)),
        }))
    }

    pub fn implementation_pin(&self) -> &str {
        &self.pin
    }
    /// Static admission check matching Extism's callable export ABI. This does
    /// not compile or execute unreviewed code merely to advertise a tool.
    pub fn require_export(&self, export: &str) -> Result<()> {
        ensure!(
            !export.is_empty()
                && export.len() <= 128
                && export
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && self.exports.contains(export),
            "plugin export is missing or must take no parameters and return nothing or i32"
        );
        Ok(())
    }
    pub fn is_live(&self) -> bool {
        *self.admission.lock().unwrap() == Admission::Enabled
    }
    pub fn enable(&self, expected_pin: &str) -> Result<()> {
        ensure!(
            expected_pin == self.pin,
            "deployment identity changed; review the current pin"
        );
        let mut state = self.admission.lock().unwrap();
        ensure!(
            *state != Admission::Revoked,
            "revoked deployment cannot be reactivated"
        );
        *state = Admission::Enabled;
        Ok(())
    }

    /// Returning a timeout means cleanup is pending. The tracked owner retains
    /// the blocking task and permit until compilation/initialization/call ends.
    pub async fn invoke(
        &self,
        export: &str,
        input: &[u8],
        deadline: Instant,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>> {
        ensure!(
            input.len() <= self.limits.max_input_bytes,
            "plugin input exceeds byte budget"
        );
        self.require_export(export)?;
        let deadline = deadline.min(Instant::now() + Duration::from_millis(self.limits.timeout_ms));
        ensure!(
            deadline > Instant::now() && !cancel.is_cancelled(),
            "plugin invocation cancelled before admission"
        );
        let (reply, response) = oneshot::channel();
        {
            // Closing admission and creating the owner are one synchronous operation.
            let state = self.admission.lock().unwrap();
            ensure!(
                *state == Admission::Enabled,
                "plugin deployment is disabled or revoked"
            );
            let outstanding = self
                .outstanding
                .clone()
                .try_acquire_owned()
                .context("plugin invocation queue is full")?;
            let module = self.module.clone();
            let limits = self.limits.clone();
            let export = export.to_owned();
            let input = input.to_vec();
            let stop = self.stop.clone();
            let permit = self.permit.clone();
            self.tasks.spawn(async move {
                let _outstanding = outstanding;
                own_call(
                    module, limits, export, input, deadline, cancel, stop, permit, reply,
                )
                .await;
            });
        }
        match tokio::time::timeout_at(deadline, response).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => anyhow::bail!("plugin owner failed"),
            Err(_) => anyhow::bail!("plugin deadline exceeded; cleanup pending"),
        }
    }

    pub fn close(&self) {
        *self.admission.lock().unwrap() = Admission::Revoked;
        self.stop.cancel();
        self.tasks.close();
    }
    pub async fn revoke(&self) -> Result<()> {
        self.shutdown().await
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.close();
        self.tasks.wait().await;
        Ok(())
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        self.close();
    }
}

#[allow(clippy::too_many_arguments)]
async fn own_call(
    module: Arc<[u8]>,
    limits: Limits,
    export: String,
    input: Vec<u8>,
    deadline: Instant,
    cancel: CancellationToken,
    stop: CancellationToken,
    permit: Arc<Semaphore>,
    mut reply: oneshot::Sender<Result<Vec<u8>>>,
) {
    let permit = tokio::select! {
        biased;
        _ = stop.cancelled() => { let _ = reply.send(Err(anyhow::anyhow!("plugin revoked"))); return; },
        _ = cancel.cancelled() => { let _ = reply.send(Err(anyhow::anyhow!("plugin invocation cancelled"))); return; },
        _ = reply.closed() => return,
        _ = tokio::time::sleep_until(deadline) => { let _ = reply.send(Err(anyhow::anyhow!("plugin deadline exceeded before execution"))); return; },
        result = permit.acquire_owned() => match result { Ok(permit) => permit, Err(_) => return },
    };
    static GLOBAL: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let global = GLOBAL.get_or_init(|| Arc::new(Semaphore::new(2))).clone();
    let _global_permit = tokio::select! {
        biased;
        _ = stop.cancelled() => return,
        _ = cancel.cancelled() => return,
        _ = reply.closed() => return,
        _ = tokio::time::sleep_until(deadline) => return,
        result = global.acquire_owned() => match result { Ok(permit) => permit, Err(_) => return },
    };
    let (handle_tx, handle_rx) = oneshot::channel();
    let mut worker = tokio::task::spawn_blocking(move || {
        // Prevent guest text or private input from reaching any inherited subscriber.
        let _quiet = tracing::subscriber::set_default(tracing::subscriber::NoSubscriber::default());
        let manifest = extism::Manifest::new([extism::Wasm::data(module.to_vec())])
            .with_memory_max(limits.memory_pages)
            .with_timeout(Duration::from_millis(limits.timeout_ms));
        let mut config = wasmtime::Config::new();
        config
            .wasm_backtrace_max_frames(None)
            .parallel_compilation(false);
        let mut plugin = extism::PluginBuilder::new(manifest)
            .with_wasmtime_config(config)
            .with_wasi(false)
            .with_cache_disabled()
            .with_debug_options(extism::DebugOptions {
                profiling_strategy: wasmtime::ProfilingStrategy::None,
                coredump: None,
                memdump: None,
                debug_info: false,
            })
            .with_fuel_limit(limits.fuel)
            .with_initialization_fuel_limit(limits.initialization_fuel)
            .build()
            .map_err(|_| anyhow::anyhow!("plugin compilation or initialization failed"))?;
        let _ = handle_tx.send(plugin.cancel_handle());
        // Borrow first; never let guest output trigger an unbounded copy/decoder.
        let bytes: &[u8] = plugin.call(&export, input.as_slice()).map_err(|_| {
            anyhow::anyhow!("plugin trapped, exhausted its budget, or returned an error")
        })?;
        ensure!(
            bytes.len() <= limits.max_output_bytes,
            "plugin output exceeds byte budget"
        );
        Ok(bytes.to_vec())
    });
    let mut handle = None;
    let mut handle_rx = Some(handle_rx);
    let completed = loop {
        tokio::select! {
            result = &mut worker => {
                let _ = reply.send(result.unwrap_or_else(|_| Err(anyhow::anyhow!("plugin worker panicked"))));
                break true;
            },
            received = async { handle_rx.as_mut().expect("guarded receiver").await }, if handle_rx.is_some() => {
                handle = received.ok();
                handle_rx = None;
            },
            _ = stop.cancelled() => break false,
            _ = cancel.cancelled() => break false,
            _ = reply.closed() => break false,
            _ = tokio::time::sleep_until(deadline) => break false,
        }
    };
    // Never abort or drop a live JoinHandle: compilation and initialization
    // remain owned even if the caller no longer waits for their result.
    if !completed {
        if let Some(receiver) = handle_rx {
            handle = receiver.await.ok();
        }
        if let Some(handle) = handle {
            let _ = handle.cancel();
        }
        let _ = worker.await;
    }
    drop(permit);
}

fn validate_limits(limits: &Limits) -> Result<()> {
    ensure!(
        (1..=1024).contains(&limits.memory_pages),
        "plugin memory budget must be 1..1024 pages"
    );
    ensure!(
        (1..=100_000_000).contains(&limits.fuel)
            && (1..=20_000_000).contains(&limits.initialization_fuel),
        "invalid plugin fuel budget"
    );
    ensure!(
        (1..=30_000).contains(&limits.timeout_ms),
        "plugin timeout must be 1..30000 ms"
    );
    ensure!(
        (1..=1_048_576).contains(&limits.max_input_bytes)
            && (1..=1_048_576).contains(&limits.max_output_bytes),
        "invalid plugin byte budget"
    );
    Ok(())
}

pub fn validate_module(module: &[u8], limits: &Limits) -> Result<()> {
    validated_exports(module, limits).map(|_| ())
}
fn validated_exports(module: &[u8], limits: &Limits) -> Result<BTreeSet<String>> {
    use wasmparser::{ExternalKind, Parser, Payload, TypeRef, ValType};
    ensure!(
        module.len() <= MAX_MODULE_BYTES && module.starts_with(b"\0asm"),
        "expected a bounded WebAssembly binary"
    );
    let mut features = wasmparser::WasmFeatures::default();
    features.set(wasmparser::WasmFeatures::GC, false);
    features.set(wasmparser::WasmFeatures::THREADS, false);
    features.set(wasmparser::WasmFeatures::MEMORY64, false);
    features.set(wasmparser::WasmFeatures::MULTI_MEMORY, false);
    features.set(wasmparser::WasmFeatures::COMPONENT_MODEL, false);
    wasmparser::Validator::new_with_features(features)
        .validate_all(module)
        .context("unsupported or invalid guest WASM")?;
    let mut table_count = 0;
    let mut callable_types = Vec::new();
    let mut function_types = Vec::new();
    let mut exports = BTreeSet::new();
    for payload in Parser::new(0).parse_all(module) {
        match payload? {
            Payload::TypeSection(types) => {
                for ty in types.into_iter_err_on_gc_types() {
                    let ty = ty?;
                    callable_types.push(
                        ty.params().is_empty()
                            && (ty.results().is_empty() || ty.results() == [ValType::I32]),
                    );
                }
            }
            Payload::FunctionSection(functions) => {
                for ty in functions {
                    function_types.push(ty?);
                }
            }
            Payload::ExportSection(items) => {
                for export in items {
                    let export = export?;
                    if export.kind == ExternalKind::Func
                        && function_types
                            .get(export.index as usize)
                            .and_then(|ty| callable_types.get(*ty as usize))
                            .copied()
                            == Some(true)
                    {
                        exports.insert(export.name.to_owned());
                    }
                }
            }
            Payload::StartSection { .. } => anyhow::bail!("guest start sections are not allowed"),
            Payload::ImportSection(imports) => {
                for import in imports.into_imports() {
                    let import = import?;
                    if let TypeRef::Func(ty) = import.ty {
                        function_types.push(ty);
                    }
                    ensure!(
                        matches!(import.ty, TypeRef::Func(_)) && import.module == "extism:host/env",
                        "guest imports unauthorized host authority"
                    );
                    ensure!(
                        !matches!(
                            import.name,
                            "http_request" | "http_headers" | "http_status_code"
                        ),
                        "guest HTTP is not allowed; use a separately authorized host tool"
                    );
                }
            }
            Payload::MemorySection(memories) => {
                for memory in memories {
                    let memory = memory?;
                    ensure!(
                        !memory.shared
                            && !memory.memory64
                            && memory.initial <= limits.memory_pages as u64,
                        "invalid guest memory"
                    );
                    ensure!(
                        memory
                            .maximum
                            .is_some_and(|max| max <= limits.memory_pages as u64),
                        "guest memory must declare a maximum within the host budget"
                    );
                }
            }
            Payload::TableSection(tables) => {
                for table in tables {
                    table_count += 1;
                    ensure!(table_count <= 1, "guest may have at most one table");
                    let table = table?.ty;
                    ensure!(
                        table.maximum.is_some_and(|max| max <= 65536) && table.initial <= 65536,
                        "guest table requires a bounded maximum"
                    );
                }
            }
            _ => {}
        }
    }
    Ok(exports)
}

fn host_digest() -> Result<&'static str> {
    static HASH: OnceLock<String> = OnceLock::new();
    if let Some(hash) = HASH.get() {
        return Ok(hash);
    }
    let mut file = std::fs::File::open(std::env::current_exe()?)?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    let _ = HASH.set(format!("{:x}", hash.finalize()));
    Ok(HASH.get().expect("host hash initialized"))
}

#[cfg(test)]
mod tests;
