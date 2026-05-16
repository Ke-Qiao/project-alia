use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

use crate::auth::JwtKeys;
use crate::channels::{ChannelAdapter, ChannelRegistry, spawn_pipeline};
use crate::config::ServerConfig;
use crate::event::EventBus;
use crate::module::factory;
use crate::module::lifecycle::{ModuleContext, ModuleLifecycle};
use crate::module::registry::ModuleRegistry;
use crate::providers::adapter::{
    CacheBackend, ConversationBackend, EmbeddingAdapter, LlmAdapter, MeditationStrategy,
    MemoryBackend, PersonaProvider, RouterStrategy, RuleEvaluator, SchedulerBackend, SkillProvider,
    SttAdapter, ToolRegistry, TtsAdapter,
};
use crate::providers::sensor::{SensorRegistry, SensorRegistryConfig};
use crate::scheduler::ConditionRegistry;
use crate::scheduler::evaluators::{SensorConditionEvaluator, TimeRangeConditionEvaluator};
use crate::settings::SettingsRegistry;
use crate::state::AppState;

pub struct ServerBuilder {
    config: Arc<ServerConfig>,
    db: DatabaseConnection,
    event_bus: EventBus,
    cancel_token: CancellationToken,

    // Channel adapters (multi-instance)
    channels: Vec<Box<dyn ChannelAdapter>>,

    // Module slots (None = use config default)
    llm: Option<Box<dyn LlmAdapter>>,
    stt: Option<Box<dyn SttAdapter>>,
    tts: Option<Box<dyn TtsAdapter>>,
    embedding: Option<Box<dyn EmbeddingAdapter>>,
    conversation: Option<Box<dyn ConversationBackend>>,
    memory: Option<Box<dyn MemoryBackend>>,
    cache: Option<Box<dyn CacheBackend>>,
    skills: Option<Box<dyn SkillProvider>>,
    persona: Option<Box<dyn PersonaProvider>>,
    meditation: Option<Box<dyn MeditationStrategy>>,
    router: Option<Box<dyn RouterStrategy>>,
    rule_engine: Option<Box<dyn RuleEvaluator>>,
    scheduler: Option<Box<dyn SchedulerBackend>>,
    tools: Option<Box<dyn ToolRegistry>>,

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
            llm: None,
            stt: None,
            tts: None,
            embedding: None,
            conversation: None,
            memory: None,
            cache: None,
            skills: None,
            persona: None,
            meditation: None,
            router: None,
            rule_engine: None,
            scheduler: None,
            tools: None,
            extensions: Vec::new(),
        }
    }

    pub fn with_llm(mut self, llm: impl LlmAdapter + 'static) -> Self {
        self.llm = Some(Box::new(llm));
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

    pub fn with_conversation(mut self, conv: impl ConversationBackend + 'static) -> Self {
        self.conversation = Some(Box::new(conv));
        self
    }

    pub fn with_memory(mut self, mem: impl MemoryBackend + 'static) -> Self {
        self.memory = Some(Box::new(mem));
        self
    }

    pub fn with_cache(mut self, cache: impl CacheBackend + 'static) -> Self {
        self.cache = Some(Box::new(cache));
        self
    }

    pub fn with_skills(mut self, skills: impl SkillProvider + 'static) -> Self {
        self.skills = Some(Box::new(skills));
        self
    }

    pub fn with_persona(mut self, persona: impl PersonaProvider + 'static) -> Self {
        self.persona = Some(Box::new(persona));
        self
    }

    pub fn with_meditation(mut self, med: impl MeditationStrategy + 'static) -> Self {
        self.meditation = Some(Box::new(med));
        self
    }

    pub fn with_router(mut self, router: impl RouterStrategy + 'static) -> Self {
        self.router = Some(Box::new(router));
        self
    }

    pub fn with_rules(mut self, rules: impl RuleEvaluator + 'static) -> Self {
        self.rule_engine = Some(Box::new(rules));
        self
    }

    pub fn with_scheduler(mut self, sched: impl SchedulerBackend + 'static) -> Self {
        self.scheduler = Some(Box::new(sched));
        self
    }

    pub fn with_tools(mut self, tools: impl ToolRegistry + 'static) -> Self {
        self.tools = Some(Box::new(tools));
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
        // Create sensor registry early so tools and modules can reference it
        let sensor_registry = Arc::new(SensorRegistry::new(SensorRegistryConfig::default()));
        let channel_registry = Arc::new(ChannelRegistry::new());

        let base_ctx = ModuleContext {
            db: self.db.clone(),
            config: self.config.clone(),
            event_bus: self.event_bus.clone(),
            cancel_token: self.cancel_token.clone(),
            scheduler: None,
            conditions: None,
            conversation: None,
        };

        // Create modules: use override or create from config
        let mut llm = self
            .llm
            .unwrap_or_else(|| factory::create_llm(&self.config.modules.llm));
        let mut stt = self
            .stt
            .unwrap_or_else(|| factory::create_stt(&self.config.modules.stt));
        let mut tts = self
            .tts
            .unwrap_or_else(|| factory::create_tts(&self.config.modules.tts));
        let mut embedding = self
            .embedding
            .unwrap_or_else(|| factory::create_embedding(&self.config.modules.embedding));
        let mut conversation = self.conversation.unwrap_or_else(|| {
            Box::new(crate::providers::conversation::DatabaseConversationBackend::new())
        });
        let mut memory = self
            .memory
            .unwrap_or_else(|| factory::create_memory(&self.config.modules.memory));
        let mut cache = self
            .cache
            .unwrap_or_else(|| factory::create_cache(&self.config.modules.cache));
        let mut skills = self
            .skills
            .unwrap_or_else(|| factory::create_skills(&self.config.modules.skills));
        let mut persona = self
            .persona
            .unwrap_or_else(|| factory::create_persona(&self.config.modules.persona));
        let mut meditation = self
            .meditation
            .unwrap_or_else(|| factory::create_meditation(&self.config.modules.meditation));
        let mut router = self
            .router
            .unwrap_or_else(|| factory::create_router(&self.config.modules.router));
        let mut rule_engine = self
            .rule_engine
            .unwrap_or_else(|| factory::create_rule_engine(&self.config.modules.rule_engine));

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

        let mut scheduler = self.scheduler.unwrap_or_else(|| {
            factory::create_scheduler(&self.config.modules.scheduler, condition_registry.clone())
        });

        let mut tools = self.tools.unwrap_or_else(|| {
            factory::create_tools_with_sensors(
                &self.config.modules.tools,
                sensor_registry.clone(),
                channel_registry.clone(),
            )
        });

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
            scheduler: Some(scheduler.clone()),
            conditions: Some(condition_registry.clone()),
            conversation: None, // Will be populated shortly but usually not needed in other modules init
        };

        // Initialize remaining modules in dependency order
        init_module(embedding.as_mut(), &ctx).await;
        init_module(conversation.as_mut(), &ctx).await;
        let conversation_arc: Arc<dyn ConversationBackend> = Arc::from(conversation);
        let ctx = ModuleContext {
            conversation: Some(conversation_arc.clone()),
            ..ctx
        };
        init_module(llm.as_mut(), &ctx).await;
        init_module(stt.as_mut(), &ctx).await;
        init_module(tts.as_mut(), &ctx).await;
        init_module(memory.as_mut(), &ctx).await;
        init_module(cache.as_mut(), &ctx).await;
        init_module(skills.as_mut(), &ctx).await;
        init_module(persona.as_mut(), &ctx).await;
        init_module(router.as_mut(), &ctx).await;
        init_module(meditation.as_mut(), &ctx).await;
        init_module(rule_engine.as_mut(), &ctx).await;
        init_module(tools.as_mut(), &ctx).await;

        // Initialize extensions
        let mut extensions: Vec<Arc<dyn ModuleLifecycle>> = Vec::new();
        for mut ext in self.extensions {
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

        // Initialize channels from config + explicit with_channel() calls
        // Create channels from [[channels]] config
        for ch_cfg in &self.config.channels {
            if !ch_cfg.enabled() {
                tracing::info!(channel_id = ch_cfg.id(), "channel disabled, skipping");
                continue;
            }
            let channel: Box<dyn ChannelAdapter> =
                factory::create_channel(ch_cfg, self.event_bus.clone());
            register_initialized_channel(channel, &ctx, channel_registry.as_ref()).await;
        }
        // Register explicitly added channels
        for channel in self.channels {
            register_initialized_channel(channel, &ctx, channel_registry.as_ref()).await;
        }

        // Build registry
        let registry = ModuleRegistry {
            llm: Arc::from(llm),
            stt: Arc::from(stt),
            tts: Arc::from(tts),
            embedding: Arc::from(embedding),
            conversation: conversation_arc,
            memory: Arc::from(memory),
            cache: Arc::from(cache),
            skills: Arc::from(skills),
            persona: Arc::from(persona),
            meditation: Arc::from(meditation),
            router: Arc::from(router),
            rule_engine: Arc::from(rule_engine),
            scheduler,
            tools: Arc::from(tools),
            extensions,
        };

        // Start modules (scheduler first since it was init'd first)
        tracing::info!("starting modules...");
        start_module(registry.scheduler.as_ref()).await;
        start_module(registry.llm.as_ref()).await;
        start_module(registry.stt.as_ref()).await;
        start_module(registry.tts.as_ref()).await;
        start_module(registry.embedding.as_ref()).await;
        start_module(registry.conversation.as_ref()).await;
        start_module(registry.memory.as_ref()).await;
        start_module(registry.cache.as_ref()).await;
        start_module(registry.skills.as_ref()).await;
        start_module(registry.persona.as_ref()).await;
        start_module(registry.router.as_ref()).await;
        start_module(registry.meditation.as_ref()).await;
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

        // Build settings registry and seed from config
        let settings = SettingsRegistry::new();
        settings.seed_from_config(&self.config).await;
        let settings_path = self.config.resolve_path("settings.toml");
        if let Err(e) = settings.load_persisted(&settings_path).await {
            tracing::warn!(error = %e, "failed to load persisted settings (non-fatal)");
        }
        settings.set_persist_path(settings_path).await;

        // Start sensor auto-cleanup
        sensor_registry.spawn_cleanup_loop(self.cancel_token.clone());

        let state = AppState {
            db: self.db,
            channels: channel_registry,
            event_bus: self.event_bus,
            modules: Arc::new(registry),
            sensors: sensor_registry,
            conditions: condition_registry,
            config: self.config,
            jwt_keys: Arc::new(jwt_keys),
            settings,
            cancel_token: self.cancel_token,
        };

        // Spawn the Channel → AI Pipeline processing loop
        spawn_pipeline(
            &state.event_bus,
            state.modules.clone(),
            state.channels.clone(),
            state.sensors.clone(),
            state.cancel_token.clone(),
            state.config.server.debug_output,
            state.config.tools.clone(),
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
