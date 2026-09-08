use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::FutureExt;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::binding::BindingManager;
use crate::config::EmbeddedConfig;
use crate::identity;
use crate::plugin::PluginRegistry;
use crate::settings::EmbeddedSettingsManager;
use crate::storage::EmbeddedStore;

use super::audio_debug::AudioDebugRecorder;
use super::context::{AudioBus, EventBus, ModuleContext};
use super::factory;
use super::hmmd_mmwave::HmmdMmWaveModule;
use super::lifecycle::{
    CancelOnDrop, ModuleCleanup, ModuleLifecycle, ShutdownHandle, initialize, startup_step,
};
use super::motion::MotionRuntime;
use super::noop::NoopEdgeStt;
use super::playback_session::PlaybackSession;
use super::registry::EmbeddedModuleRegistry;
use super::sensor::SensorRegistry;
use super::speech::SpeechPipeline;
use super::traits::*;
use super::vision_debug::{VisionDebugHub, VisionDebugServer};
use super::volume::{AUDIO_VOLUME_STATE_KEY, AudioVolumeConfig, AudioVolumeState};

mod transport;
use transport::Connections;

const GRPC_MESSAGE_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// The assembled embedded node, ready to run.
pub struct EmbeddedNode {
    pub registry: EmbeddedModuleRegistry,
    /// Effective configuration used by modules, including the actual bound port.
    pub config: Arc<EmbeddedConfig>,
    /// Requested settings retained for persistence/restart (e.g. listen port 0).
    pub requested_config: Arc<EmbeddedConfig>,
    pub cancel_token: CancellationToken,
    pub binding_manager: Arc<BindingManager>,
    pub settings_manager: Arc<EmbeddedSettingsManager>,
    pub state_store: EmbeddedStore,
    pub volume_state: AudioVolumeState,
    pub audio_debug: AudioDebugRecorder,
    pub event_bus: EventBus,
    listener: Option<tokio::net::TcpListener>,
    connections: Arc<Connections>,
    bound_addr: SocketAddr,
}

impl EmbeddedNode {
    /// The actual bound address, including the selected port when configured as :0.
    pub fn local_addr(&self) -> SocketAddr {
        self.bound_addr
    }

    /// Obtain before running if the host needs to await cleanup after a deadline.
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.registry.cleanup.handle()
    }

    /// Stop a built node without starting the gRPC request loop.
    pub async fn shutdown(mut self) -> Result<()> {
        self.listener.take();
        self.connections.close();
        let deadline = tokio::time::Instant::now() + self.shutdown_grace();
        self.registry.cleanup.start().wait_until(deadline).await
    }

    fn shutdown_grace(&self) -> Duration {
        Duration::from_millis(self.config.device_runtime.shutdown_grace_ms)
    }

    /// Run until cancellation. No logger, signal handler, or global runtime is installed.
    /// Dropping this future also requests cleanup through the node's resource owner.
    pub async fn run(mut self) -> Result<()> {
        let cancel = self.cancel_token.clone();
        let grace = self.shutdown_grace();
        let drain = Duration::from_millis(self.config.device_runtime.grpc_drain_timeout_ms);
        let (result, deadline) = {
            let services = self.run_services();
            tokio::pin!(services);
            tokio::select! {
                result = &mut services => (result, tokio::time::Instant::now() + grace),
                _ = cancel.cancelled() => {
                    let deadline = tokio::time::Instant::now() + grace;
                    let result = tokio::time::timeout(drain, &mut services).await
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("gRPC drain deadline exceeded; remaining streams closed")));
                    (result, deadline)
                }
            }
        }; // Stop accepting requests; the socket fence below terminates remaining streams.
        self.connections.close();
        let cleanup = self.registry.cleanup.start().wait_until(deadline).await;
        match (result, cleanup) {
            (Ok(()), result) => result,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => {
                Err(cleanup.context(format!("runtime also failed: {error:#}")))
            }
        }
    }

    async fn run_services(&mut self) -> Result<()> {
        use alia_proto::audio::audio_stream_server::AudioStreamServer;
        use alia_proto::binding::device_binding_server::DeviceBindingServer;
        use alia_proto::expression::embedded_expression_server::EmbeddedExpressionServer;
        use alia_proto::playback::playback_server::PlaybackServer;
        use alia_proto::plugin::plugin_manager_server::PluginManagerServer;
        use alia_proto::sensor::embedded_sensor_server::EmbeddedSensorServer;
        use alia_proto::settings::embedded_settings_server::EmbeddedSettingsServer;
        use tonic::transport::Server;

        use crate::grpc::{
            AudioStreamService, DeviceBindingService, EmbeddedExpressionService,
            EmbeddedSettingsService, PluginManagerService, SensorService,
        };

        let audio_svc = AudioStreamServer::new(AudioStreamService::new(
            self.registry.audio_playback.clone(),
            self.registry.direct_response.clone(),
            self.binding_manager.clone(),
            self.event_bus.clone(),
            self.audio_debug.clone(),
            self.config.speech.mode == crate::config::SpeechMode::ServerTurn,
        ))
        .max_decoding_message_size(GRPC_MESSAGE_LIMIT_BYTES)
        .max_encoding_message_size(GRPC_MESSAGE_LIMIT_BYTES);
        let playback_svc = PlaybackServer::new(crate::grpc::PlaybackService::new(
            self.registry.playback_session.clone(),
            self.binding_manager.clone(),
        ))
        .max_decoding_message_size(64 * 1024)
        .max_encoding_message_size(64 * 1024);
        let device_public_key =
            identity::load_device_public_key(&self.config.identity.keypair_path)
                .context("loading device public key")?;
        let plugin_svc = PluginManagerServer::new(PluginManagerService::new(
            self.registry.plugin_registry.clone(),
            self.binding_manager.clone(),
            self.registry.discovery.clone(),
            self.registry.motion.clone(),
            self.settings_manager.clone(),
        ));
        let binding_svc = DeviceBindingServer::new(DeviceBindingService::new(
            self.binding_manager.clone(),
            self.config.identity.clone(),
            self.registry.discovery.clone(),
            self.registry.plugin_registry.clone(),
            self.registry.motion.clone(),
            self.settings_manager.clone(),
            device_public_key,
        ));
        let settings_svc = EmbeddedSettingsServer::new(EmbeddedSettingsService::new(
            self.binding_manager.clone(),
            self.settings_manager.clone(),
            self.state_store.clone(),
            self.volume_state.clone(),
            self.registry.discovery.clone(),
            self.registry.plugin_registry.clone(),
            self.registry.motion.clone(),
        ));
        let sensor_svc = EmbeddedSensorServer::new(SensorService::new(
            self.registry.sensors.clone(),
            self.binding_manager.clone(),
        ));
        let expression_svc = EmbeddedExpressionServer::new(EmbeddedExpressionService::new(
            self.binding_manager.clone(),
            self.registry.motion.clone(),
        ));

        let addr = self.local_addr();
        let listener = self
            .listener
            .take()
            .context("node listener already consumed")?;
        let connections = self.connections.clone();
        let incoming = async_stream::stream! {
            loop { yield listener.accept().await.and_then(|(stream, _)| connections.wrap(stream)); }
        };
        info!(%addr, "gRPC server listening");

        let cancel = self.cancel_token.clone();
        Server::builder()
            .add_service(audio_svc)
            .add_service(plugin_svc)
            .add_service(binding_svc)
            .add_service(settings_svc)
            .add_service(sensor_svc)
            .add_service(playback_svc)
            .add_service(expression_svc)
            .serve_with_incoming_shutdown(incoming, async move {
                cancel.cancelled().await;
            })
            .await?;

        Ok(())
    }
}

impl Drop for EmbeddedNode {
    fn drop(&mut self) {
        self.cancel_token.cancel();
        self.connections.close();
        self.registry.cleanup.start();
    }
}

/// Builder for assembling the embedded node with configurable module slots.
///
/// Modules not explicitly overridden via `with_*` methods are created from
/// the `[modules]` config section using the factory.
pub struct EmbeddedBuilder {
    config: Arc<EmbeddedConfig>,
    cancel_token: CancellationToken,

    // Module slot overrides (None = use factory default from config)
    wakeword: Option<Box<dyn WakeWordEngine>>,
    edge_stt: Option<Box<dyn EdgeSttEngine>>,
    vad: Option<Box<dyn VoiceActivityEngine>>,
    direct_response: Option<Box<dyn DirectResponseEngine>>,
    audio_capture: Option<Box<dyn AudioCaptureBackend>>,
    audio_playback: Option<Box<dyn AudioPlaybackBackend>>,
    mixer: Option<Box<dyn AudioMixer>>,
    face_tracker: Option<Box<dyn FaceTracker>>,
    discovery: Option<Box<dyn DiscoveryBackend>>,
}

impl EmbeddedBuilder {
    /// Create a builder from the loaded configuration.
    pub async fn from_config(mut config: EmbeddedConfig) -> Result<Self> {
        crate::config::resolve_config_paths(&mut config)?;
        config.speech.validate(&config.audio)?;
        Ok(Self {
            config: Arc::new(config),
            cancel_token: CancellationToken::new(),
            wakeword: None,
            edge_stt: None,
            vad: None,
            direct_response: None,
            audio_capture: None,
            audio_playback: None,
            mixer: None,
            face_tracker: None,
            discovery: None,
        })
    }

    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = token;
        self
    }

    pub fn with_wakeword(mut self, engine: impl WakeWordEngine + 'static) -> Self {
        self.wakeword = Some(Box::new(engine));
        self
    }

    pub fn with_edge_stt(mut self, engine: impl EdgeSttEngine + 'static) -> Self {
        self.edge_stt = Some(Box::new(engine));
        self
    }

    pub fn with_vad(mut self, engine: impl VoiceActivityEngine + 'static) -> Self {
        self.vad = Some(Box::new(engine));
        self
    }

    pub fn with_direct_response(mut self, engine: impl DirectResponseEngine + 'static) -> Self {
        self.direct_response = Some(Box::new(engine));
        self
    }

    pub fn with_audio_capture(mut self, backend: impl AudioCaptureBackend + 'static) -> Self {
        self.audio_capture = Some(Box::new(backend));
        self
    }

    pub fn with_audio_playback(mut self, backend: impl AudioPlaybackBackend + 'static) -> Self {
        self.audio_playback = Some(Box::new(backend));
        self
    }

    pub fn with_mixer(mut self, mixer: impl AudioMixer + 'static) -> Self {
        self.mixer = Some(Box::new(mixer));
        self
    }

    pub fn with_face_tracker(mut self, tracker: impl FaceTracker + 'static) -> Self {
        self.face_tracker = Some(Box::new(tracker));
        self
    }

    pub fn with_discovery(mut self, backend: impl DiscoveryBackend + 'static) -> Self {
        self.discovery = Some(Box::new(backend));
        self
    }

    /// Bind, construct and initialize transactionally, then start in dependency order.
    /// The internal owner survives caller-future cancellation and finishes rollback.
    pub async fn build(mut self) -> Result<EmbeddedNode> {
        self.cancel_token = self.cancel_token.child_token();
        let cancelled = self.cancel_token.clone();
        let mut guard = CancelOnDrop(Some(cancelled.clone()));
        let result = tokio::spawn(async move {
            let grace = Duration::from_millis(self.config.device_runtime.shutdown_grace_ms);
            let cleanup = ModuleCleanup::new(self.cancel_token.clone(), grace);
            let result = std::panic::AssertUnwindSafe(self.build_owned(cleanup.clone()))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("embedded startup panicked")));
            if result.is_err() {
                if let Err(error) = cleanup
                    .start()
                    .wait_until(tokio::time::Instant::now() + grace)
                    .await
                {
                    return Err(
                        error.context(format!("startup failed: {:#}", result.err().unwrap()))
                    );
                }
            }
            result
        })
        .await
        .context("embedded startup task failed")?;
        // A successful node takes ownership of cancellation. Failed/dropped callers cancel.
        if result.is_ok() {
            guard.0.take();
        }
        result
    }

    async fn build_owned(self, cleanup: Arc<ModuleCleanup>) -> Result<EmbeddedNode> {
        anyhow::ensure!(
            !self.cancel_token.is_cancelled(),
            "startup cancelled before binding"
        );
        let listener = tokio::net::TcpListener::bind(self.config.device_runtime.listen_addr)
            .await
            .context("binding embedded gRPC listener")?;
        let connections = Connections::new();
        cleanup.add(connections.clone());
        let mut resolved_config = (*self.config).clone();
        let bound_addr = listener.local_addr()?;
        resolved_config.device_runtime.listen_addr = bound_addr;
        let config = Arc::new(resolved_config);
        let state_store = startup_step(
            &self.cancel_token,
            "opening embedded SQLite state store",
            EmbeddedStore::open(&config.device_runtime.state_path),
        )
        .await?;
        let sensors = Arc::new(SensorRegistry::new());
        let ctx = ModuleContext {
            config: config.clone(),
            state_store: state_store.clone(),
            event_bus: EventBus::new(256),
            audio_bus: AudioBus::new(512),
            cancel_token: self.cancel_token.clone(),
            sensors: sensors.clone(),
        };
        let volume_config = startup_step(
            &self.cancel_token,
            "loading persisted audio volume",
            state_store.load_json::<AudioVolumeConfig>(AUDIO_VOLUME_STATE_KEY),
        )
        .await?
        .map(|value| value.sanitized())
        .unwrap_or_else(|| AudioVolumeConfig::from_audio_config(&config.audio));
        let volume_state = AudioVolumeState::new(volume_config);
        let audio_debug = AudioDebugRecorder::new(config.audio_debug.clone());
        let vision_debug_hub = Arc::new(VisionDebugHub::new(config.vision_debug.clone()));

        // Register even a failed/cancelled init before propagating its error.
        macro_rules! init {
            ($module:expr) => {{
                let (module, result) = initialize($module, &ctx).await;
                cleanup.add(module.clone());
                result?;
                module
            }};
        }
        let wakeword = init!(match self.wakeword {
            Some(module) => module,
            None => factory::create_wakeword(&config)?,
        });
        let edge_stt = init!(match self.edge_stt {
            Some(module) => module,
            None if config.speech.mode == crate::config::SpeechMode::ServerTurn =>
                Box::new(NoopEdgeStt) as Box<dyn EdgeSttEngine>,
            None => factory::create_edge_stt(&config)?,
        });
        let vad = init!(match self.vad {
            Some(module) => module,
            None => factory::create_vad(&config)?,
        });
        let direct_response = init!(match self.direct_response {
            Some(module) => module,
            None => factory::create_direct_response(&config)?,
        });
        let audio_playback = init!(match self.audio_playback {
            Some(module) => module,
            None => factory::create_audio_playback(&config, volume_state.clone())?,
        });
        let mixer = init!(match self.mixer {
            Some(module) => module,
            None => factory::create_mixer(&config)?,
        });
        let motion = init!(Box::new(
            MotionRuntime::new(config.motion.clone()).context("creating motion module")?
        ));
        let face_tracker = init!(match self.face_tracker {
            Some(module) => module,
            None => factory::create_face_tracker(
                &config,
                motion.clone(),
                Some(vision_debug_hub.clone())
            )?,
        });
        let vision_debug = init!(Box::new(VisionDebugServer::new(
            config.vision_debug.clone(),
            vision_debug_hub
        )));
        let mut sensor_modules: Vec<Arc<dyn ModuleLifecycle>> = Vec::new();
        if config.hmmd_mmwave.enabled {
            sensor_modules.push(init!(Box::new(HmmdMmWaveModule::new(
                config.hmmd_mmwave.clone()
            ))));
        }
        let audio_capture = init!(match self.audio_capture {
            Some(module) => module,
            None => factory::create_audio_capture(&config, volume_state.clone())?,
        });
        let plugin_registry = Arc::new(PluginRegistry::new(
            config.device_runtime.plugins_dir.clone(),
        ));
        cleanup.add(Arc::new(super::registry::PluginCleanup(
            plugin_registry.clone(),
        )));
        startup_step(
            &self.cancel_token,
            "scanning embedded plugins",
            plugin_registry.scan(),
        )
        .await?;
        // Settings describe the next boot; do not persist an ephemeral selected port.
        let settings_manager = Arc::new(EmbeddedSettingsManager::new(&self.config));
        let binding_manager = Arc::new(BindingManager::new(
            state_store.clone(),
            config.discovery.max_bind_attempts,
            config.discovery.lockout_duration_sec,
        ));
        startup_step(
            &self.cancel_token,
            "loading device binding",
            binding_manager.load(),
        )
        .await?;
        if config.dev.skip_binding {
            let endpoint = if config.dev.server_endpoint.trim().is_empty() {
                "http://127.0.0.1:3000"
            } else {
                config.dev.server_endpoint.trim()
            };
            let server_id = if config.dev.server_id.trim().is_empty() {
                "alia-local-dev"
            } else {
                config.dev.server_id.trim()
            };
            let server_name = if config.dev.server_name.trim().is_empty() {
                "alia-local-dev"
            } else {
                config.dev.server_name.trim()
            };
            startup_step(
                &self.cancel_token,
                "seeding dev binding",
                binding_manager.seed_dev_binding(server_id, server_name, endpoint),
            )
            .await?;
        }
        let playback_session = Arc::new(PlaybackSession::new(audio_playback.clone()));
        cleanup.add(playback_session.clone());
        let speech_pipeline = init!(Box::new(SpeechPipeline::new(
            ctx.audio_bus.clone(),
            ctx.event_bus.clone(),
            wakeword.clone(),
            edge_stt.clone(),
            vad.clone(),
            direct_response.clone(),
            audio_playback.clone(),
            playback_session.clone(),
            binding_manager.clone(),
            &config.edge_stt,
            &config.speech,
            &config.interrupt,
            &config.thinking,
            &config.continuous_dialogue,
            &config.identity,
            audio_debug.clone(),
            self.cancel_token.clone(),
        )));
        let discovery = init!(match self.discovery {
            Some(module) => module,
            None => factory::create_discovery(&config)?,
        });
        let registry = EmbeddedModuleRegistry {
            wakeword,
            edge_stt,
            vad,
            direct_response,
            audio_capture,
            audio_playback,
            playback_session,
            mixer,
            speech_pipeline,
            face_tracker,
            vision_debug,
            motion,
            discovery,
            sensors,
            sensor_modules,
            plugin_registry,
            cleanup,
        };
        info!("starting embedded modules");
        registry.start(&ctx).await?;
        let mut metadata = binding_manager.status().await.discovery_metadata();
        metadata.attributes.insert(
            "provisioning_mode".to_string(),
            settings_manager.provisioning_mode().await.to_string(),
        );
        registry.motion.enrich_discovery_metadata(&mut metadata);
        registry
            .plugin_registry
            .enrich_discovery_metadata(&mut metadata)
            .await;
        startup_step(
            &self.cancel_token,
            "publishing discovery metadata",
            registry.discovery.update_metadata(&metadata),
        )
        .await?;
        anyhow::ensure!(
            !self.cancel_token.is_cancelled(),
            "startup cancelled after module start"
        );
        Ok(EmbeddedNode {
            registry,
            config,
            requested_config: self.config,
            cancel_token: self.cancel_token,
            binding_manager,
            settings_manager,
            state_store,
            volume_state,
            audio_debug,
            event_bus: ctx.event_bus,
            listener: Some(listener),
            connections,
            bound_addr,
        })
    }
}
