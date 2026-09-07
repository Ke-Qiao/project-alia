use std::sync::Arc;
use std::time::Duration;

use sea_orm::DatabaseConnection;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::auth::JwtKeys;
use crate::channels::{ChannelAdapter, ChannelRegistry, PipelinePolicies, spawn_pipeline};
use crate::config::MemorySection;
use crate::config::ServerConfig;
use crate::event::EventBus;
use crate::expression::ExpressionRuntime;
use crate::module::factory;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::module::registry::ModuleRegistry;
use crate::plugins::PluginStorageService;
#[cfg(feature = "plugin-home-assistant")]
use crate::plugins::home_assistant::HomeAssistantPlugin;
use crate::providers::adapter::{
    CacheBackend, ConversationBackend, EmbeddingAdapter, LlmAdapter, MemoryBackend,
    SchedulerBackend, SttAdapter, ToolRegistry, TtsAdapter,
};
use crate::providers::memory::NoopMemory;
use crate::providers::sensor::{SensorRegistry, SensorRegistryConfig};
use crate::registry::ArtifactRegistry;
use crate::scheduler::ConditionRegistry;
use crate::scheduler::action::{
    ActionRegistry, EmitEventActionExecutor, LogActionExecutor, NoopActionExecutor,
    ToolCallActionExecutor,
};
use crate::scheduler::evaluators::{
    EventFieldConditionEvaluator, EventPatternConditionEvaluator, SensorConditionEvaluator,
    TimeRangeConditionEvaluator,
};
use crate::settings::SettingsRegistry;
use crate::state::AppState;

pub struct ServerBuilder {
    config: Arc<ServerConfig>,
    db: DatabaseConnection,
    event_bus: EventBus,
    cancel_token: CancellationToken,

    // Channel adapters (multi-instance)
    channels: Vec<Box<dyn ChannelAdapter>>,

    // Provider override slots (None = use config selection). Core domain
    // systems such as memory are built in below, not selected through slots.
    minor_llm: Option<Box<dyn LlmAdapter>>,
    major_llm: Option<Box<dyn LlmAdapter>>,
    transcript_correction_llm: Option<Box<dyn LlmAdapter>>,
    memory_capture_llm: Option<Box<dyn LlmAdapter>>,
    sentence_expression_llm: Option<Box<dyn LlmAdapter>>,
    stt: Option<Box<dyn SttAdapter>>,
    tts: Option<Box<dyn TtsAdapter>>,
    embedding: Option<Box<dyn EmbeddingAdapter>>,

    extensions: Vec<Box<dyn ModuleLifecycle>>,
}

impl ServerBuilder {
    pub fn new(
        config: Arc<ServerConfig>,
        db: DatabaseConnection,
        event_bus: EventBus,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            config,
            db,
            event_bus,
            cancel_token,
            channels: Vec::new(),
            minor_llm: None,
            major_llm: None,
            transcript_correction_llm: None,
            memory_capture_llm: None,
            sentence_expression_llm: None,
            stt: None,
            tts: None,
            embedding: None,
            extensions: Vec::new(),
        }
    }

    pub fn with_llm(mut self, llm: impl LlmAdapter + 'static) -> Self {
        self.major_llm = Some(Box::new(llm));
        self
    }

    pub fn with_minor_llm(mut self, llm: impl LlmAdapter + 'static) -> Self {
        self.minor_llm = Some(Box::new(llm));
        self
    }

    pub fn with_major_llm(mut self, llm: impl LlmAdapter + 'static) -> Self {
        self.major_llm = Some(Box::new(llm));
        self
    }

    pub fn with_transcript_correction_llm(mut self, llm: impl LlmAdapter + 'static) -> Self {
        self.transcript_correction_llm = Some(Box::new(llm));
        self
    }

    pub fn with_memory_capture_llm(mut self, llm: impl LlmAdapter + 'static) -> Self {
        self.memory_capture_llm = Some(Box::new(llm));
        self
    }

    pub fn with_sentence_expression_llm(mut self, llm: impl LlmAdapter + 'static) -> Self {
        self.sentence_expression_llm = Some(Box::new(llm));
        self
    }

    pub fn with_stt(mut self, stt: impl SttAdapter + 'static) -> Self {
        self.stt = Some(Box::new(stt));
        self
    }

    pub fn with_tts(mut self, tts: impl TtsAdapter + 'static) -> Self {
        self.tts = Some(Box::new(tts));
        self
    }

    pub fn with_embedding(mut self, emb: impl EmbeddingAdapter + 'static) -> Self {
        self.embedding = Some(Box::new(emb));
        self
    }

    pub fn with_extension(mut self, ext: impl ModuleLifecycle + 'static) -> Self {
        self.extensions.push(Box::new(ext));
        self
    }

    pub fn with_channel(mut self, channel: impl ChannelAdapter + 'static) -> Self {
        self.channels.push(Box::new(channel));
        self
    }

    pub async fn build(self) -> anyhow::Result<AppState> {
        // Validate compiled capabilities and endpoints before starting any background tasks.
        let turn_detector = crate::modules::interaction::build_turn_detector(&self.config.providers.easy_turn)?;
        // Create sensor registry early so tools and modules can reference it
        let sensor_registry = Arc::new(SensorRegistry::new(SensorRegistryConfig::default()));
        let channel_registry = Arc::new(ChannelRegistry::new());
        let artifact_registry = Arc::new(ArtifactRegistry::new(self.db.clone()));
        let plugin_storage = Arc::new(PluginStorageService::new(self.db.clone()));
        let settings = SettingsRegistry::new();
        settings.seed_from_config(&self.config).await;
        let settings_path = self.config.resolve_path("settings.toml");
        if let Err(e) = settings.load_persisted(&settings_path).await {
            tracing::warn!(error = %e, "failed to load persisted settings (non-fatal)");
        }
        settings.set_persist_path(settings_path).await;
        sensor_registry.attach_db(self.db.clone()).await?;
        sensor_registry
            .attach_artifacts(artifact_registry.clone())
            .await;

        let base_ctx = ModuleContext {
            db: self.db.clone(),
            config: self.config.clone(),
            event_bus: self.event_bus.clone(),
            cancel_token: self.cancel_token.clone(),
            settings: settings.clone(),
            artifacts: artifact_registry.clone(),
            plugin_storage: plugin_storage.clone(),
            sensors: sensor_registry.clone(),
            scheduler: None,
            conditions: None,
            actions: None,
            conversation: None,
            memory: None,
            llm: None,
            minor_llm: None,
            major_llm: None,
            transcript_correction_llm: None,
            memory_capture_llm: None,
            sentence_expression_llm: None,
            embedding: None,
            cache: None,
            tools: None,
            expression: None,
        };

        // Create provider adapters from config; core services are fixed below.
        let mut minor_llm = match self.minor_llm {
            Some(llm) => llm,
            None if self.config.policies.routing.minor_llm_enabled => {
                factory::create_llm(&self.config, &self.config.llm.roles.minor)?
            }
            None => factory::create_llm(&self.config, "noop")?,
        };
        let mut major_llm = match self.major_llm {
            Some(llm) => llm,
            None if self.config.policies.routing.major_llm_enabled => {
                factory::create_llm(&self.config, &self.config.llm.roles.major)?
            }
            None => factory::create_llm(&self.config, "noop")?,
        };
        let mut transcript_correction_llm = match self.transcript_correction_llm {
            Some(llm) => Some(llm),
            None => self
                .config
                .llm
                .roles
                .transcript_correction
                .as_deref()
                .map(|profile| factory::create_llm(&self.config, profile))
                .transpose()?,
        };
        let mut memory_capture_llm = match self.memory_capture_llm {
            Some(llm) => Some(llm),
            None => self
                .config
                .llm
                .roles
                .memory_capture
                .as_deref()
                .map(|profile| factory::create_llm(&self.config, profile))
                .transpose()?,
        };
        let mut sentence_expression_llm = match self.sentence_expression_llm {
            Some(llm) => Some(llm),
            None => self
                .config
                .llm
                .roles
                .sentence_expression
                .as_deref()
                .map(|profile| factory::create_llm(&self.config, profile))
                .transpose()?,
        };
        let mut stt = self
            .stt
            .unwrap_or_else(|| factory::create_stt(&self.config.modules.stt));
        let mut tts = self
            .tts
            .unwrap_or_else(|| factory::create_tts(&self.config.modules.tts));
        let mut embedding = match self.embedding {
            Some(embedding) => embedding,
            None => factory::create_embedding(&self.config, &self.config.llm.roles.embedding)?,
        };
        let mut conversation: Box<dyn ConversationBackend> =
            Box::new(crate::providers::conversation::DatabaseConversationBackend::new());
        let mut memory = factory::create_memory(&self.config.policies.memory);
        let mut cache = factory::create_cache();
        let mut skills = factory::create_skills();
        let mut persona = factory::create_persona();
        let mut insights = factory::create_insights(self.config.policies.insights.enabled);
        let mut router = factory::create_router();
        let mut rule_engine = factory::create_rule_engine();

        // Build condition registry with built-in evaluators
        let condition_registry = Arc::new(ConditionRegistry::new());
        condition_registry
            .register(Arc::new(SensorConditionEvaluator::new(
                sensor_registry.clone(),
            )))
            .await;
        condition_registry
            .register(Arc::new(TimeRangeConditionEvaluator))
            .await;
        condition_registry
            .register(Arc::new(EventFieldConditionEvaluator))
            .await;
        condition_registry
            .register(Arc::new(EventPatternConditionEvaluator))
            .await;

        let action_registry = Arc::new(ActionRegistry::new());
        action_registry.register(Arc::new(LogActionExecutor)).await;
        action_registry
            .register(Arc::new(NoopActionExecutor::new("insights")))
            .await;
        action_registry
            .register(Arc::new(EmitEventActionExecutor::new(
                self.event_bus.clone(),
            )))
            .await;

        let mut scheduler =
            factory::create_scheduler(condition_registry.clone(), action_registry.clone());

        let mut tools =
            factory::create_tools_with_sensors(sensor_registry.clone(), channel_registry.clone());
        let expression_runtime = Arc::new(ExpressionRuntime::from_config(&self.config));
        if let Err(error) = expression_runtime.attach_db(self.db.clone()).await {
            tracing::warn!(
                error = ?error,
                "expression persistence unavailable; expression state will run in memory only"
            );
        }

        // Initialize scheduler early so other modules can reference it
        tracing::info!("initializing modules...");
        init_module(scheduler.as_mut(), &base_ctx).await;
        let scheduler: Arc<dyn SchedulerBackend> = Arc::from(scheduler);

        // Context with scheduler for modules that need it
        let ctx = ModuleContext {
            db: self.db.clone(),
            config: self.config.clone(),
            event_bus: self.event_bus.clone(),
            cancel_token: self.cancel_token.clone(),
            settings: settings.clone(),
            artifacts: artifact_registry.clone(),
            plugin_storage: plugin_storage.clone(),
            sensors: sensor_registry.clone(),
            scheduler: Some(scheduler.clone()),
            conditions: Some(condition_registry.clone()),
            actions: Some(action_registry.clone()),
            conversation: None, // Will be populated shortly but usually not needed in other modules init
            memory: None,
            llm: None,
            minor_llm: None,
            major_llm: None,
            transcript_correction_llm: None,
            memory_capture_llm: None,
            sentence_expression_llm: None,
            embedding: None,
            cache: None,
            tools: None,
            expression: Some(expression_runtime.clone()),
        };

        // Initialize remaining modules in dependency order
        init_module(embedding.as_mut(), &ctx).await;
        let embedding_arc: Arc<dyn EmbeddingAdapter> = Arc::from(embedding);
        let ctx = ModuleContext {
            embedding: Some(embedding_arc.clone()),
            ..ctx
        };
        init_module(conversation.as_mut(), &ctx).await;
        let conversation_arc: Arc<dyn ConversationBackend> = Arc::from(conversation);
        let ctx = ModuleContext {
            conversation: Some(conversation_arc.clone()),
            ..ctx
        };
        init_module(minor_llm.as_mut(), &ctx).await;
        init_module(major_llm.as_mut(), &ctx).await;
        if let Some(llm) = transcript_correction_llm.as_mut() {
            init_module(llm.as_mut(), &ctx).await;
        }
        if let Some(llm) = memory_capture_llm.as_mut() {
            init_module(llm.as_mut(), &ctx).await;
        }
        if let Some(llm) = sentence_expression_llm.as_mut() {
            init_module(llm.as_mut(), &ctx).await;
        }
        let minor_llm: Arc<dyn LlmAdapter> = Arc::from(minor_llm);
        let major_llm: Arc<dyn LlmAdapter> = Arc::from(major_llm);
        let transcript_correction_llm: Option<Arc<dyn LlmAdapter>> =
            transcript_correction_llm.map(Arc::from);
        let memory_capture_llm: Option<Arc<dyn LlmAdapter>> = memory_capture_llm.map(Arc::from);
        let sentence_expression_llm: Option<Arc<dyn LlmAdapter>> =
            sentence_expression_llm.map(Arc::from);
        let default_llm = if self.config.llm.roles.default == self.config.llm.roles.minor {
            minor_llm.clone()
        } else {
            major_llm.clone()
        };
        let ctx = ModuleContext {
            llm: Some(default_llm.clone()),
            minor_llm: Some(minor_llm.clone()),
            major_llm: Some(major_llm.clone()),
            transcript_correction_llm: transcript_correction_llm.clone(),
            memory_capture_llm: memory_capture_llm.clone(),
            sentence_expression_llm: sentence_expression_llm.clone(),
            ..ctx
        };
        init_module(stt.as_mut(), &ctx).await;
        init_module(tts.as_mut(), &ctx).await;
        init_module(memory.as_mut(), &ctx).await;
        let mut effective_memory_config = self.config.policies.memory.clone();
        if effective_memory_config.enabled
            && let Err(reason) =
                verify_memory_health(memory.as_ref(), &effective_memory_config).await
        {
            tracing::warn!(
                module = memory.name(),
                reason = %reason,
                "memory system health checks failed; disabling memory module for this server run"
            );
            effective_memory_config.enabled = false;
            memory = Box::new(NoopMemory);
        }
        let memory_arc: Arc<dyn MemoryBackend> = Arc::from(memory);
        let memory_for_ctx = effective_memory_config.enabled.then(|| memory_arc.clone());
        let ctx = ModuleContext {
            memory: memory_for_ctx,
            ..ctx
        };
        init_module(cache.as_mut(), &ctx).await;
        let cache_arc: Arc<dyn CacheBackend> = Arc::from(cache);
        let ctx = ModuleContext {
            cache: Some(cache_arc.clone()),
            ..ctx
        };
        init_module(tools.as_mut(), &ctx).await;
        let tools_arc: Arc<dyn ToolRegistry> = Arc::from(tools);
        let ctx = ModuleContext {
            tools: Some(tools_arc.clone()),
            ..ctx
        };
        action_registry
            .register(Arc::new(ToolCallActionExecutor::new(tools_arc.clone())))
            .await;

        // Initialize extensions before skills so code-backed plugin tools are
        // present when skill/tool bindings are resolved from the database.
        let mut extensions: Vec<Arc<dyn ModuleLifecycle>> = Vec::new();
        #[cfg(feature = "plugin-home-assistant")]
        let mut configured_extensions = self.extensions;
        #[cfg(not(feature = "plugin-home-assistant"))]
        let configured_extensions = self.extensions;

        #[cfg(feature = "plugin-home-assistant")]
        if self.config.plugins.home_assistant.enabled {
            configured_extensions.push(Box::new(HomeAssistantPlugin::new()));
        }
        #[cfg(not(feature = "plugin-home-assistant"))]
        if self.config.plugins.home_assistant.enabled {
            tracing::warn!(
                "home_assistant plugin is enabled in config but the plugin-home-assistant feature is disabled"
            );
        }

        for mut ext in configured_extensions {
            let name = ext.name();
            if let Err(error) = ext.init(&ctx).await {
                tracing::warn!(
                    module = name,
                    error = ?error,
                    "extension init failed; extension disabled"
                );
                continue;
            }
            extensions.push(Arc::from(ext));
        }

        init_module(skills.as_mut(), &ctx).await;
        init_module(persona.as_mut(), &ctx).await;
        init_module(router.as_mut(), &ctx).await;
        init_module(insights.as_mut(), &ctx).await;
        init_module(rule_engine.as_mut(), &ctx).await;

        // Initialize channels from config + explicit with_channel() calls
        // Create channels from [[channels]] config
        for ch_cfg in &self.config.channels {
            if !ch_cfg.enabled() {
                tracing::info!(channel_id = ch_cfg.id(), "channel disabled, skipping");
                continue;
            }
            match factory::create_channel(ch_cfg, self.event_bus.clone()) {
                Ok(channel) => {
                    register_initialized_channel(channel, &ctx, channel_registry.as_ref()).await;
                }
                Err(error) => {
                    let channel_id = ch_cfg.id().to_string();
                    let channel_type = ch_cfg.channel_type();
                    let reason = error.to_string();
                    tracing::warn!(
                        channel_type,
                        channel_id = %channel_id,
                        error = ?error,
                        "channel creation failed; channel disabled"
                    );
                    channel_registry
                        .mark_disabled(&channel_id, channel_type, reason)
                        .await;
                }
            }
        }
        // Register explicitly added channels
        for channel in self.channels {
            register_initialized_channel(channel, &ctx, channel_registry.as_ref()).await;
        }

        // Build registry
        let registry = ModuleRegistry {
            llm: default_llm,
            minor_llm,
            major_llm,
            transcript_correction_llm,
            memory_capture_llm,
            sentence_expression_llm,
            stt: Arc::from(stt),
            tts: Arc::from(tts),
            embedding: embedding_arc,
            conversation: conversation_arc,
            memory: memory_arc,
            cache: cache_arc,
            skills: Arc::from(skills),
            persona: Arc::from(persona),
            insights: Arc::from(insights),
            router: Arc::from(router),
            rule_engine: Arc::from(rule_engine),
            scheduler,
            tools: tools_arc,
            expression: expression_runtime,
            extensions,
        };

        // Start modules (scheduler first since it was init'd first)
        tracing::info!("starting modules...");
        start_module(registry.scheduler.as_ref()).await;
        start_module(registry.minor_llm.as_ref()).await;
        start_module(registry.major_llm.as_ref()).await;
        if let Some(llm) = &registry.transcript_correction_llm {
            start_module(llm.as_ref()).await;
        }
        if let Some(llm) = &registry.memory_capture_llm {
            start_module(llm.as_ref()).await;
        }
        if let Some(llm) = &registry.sentence_expression_llm {
            start_module(llm.as_ref()).await;
        }
        start_module(registry.stt.as_ref()).await;
        start_module(registry.tts.as_ref()).await;
        start_module(registry.embedding.as_ref()).await;
        start_module(registry.conversation.as_ref()).await;
        start_module(registry.memory.as_ref()).await;
        start_module(registry.cache.as_ref()).await;
        start_module(registry.skills.as_ref()).await;
        start_module(registry.persona.as_ref()).await;
        start_module(registry.router.as_ref()).await;
        start_module(registry.insights.as_ref()).await;
        start_module(registry.rule_engine.as_ref()).await;
        start_module(registry.tools.as_ref()).await;
        for ext in &registry.extensions {
            start_module(ext.as_ref()).await;
        }

        // Start channels
        for channel in channel_registry.list().await {
            let channel_id = channel.channel_id().to_string();
            if let Err(error) = channel.start().await {
                let reason = error.to_string();
                tracing::warn!(
                    channel_type = channel.channel_type(),
                    channel_id = %channel_id,
                    error = ?error,
                    "channel start failed; channel disabled"
                );
                if let Err(disable_error) = channel_registry.disable(&channel_id, reason).await {
                    tracing::warn!(
                        channel_type = channel.channel_type(),
                        channel_id = %channel_id,
                        error = ?disable_error,
                        "failed to disable channel"
                    );
                }
                continue;
            }
            tracing::info!(
                channel_type = channel.channel_type(),
                channel_id = %channel_id,
                "channel started"
            );
        }

        // Build JWT keys
        let jwt_keys = JwtKeys::new(&self.config.auth.jwt_secret);

        // Start sensor auto-cleanup
        sensor_registry.spawn_cleanup_loop(self.cancel_token.clone());
        let pipeline_policies = Arc::new(RwLock::new(PipelinePolicies::new(
            self.config.policies.conversation.clone(),
            self.config.policies.tools.clone(),
            self.config.policies.cache.clone(),
            self.config.resources.cache_policy.clone(),
            effective_memory_config,
            self.config.policies.transcript_correction.clone(),
        )));

        let state = AppState {
            turn_detector,
            db: self.db,
            channels: channel_registry,
            event_bus: self.event_bus,
            artifacts: artifact_registry,
            plugin_storage,
            modules: Arc::new(registry),
            sensors: sensor_registry,
            conditions: condition_registry,
            actions: action_registry,
            config: self.config,
            jwt_keys: Arc::new(jwt_keys),
            settings,
            pipeline_policies: pipeline_policies.clone(),
            cancel_token: self.cancel_token,
        };

        let runtime_settings = state
            .settings
            .all()
            .await
            .into_iter()
            .filter(|(_, entry)| {
                !entry
                    .schema
                    .as_ref()
                    .is_some_and(|schema| schema.requires_restart)
            })
            .map(|(key, entry)| (key, entry.value))
            .collect();
        for report in state.modules.apply_settings(&runtime_settings).await {
            if let Some(error) = report.error {
                tracing::warn!(
                    module = %report.module,
                    error = %error,
                    "failed to apply persisted runtime settings"
                );
            }
        }
        let applied_pipeline_settings = state
            .pipeline_policies
            .write()
            .await
            .apply_settings(&runtime_settings);
        if !applied_pipeline_settings.is_empty() {
            tracing::debug!(
                applied = ?applied_pipeline_settings,
                "persisted pipeline settings applied"
            );
        }

        // Spawn the Channel → AI Pipeline processing loop
        spawn_pipeline(
            &state.event_bus,
            state.modules.clone(),
            state.channels.clone(),
            state.sensors.clone(),
            state.cancel_token.clone(),
            state.config.server.debug_output,
            pipeline_policies,
        );

        Ok(state)
    }
}

async fn init_module(module: &mut dyn ModuleLifecycle, ctx: &ModuleContext) {
    let name = module.name();
    if let Err(error) = module.init(ctx).await {
        tracing::warn!(
            module = name,
            error = %error,
            "module init failed; continuing in degraded mode"
        );
        tracing::debug!(
            module = name,
            error = ?error,
            "module init failure details"
        );
    }
}

async fn verify_memory_health(
    module: &dyn ModuleLifecycle,
    config: &MemorySection,
) -> Result<(), String> {
    let attempts = config.health_check_attempts.max(1);
    let delay = Duration::from_millis(config.health_check_interval_ms);

    for attempt in 1..=attempts {
        match module.health_check().await {
            ModuleHealth::Healthy => return Ok(()),
            ModuleHealth::Degraded(reason) => {
                tracing::warn!(
                    module = module.name(),
                    attempt,
                    attempts,
                    reason = %reason,
                    "memory system health check degraded; keeping memory enabled"
                );
                return Ok(());
            }
            ModuleHealth::Unhealthy(reason) => {
                tracing::warn!(
                    module = module.name(),
                    attempt,
                    attempts,
                    reason = %reason,
                    "memory system health check failed"
                );
                if attempt == attempts {
                    return Err(reason);
                }
                if !delay.is_zero() {
                    sleep(delay).await;
                }
            }
        }
    }

    Err("memory system health check failed".to_string())
}

async fn start_module(module: &dyn ModuleLifecycle) {
    let name = module.name();
    if let Err(error) = module.start().await {
        tracing::warn!(
            module = name,
            error = %error,
            "module start failed; continuing in degraded mode"
        );
        tracing::debug!(
            module = name,
            error = ?error,
            "module start failure details"
        );
    }
}

async fn register_initialized_channel(
    mut channel: Box<dyn ChannelAdapter>,
    ctx: &ModuleContext,
    channel_registry: &ChannelRegistry,
) {
    let channel_type = channel.channel_type();
    let channel_id = channel.channel_id().to_string();

    if let Err(error) = channel.init(ctx).await {
        let reason = error.to_string();
        tracing::warn!(
            channel_type,
            channel_id = %channel_id,
            error = ?error,
            "channel init failed; channel disabled"
        );
        channel_registry
            .mark_disabled(&channel_id, channel_type, reason)
            .await;
        return;
    }

    let channel: Arc<dyn ChannelAdapter> = Arc::from(channel);
    if let Err(error) = channel_registry.register(channel).await {
        let reason = error.to_string();
        tracing::warn!(
            channel_type,
            channel_id = %channel_id,
            error = ?error,
            "channel registration failed; channel disabled"
        );
        channel_registry
            .mark_disabled(&channel_id, channel_type, reason)
            .await;
    }
}
