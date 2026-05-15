use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::binding::BindingManager;
use crate::config::EmbeddedConfig;
use crate::identity;
use crate::plugin::PluginRegistry;
use crate::process::ProcessManager;
use crate::settings::EmbeddedSettingsManager;
use crate::storage::EmbeddedStore;

use super::context::{AudioBus, EventBus, ModuleContext};
use super::factory;
use super::lifecycle::ModuleLifecycle;
use super::noop::{
    NoopAudioCapture, NoopAudioPlayback, NoopDirectResponse, NoopDiscovery, NoopEdgeStt,
    NoopFaceTracker, NoopMixer, NoopVad, NoopWakeWord,
};
use super::registry::EmbeddedModuleRegistry;
use super::speech::SpeechPipeline;
use super::traits::*;

const GRPC_MESSAGE_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// The assembled embedded node, ready to run.
pub struct EmbeddedNode {
    pub registry: EmbeddedModuleRegistry,
    pub config: Arc<EmbeddedConfig>,
    pub cancel_token: CancellationToken,
    pub binding_manager: Arc<BindingManager>,
    pub settings_manager: Arc<EmbeddedSettingsManager>,
    pub state_store: EmbeddedStore,
    pub event_bus: EventBus,
}

impl EmbeddedNode {
    /// Run the embedded node (starts gRPC server, blocks until cancellation).
    pub async fn run(self) -> Result<()> {
        use alia_proto::audio::audio_stream_server::AudioStreamServer;
        use alia_proto::binding::device_binding_server::DeviceBindingServer;
        use alia_proto::plugin::plugin_manager_server::PluginManagerServer;
        use alia_proto::process::process_manager_server::ProcessManagerServer;
        use alia_proto::settings::embedded_settings_server::EmbeddedSettingsServer;
        use tonic::transport::Server;

        use crate::grpc::{
            AudioStreamService, DeviceBindingService, EmbeddedSettingsService,
            PluginManagerService, ProcessManagerService,
        };

        let audio_svc = AudioStreamServer::new(AudioStreamService::new(
            self.registry.audio_playback.clone(),
            self.binding_manager.clone(),
            self.event_bus.clone(),
        ))
        .max_decoding_message_size(GRPC_MESSAGE_LIMIT_BYTES)
        .max_encoding_message_size(GRPC_MESSAGE_LIMIT_BYTES);
        let process_svc = ProcessManagerServer::new(ProcessManagerService::new(
            self.registry.process_manager.clone(),
            self.binding_manager.clone(),
        ));
        let device_public_key =
            identity::load_device_public_key(&self.config.identity.keypair_path)
                .context("loading device public key")?;
        let plugin_svc = PluginManagerServer::new(PluginManagerService::new(
            self.registry.plugin_registry.clone(),
            self.binding_manager.clone(),
            self.registry.discovery.clone(),
            self.settings_manager.clone(),
        ));
        let binding_svc = DeviceBindingServer::new(DeviceBindingService::new(
            self.binding_manager.clone(),
            self.config.identity.clone(),
            self.registry.discovery.clone(),
            self.registry.plugin_registry.clone(),
            self.settings_manager.clone(),
            device_public_key,
        ));
        let settings_svc = EmbeddedSettingsServer::new(EmbeddedSettingsService::new(
            self.binding_manager.clone(),
            self.settings_manager.clone(),
            self.state_store.clone(),
            self.registry.discovery.clone(),
            self.registry.plugin_registry.clone(),
        ));

        let addr: SocketAddr = "0.0.0.0:50051".parse()?;
        info!(%addr, "gRPC server listening");

        let cancel = self.cancel_token.clone();
        Server::builder()
            .add_service(audio_svc)
            .add_service(process_svc)
            .add_service(plugin_svc)
            .add_service(binding_svc)
            .add_service(settings_svc)
            .serve_with_shutdown(addr, async move {
                cancel.cancelled().await;
            })
            .await?;

        // Graceful shutdown
        info!("shutting down modules");
        self.registry.shutdown().await;

        Ok(())
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
    pub async fn from_config(config: EmbeddedConfig) -> Result<Self> {
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

    /// Build the embedded node: instantiate modules, initialize them, start services.
    pub async fn build(self) -> Result<EmbeddedNode> {
        let config = self.config.clone();

        // Create shared context resources
        let run_dir = PathBuf::from("run");
        let state_store = EmbeddedStore::open(run_dir.join("embedded-state.sqlite3"))
            .await
            .context("opening embedded SQLite state store")?;
        let event_bus = EventBus::new(256);
        let audio_bus = AudioBus::new(512);
        let ctx = ModuleContext {
            config: config.clone(),
            state_store: state_store.clone(),
            event_bus,
            cancel_token: self.cancel_token.clone(),
            audio_bus,
        };

        // Resolve modules: use overrides or fall back to factory
        let mut wakeword: Box<dyn WakeWordEngine> = match self.wakeword {
            Some(m) => m,
            None => factory::create_wakeword(&config).unwrap_or_else(|error| {
                warn!(error = ?error, "wakeword module creation failed; using noop");
                Box::new(NoopWakeWord)
            }),
        };
        let mut edge_stt: Box<dyn EdgeSttEngine> = match self.edge_stt {
            Some(m) => m,
            None => factory::create_edge_stt(&config).unwrap_or_else(|error| {
                warn!(error = ?error, "edge STT module creation failed; using noop");
                Box::new(NoopEdgeStt)
            }),
        };
        let mut vad: Box<dyn VoiceActivityEngine> = match self.vad {
            Some(m) => m,
            None => factory::create_vad(&config).unwrap_or_else(|error| {
                warn!(error = ?error, "VAD module creation failed; using noop");
                Box::new(NoopVad)
            }),
        };
        let mut direct_response: Box<dyn DirectResponseEngine> = match self.direct_response {
            Some(m) => m,
            None => factory::create_direct_response(&config).unwrap_or_else(|error| {
                warn!(error = ?error, "direct response module creation failed; using noop");
                Box::new(NoopDirectResponse)
            }),
        };
        let mut audio_capture: Box<dyn AudioCaptureBackend> = match self.audio_capture {
            Some(m) => m,
            None => factory::create_audio_capture(&config).unwrap_or_else(|error| {
                warn!(error = ?error, "audio capture module creation failed; using noop");
                Box::new(NoopAudioCapture)
            }),
        };
        let mut audio_playback: Box<dyn AudioPlaybackBackend> = match self.audio_playback {
            Some(m) => m,
            None => factory::create_audio_playback(&config).unwrap_or_else(|error| {
                warn!(error = ?error, "audio playback module creation failed; using noop");
                Box::new(NoopAudioPlayback)
            }),
        };
        let mut mixer: Box<dyn AudioMixer> = match self.mixer {
            Some(m) => m,
            None => factory::create_mixer(&config).unwrap_or_else(|error| {
                warn!(error = ?error, "mixer module creation failed; using noop");
                Box::new(NoopMixer)
            }),
        };
        let mut face_tracker: Box<dyn FaceTracker> = match self.face_tracker {
            Some(m) => m,
            None => factory::create_face_tracker(&config).unwrap_or_else(|error| {
                warn!(error = ?error, "face tracker module creation failed; using noop");
                Box::new(NoopFaceTracker)
            }),
        };
        let mut discovery: Box<dyn DiscoveryBackend> = match self.discovery {
            Some(m) => m,
            None => factory::create_discovery(&config).unwrap_or_else(|error| {
                warn!(error = ?error, "discovery module creation failed; using noop");
                Box::new(NoopDiscovery)
            }),
        };

        // Initialize all modules
        info!("initializing modules");
        init_embedded_module(wakeword.as_mut(), &ctx).await;
        init_embedded_module(edge_stt.as_mut(), &ctx).await;
        init_embedded_module(vad.as_mut(), &ctx).await;
        init_embedded_module(direct_response.as_mut(), &ctx).await;
        init_embedded_module(audio_capture.as_mut(), &ctx).await;
        init_embedded_module(audio_playback.as_mut(), &ctx).await;
        init_embedded_module(mixer.as_mut(), &ctx).await;
        init_embedded_module(face_tracker.as_mut(), &ctx).await;
        init_embedded_module(discovery.as_mut(), &ctx).await;

        // Initialize core infrastructure
        let process_manager =
            Arc::new(ProcessManager::new(config.process.clone(), run_dir.clone()));
        if let Err(e) = process_manager.cleanup_orphans().await {
            error!(error = %e, "failed to clean up orphan processes");
        }

        let plugins_dir = PathBuf::from("plugins");
        let plugin_registry = Arc::new(PluginRegistry::new(plugins_dir));
        if let Err(e) = plugin_registry.scan().await {
            error!(error = %e, "failed to scan plugins directory");
        }

        let settings_manager = Arc::new(EmbeddedSettingsManager::new(&config));

        // Initialize binding manager
        let binding_manager = Arc::new(BindingManager::new(
            state_store,
            config.discovery.max_bind_attempts,
            config.discovery.lockout_duration_sec,
        ));
        if let Err(e) = binding_manager.load().await {
            error!(error = %e, "failed to load binding state");
        }
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
            binding_manager
                .seed_dev_binding(server_id, server_name, endpoint)
                .await
                .context("seeding dev binding")?;
        }

        let wakeword: Arc<dyn WakeWordEngine> = Arc::from(wakeword);
        let edge_stt: Arc<dyn EdgeSttEngine> = Arc::from(edge_stt);
        let vad: Arc<dyn VoiceActivityEngine> = Arc::from(vad);
        let direct_response: Arc<dyn DirectResponseEngine> = Arc::from(direct_response);
        let audio_capture: Arc<dyn AudioCaptureBackend> = Arc::from(audio_capture);
        let audio_playback: Arc<dyn AudioPlaybackBackend> = Arc::from(audio_playback);
        let mixer: Arc<dyn AudioMixer> = Arc::from(mixer);
        let face_tracker: Arc<dyn FaceTracker> = Arc::from(face_tracker);
        let discovery: Arc<dyn DiscoveryBackend> = Arc::from(discovery);
        let speech_pipeline = Arc::new(SpeechPipeline::new(
            ctx.audio_bus.clone(),
            ctx.event_bus.clone(),
            wakeword.clone(),
            edge_stt.clone(),
            vad.clone(),
            direct_response.clone(),
            audio_playback.clone(),
            binding_manager.clone(),
            &config.edge_stt,
            self.cancel_token.clone(),
        ));

        // Start all modules
        info!("starting modules");
        start_embedded_module(wakeword.as_ref()).await;
        start_embedded_module(edge_stt.as_ref()).await;
        start_embedded_module(vad.as_ref()).await;
        start_embedded_module(direct_response.as_ref()).await;
        start_embedded_module(audio_capture.as_ref()).await;
        start_embedded_module(audio_playback.as_ref()).await;
        start_embedded_module(mixer.as_ref()).await;
        start_embedded_module(speech_pipeline.as_ref()).await;
        if let Err(error) = audio_capture.start_capture(&ctx.audio_bus) {
            warn!(
                module = audio_capture.name(),
                error = ?error,
                "audio capture stream failed to start; continuing without microphone capture"
            );
        }
        start_embedded_module(face_tracker.as_ref()).await;
        start_embedded_module(discovery.as_ref()).await;

        let initial_discovery_metadata = binding_manager.status().await.discovery_metadata();
        let mut initial_discovery_metadata = initial_discovery_metadata;
        initial_discovery_metadata.attributes.insert(
            "provisioning_mode".to_string(),
            settings_manager.provisioning_mode().await.to_string(),
        );
        plugin_registry
            .enrich_discovery_metadata(&mut initial_discovery_metadata)
            .await;
        discovery
            .update_metadata(&initial_discovery_metadata)
            .await
            .unwrap_or_else(
                |error| warn!(error = ?error, "failed to sync discovery metadata; continuing"),
            );

        // Build registry
        let registry = EmbeddedModuleRegistry {
            wakeword,
            edge_stt,
            vad,
            direct_response,
            audio_capture,
            audio_playback,
            mixer,
            speech_pipeline,
            face_tracker,
            discovery,
            process_manager,
            plugin_registry,
        };

        Ok(EmbeddedNode {
            registry,
            config,
            cancel_token: self.cancel_token,
            binding_manager,
            settings_manager,
            state_store: ctx.state_store.clone(),
            event_bus: ctx.event_bus.clone(),
        })
    }
}

async fn init_embedded_module(module: &mut dyn ModuleLifecycle, ctx: &ModuleContext) {
    let name = module.name();
    if let Err(error) = module.init(ctx).await {
        warn!(
            module = name,
            error = %error,
            "embedded module init failed; continuing in degraded mode"
        );
        tracing::debug!(
            module = name,
            error = ?error,
            "embedded module init failure details"
        );
    }
}

async fn start_embedded_module(module: &dyn ModuleLifecycle) {
    let name = module.name();
    if let Err(error) = module.start().await {
        warn!(
            module = name,
            error = %error,
            "embedded module start failed; continuing in degraded mode"
        );
        tracing::debug!(
            module = name,
            error = ?error,
            "embedded module start failure details"
        );
    }
}
