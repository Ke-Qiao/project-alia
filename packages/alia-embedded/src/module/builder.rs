use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::binding::BindingManager;
use crate::config::EmbeddedConfig;
use crate::identity;
use crate::plugin::PluginRegistry;
use crate::process::ProcessManager;
use crate::settings::EmbeddedSettingsManager;

use super::context::{AudioBus, EventBus, ModuleContext};
use super::factory;
use super::registry::EmbeddedModuleRegistry;
use super::traits::*;

/// The assembled embedded node, ready to run.
pub struct EmbeddedNode {
    pub registry: EmbeddedModuleRegistry,
    pub config: Arc<EmbeddedConfig>,
    pub cancel_token: CancellationToken,
    pub binding_manager: Arc<BindingManager>,
    pub settings_manager: Arc<EmbeddedSettingsManager>,
}

impl EmbeddedNode {
    /// Run the embedded node (starts gRPC server, blocks until cancellation).
    pub async fn run(self) -> Result<()> {
        use alia_proto::binding::device_binding_server::DeviceBindingServer;
        use alia_proto::plugin::plugin_manager_server::PluginManagerServer;
        use alia_proto::process::process_manager_server::ProcessManagerServer;
        use alia_proto::settings::embedded_settings_server::EmbeddedSettingsServer;
        use tonic::transport::Server;

        use crate::grpc::{
            DeviceBindingService, EmbeddedSettingsService, PluginManagerService,
            ProcessManagerService,
        };

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
            self.registry.discovery.clone(),
            self.registry.plugin_registry.clone(),
        ));

        let addr: SocketAddr = "0.0.0.0:50051".parse()?;
        info!(%addr, "gRPC server listening");

        let cancel = self.cancel_token.clone();
        Server::builder()
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
        let event_bus = EventBus::new(256);
        let audio_bus = AudioBus::new(512);
        let ctx = ModuleContext {
            config: config.clone(),
            event_bus,
            cancel_token: self.cancel_token.clone(),
            audio_bus,
        };

        // Resolve modules: use overrides or fall back to factory
        let mut wakeword: Box<dyn WakeWordEngine> = match self.wakeword {
            Some(m) => m,
            None => factory::create_wakeword(&config)?,
        };
        let mut edge_stt: Box<dyn EdgeSttEngine> = match self.edge_stt {
            Some(m) => m,
            None => factory::create_edge_stt(&config)?,
        };
        let mut direct_response: Box<dyn DirectResponseEngine> = match self.direct_response {
            Some(m) => m,
            None => factory::create_direct_response(&config)?,
        };
        let mut audio_capture: Box<dyn AudioCaptureBackend> = match self.audio_capture {
            Some(m) => m,
            None => factory::create_audio_capture(&config)?,
        };
        let mut audio_playback: Box<dyn AudioPlaybackBackend> = match self.audio_playback {
            Some(m) => m,
            None => factory::create_audio_playback(&config)?,
        };
        let mut mixer: Box<dyn AudioMixer> = match self.mixer {
            Some(m) => m,
            None => factory::create_mixer(&config)?,
        };
        let mut face_tracker: Box<dyn FaceTracker> = match self.face_tracker {
            Some(m) => m,
            None => factory::create_face_tracker(&config)?,
        };
        let mut discovery: Box<dyn DiscoveryBackend> = match self.discovery {
            Some(m) => m,
            None => factory::create_discovery(&config)?,
        };

        // Initialize all modules
        info!("initializing modules");
        wakeword.init(&ctx).await.context("init wakeword")?;
        edge_stt.init(&ctx).await.context("init edge_stt")?;
        direct_response
            .init(&ctx)
            .await
            .context("init direct_response")?;
        audio_capture
            .init(&ctx)
            .await
            .context("init audio_capture")?;
        audio_playback
            .init(&ctx)
            .await
            .context("init audio_playback")?;
        mixer.init(&ctx).await.context("init mixer")?;
        face_tracker.init(&ctx).await.context("init face_tracker")?;
        discovery.init(&ctx).await.context("init discovery")?;

        // Initialize core infrastructure
        let run_dir = PathBuf::from("run");
        let process_manager = Arc::new(ProcessManager::new(config.process.clone(), run_dir));
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
        let binding_path = PathBuf::from("run/binding.json");
        let binding_manager = Arc::new(BindingManager::new(
            binding_path,
            config.discovery.max_bind_attempts,
            config.discovery.lockout_duration_sec,
        ));
        if let Err(e) = binding_manager.load().await {
            error!(error = %e, "failed to load binding state");
        }

        // Start all modules
        info!("starting modules");
        wakeword.start().await.context("start wakeword")?;
        edge_stt.start().await.context("start edge_stt")?;
        direct_response
            .start()
            .await
            .context("start direct_response")?;
        audio_capture.start().await.context("start audio_capture")?;
        audio_playback
            .start()
            .await
            .context("start audio_playback")?;
        mixer.start().await.context("start mixer")?;
        face_tracker.start().await.context("start face_tracker")?;
        discovery.start().await.context("start discovery")?;

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
            .context("sync discovery metadata")?;

        // Build registry
        let registry = EmbeddedModuleRegistry {
            wakeword: Arc::from(wakeword),
            edge_stt: Arc::from(edge_stt),
            direct_response: Arc::from(direct_response),
            audio_capture: Arc::from(audio_capture),
            audio_playback: Arc::from(audio_playback),
            mixer: Arc::from(mixer),
            face_tracker: Arc::from(face_tracker),
            discovery: Arc::from(discovery),
            process_manager,
            plugin_registry,
        };

        Ok(EmbeddedNode {
            registry,
            config,
            cancel_token: self.cancel_token,
            binding_manager,
            settings_manager,
        })
    }
}
