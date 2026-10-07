use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use sea_orm::{ConnectOptions, Database, DatabaseConnection, EntityTrait};
use tokio::sync::RwLock;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::auth::JwtKeys;
use crate::bootstrap::factory;
use crate::bootstrap::registry::ModuleRegistry;
use crate::channels::{ChannelAdapter, ChannelRegistry};
use crate::config::MemorySection;
use crate::config::ServerConfig;
use crate::event::EventBus;
use crate::expression::ExpressionRuntime;
use crate::modules::interaction::{InteractionPolicies, InteractionServices, spawn_interaction};
use crate::modules::platform::lifecycle::{
    ModuleContext, ModuleHealth, ModuleLifecycle, RuntimeCleanup, ShutdownPhase, start_module,
    startup_step,
};
use crate::plugins::PluginStorageService;
#[cfg(feature = "plugin-home-assistant")]
use crate::plugins::home_assistant::HomeAssistantPlugin;
use crate::providers::adapter::{
    ConversationBackend, EmbeddingAdapter, LlmAdapter, SttAdapter, ToolRegistry, TtsAdapter,
};
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
    db: Option<DatabaseConnection>,
    event_bus: EventBus,
    cancel_token: CancellationToken,

    // Channel adapters (multi-instance)
    channels: Vec<Box<dyn ChannelAdapter>>,

    // Provider override slots (None = use config selection). Core domain
    // systems such as memory are built in below, not selected through slots.
    minor_llm: Option<Box<dyn LlmAdapter>>,
    major_llm: Option<Box<dyn LlmAdapter>>,
    memory_capture_llm: Option<Box<dyn LlmAdapter>>,
    sentence_expression_llm: Option<Box<dyn LlmAdapter>>,
    stt: Option<Box<dyn SttAdapter>>,
    tts: Option<Box<dyn TtsAdapter>>,
    embedding: Option<Box<dyn EmbeddingAdapter>>,

    extensions: Vec<Box<dyn ModuleLifecycle>>,
}

/// An owned startup operation. Dropping its ready waiter cancels construction;
/// the independent owner still completes rollback and reports through the handle.
pub struct ServerStartup {
    cancel: CancellationToken,
    cleanup: Arc<RuntimeCleanup>,
    task: Option<tokio::task::JoinHandle<anyhow::Result<crate::bootstrap::ServerRuntime>>>,
    handed_off: bool,
}
impl ServerStartup {
    pub fn shutdown_handle(&self) -> crate::modules::platform::lifecycle::ShutdownHandle {
        self.cleanup.handle()
    }
    pub async fn wait(mut self) -> anyhow::Result<crate::bootstrap::ServerRuntime> {
        let result = self
            .task
            .as_mut()
            .expect("startup waiter already consumed")
            .await
            .context("server startup owner failed")?;
        self.task.take();
        self.handed_off = result.is_ok();
        result
    }
}
impl Drop for ServerStartup {
    fn drop(&mut self) {
        if !self.handed_off {
            self.cancel.cancel();
        }
        // Dropping the JoinHandle does not abort the independent startup owner.
        self.task.take();
    }
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
            db: Some(db),
            event_bus,
            cancel_token,
            channels: Vec::new(),
            minor_llm: None,
            major_llm: None,
            memory_capture_llm: None,
            sentence_expression_llm: None,
            stt: None,
            tts: None,
            embedding: None,
            extensions: Vec::new(),
        }
    }

    /// Create an instance whose PostgreSQL pool is owned and closed by the runtime.
    pub fn from_config(config: Arc<ServerConfig>, cancel_token: CancellationToken) -> Self {
        let mut builder = Self::new(
            config,
            DatabaseConnection::Disconnected,
            EventBus::new(1024),
            cancel_token,
        );
        builder.db = None;
        builder
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

    /// Begin construction while retaining a handle that can outlive the ready waiter.
    pub fn start_build(mut self) -> ServerStartup {
        self.cancel_token = self.cancel_token.child_token();
        let cancel = self.cancel_token.clone();
        let cleanup = RuntimeCleanup::new(
            cancel.clone(),
            Duration::from_millis(self.config.server.shutdown_grace_ms),
        );
        let owner = cleanup.clone();
        let task = tokio::spawn(async move {
            let cancel = self.cancel_token.clone();
            let result = startup_step(
                &cancel,
                "assembling server runtime",
                self.build_owned(owner.clone()),
            )
            .await;
            if let Err(error) = result {
                owner.record_failure(format!("startup: {error:#}"));
                return match owner.start().wait_until(owner.deadline()).await {
                    Ok(()) => Err(error),
                    Err(cleanup) => {
                        Err(cleanup.context(format!("server startup failed: {error:#}")))
                    }
                };
            }
            result
        });
        ServerStartup {
            cancel,
            cleanup,
            task: Some(task),
            handed_off: false,
        }
    }

    pub async fn build(self) -> anyhow::Result<crate::bootstrap::ServerRuntime> {
        self.start_build().wait().await
    }

    async fn build_owned(
        mut self,
        cleanup: Arc<RuntimeCleanup>,
    ) -> anyhow::Result<crate::bootstrap::ServerRuntime> {
        let requested_config = self.config.clone();
        let (wiring, config) = crate::bootstrap::RuntimeWiring::bind(
            self.config.clone(),
            self.cancel_token.clone(),
            cleanup.clone(),
        )
        .await?;
        self.config = config;
        let db = match self.db.take() {
            Some(db) => db,
            None => {
                let mut options = ConnectOptions::new(&self.config.database.url);
                options
                    .max_connections(self.config.database.max_connections)
                    .min_connections(self.config.database.min_connections);
                let db = startup_step(&self.cancel_token, "connecting to PostgreSQL", async {
                    Database::connect(options).await.map_err(Into::into)
                })
                .await?;
                let pool = db.clone();
                cleanup.add("postgresql", ShutdownPhase::Database, move || {
                    let pool = pool.clone();
                    async move { pool.close().await.map_err(Into::into) }
                });
                db
            }
        };
        startup_step(
            &self.cancel_token,
            "migrating PostgreSQL",
            crate::modules::platform::migrations::run(&db),
        )
        .await?;

        // Owners are explicit configuration, never synthesized from channel IDs.
        // Validate before starting any module or creating background work.
        let owners = self
            .config
            .channels
            .iter()
            .filter_map(|channel| channel.owner_user_id())
            .chain(
                self.channels
                    .iter()
                    .filter_map(|channel| channel.owner_user_id()),
            )
            .collect::<std::collections::HashSet<_>>();
        for owner in owners {
            let user = crate::entity::user::Entity::find_by_id(owner)
                .one(&db)
                .await?;
            anyhow::ensure!(
                user.is_some_and(|user| user.status == "active"),
                "channel owner_user_id {owner} must reference an existing active account (identity_unlinked)"
            );
        }
        // Validate compiled capabilities and endpoints before starting any background tasks.
        let turn_detector = factory::create_turn_detector(&self.config.providers.easy_turn)?;
        // Create sensor registry early so tools and modules can reference it
        let sensor_registry = Arc::new(SensorRegistry::new(SensorRegistryConfig::default()));
        let channel_registry = Arc::new(ChannelRegistry::new());
        let artifact_registry = Arc::new(ArtifactRegistry::new(db.clone()));
        let plugin_storage = Arc::new(PluginStorageService::new(db.clone()));
        let settings = SettingsRegistry::new();
        settings.seed_from_config(&self.config).await;
        let settings_path = self.config.resolve_path("settings.toml");
        settings
            .load_persisted(&settings_path)
            .await
            .context("loading persisted server settings")?;
        settings.set_persist_path(settings_path).await;
        let mut initial_policies = InteractionPolicies::new(
            self.config.policies.conversation.clone(),
            self.config.policies.tools.clone(),
            self.config.policies.cache.clone(),
            self.config.resources.cache_policy.clone(),
            self.config.policies.memory.clone(),
        );
        let saved_settings = settings
            .all()
            .await
            .into_iter()
            .map(|(key, entry)| (key, entry.value))
            .collect();
        initial_policies.apply_settings(&saved_settings);
        sensor_registry.attach_db(db.clone()).await?;
        sensor_registry
            .attach_artifacts(artifact_registry.clone())
            .await;

        let base_ctx = ModuleContext {
            db: db.clone(),
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
            memory_capture_llm: None,
            sentence_expression_llm: None,
            embedding: None,
            cache: None,
            tools: None,
            expression: None,
        };

        // Create provider adapters from config; core services are fixed below.
        let minor_llm = match self.minor_llm {
            Some(llm) => llm,
            None if self.config.policies.routing.minor_llm_enabled => {
                factory::create_llm(&self.config, &self.config.llm.roles.minor)?
            }
            None => factory::create_llm(&self.config, "noop")?,
        };
        let major_llm = match self.major_llm {
            Some(llm) => llm,
            None if self.config.policies.routing.major_llm_enabled => {
                factory::create_llm(&self.config, &self.config.llm.roles.major)?
            }
            None => factory::create_llm(&self.config, "noop")?,
        };
        let memory_capture_llm = match self.memory_capture_llm {
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
        let sentence_expression_llm = match self.sentence_expression_llm {
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
        let stt = match self.stt {
            Some(module) => module,
            None => factory::create_stt(&self.config.modules.stt)?,
        };
        let tts = match self.tts {
            Some(module) => module,
            None => factory::create_tts(&self.config.modules.tts)?,
        };
        let embedding = match self.embedding {
            Some(embedding) => embedding,
            None => factory::create_embedding(&self.config, &self.config.llm.roles.embedding)?,
        };
        let conversation: Box<dyn ConversationBackend> =
            Box::new(crate::providers::conversation::DatabaseConversationBackend::new());
        // Memory is a canonical service; recall policy never disables storage or erasure.
        let memory = Box::new(crate::modules::memory::MemoryService::new());
        let cache = factory::create_cache();
        let skills = factory::create_skills();
        let persona = factory::create_persona();
        let insights = factory::create_insights(self.config.policies.insights.enabled)?;
        let router = factory::create_router();
        let rule_engine = factory::create_rule_engine();

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

        let scheduler =
            factory::create_scheduler(condition_registry.clone(), action_registry.clone());

        let tools =
            factory::create_tools_with_sensors(sensor_registry.clone(), channel_registry.clone());
        let expression_runtime = Arc::new(ExpressionRuntime::from_config(&self.config));
        let expression_owner = expression_runtime.clone();
        cleanup.add(
            "expression_persistence",
            ShutdownPhase::Resources,
            move || {
                let expression_owner = expression_owner.clone();
                async move { expression_owner.shutdown().await }
            },
        );
        expression_runtime
            .attach_db(db.clone())
            .await
            .context("attaching expression persistence")?;

        // Initialize scheduler early so other modules can reference it
        tracing::info!("initializing modules...");
        let scheduler = cleanup
            .initialize(scheduler, &base_ctx, ShutdownPhase::Producers)
            .await?;

        // Context with scheduler for modules that need it
        let ctx = ModuleContext {
            db: db.clone(),
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
            memory_capture_llm: None,
            sentence_expression_llm: None,
            embedding: None,
            cache: None,
            tools: None,
            expression: Some(expression_runtime.clone()),
        };

        // Initialize remaining modules in dependency order
        let embedding_arc = cleanup
            .initialize(embedding, &ctx, ShutdownPhase::Resources)
            .await?;
        let mut policy_seed = crate::modules::memory::context::ContextPolicy::from_config(
            &initial_policies.memory,
            &initial_policies.conversation,
        )?
        .with_cache(&initial_policies.cache)?;
        policy_seed.appraisal =
            crate::modules::memory::context::AppraisalPolicy::from_config(&self.config);
        // Profile identity and settings parsing share the provider's actual
        // request snapshot. No embedding request is needed during this step.
        let actual_profile = embedding_arc
            .preview_profile_settings(&saved_settings)
            .await?;
        let actual_profile_id = actual_profile.as_ref().map(|p| p.id()).transpose()?;
        policy_seed.cache_embedding_profile_id = actual_profile_id.clone();
        policy_seed.active_embedding_profile = actual_profile_id.clone();
        let persisted_policy = startup_step(&self.cancel_token, "loading context policy", async {
            crate::modules::memory::context::ContextPolicyStore::new(db.clone())
                .initialize_or_load(policy_seed)
                .await
        })
        .await?;
        embedding_arc.apply_settings(&saved_settings).await?;
        if persisted_policy.policy.cache_semantic_enabled
            && persisted_policy.policy.cache_embedding_profile_id != actual_profile_id
        {
            tracing::warn!(
                "configured embedding profile differs from persistent policy; semantic cache remains unavailable until an explicit policy update"
            );
        }
        if persisted_policy.policy.memory_enabled
            && persisted_policy.policy.active_embedding_profile != actual_profile_id
        {
            tracing::warn!(
                "configured embedding profile differs from persistent Memory policy; canonical keyword retrieval remains available until an explicit profile update"
            );
        }
        let policy_settings = crate::settings::context_policy::projection(&persisted_policy.policy);
        settings.batch_update(&policy_settings).await;
        initial_policies.apply_settings(&policy_settings);
        // The database wins over a previous process's TOML projection before
        // memory/cache producers or Interaction can start work.
        let mut effective_config = (*self.config).clone();
        effective_config.policies.memory = initial_policies.memory.clone();
        effective_config.policies.conversation = initial_policies.conversation.clone();
        effective_config.policies.cache = initial_policies.cache.clone();
        self.config = Arc::new(effective_config);
        let context = Arc::new(crate::modules::memory::context::ContextService::new(
            db.clone(),
            embedding_arc.clone(),
        ));
        let ctx = ModuleContext {
            config: self.config.clone(),
            embedding: Some(embedding_arc.clone()),
            ..ctx
        };
        let conversation_arc = cleanup
            .initialize(conversation, &ctx, ShutdownPhase::Resources)
            .await?;
        let ctx = ModuleContext {
            conversation: Some(conversation_arc.clone()),
            ..ctx
        };
        let minor_llm = cleanup
            .initialize(minor_llm, &ctx, ShutdownPhase::Resources)
            .await?;
        let major_llm = cleanup
            .initialize(major_llm, &ctx, ShutdownPhase::Resources)
            .await?;
        let memory_capture_llm = match memory_capture_llm {
            Some(module) => Some(
                cleanup
                    .initialize(module, &ctx, ShutdownPhase::Resources)
                    .await?,
            ),
            None => None,
        };
        let sentence_expression_llm = match sentence_expression_llm {
            Some(module) => Some(
                cleanup
                    .initialize(module, &ctx, ShutdownPhase::Resources)
                    .await?,
            ),
            None => None,
        };
        let default_llm = if self.config.llm.roles.default == self.config.llm.roles.minor {
            minor_llm.clone()
        } else {
            major_llm.clone()
        };
        let ctx = ModuleContext {
            llm: Some(default_llm.clone()),
            minor_llm: Some(minor_llm.clone()),
            major_llm: Some(major_llm.clone()),
            memory_capture_llm: memory_capture_llm.clone(),
            sentence_expression_llm: sentence_expression_llm.clone(),
            ..ctx
        };
        let stt = cleanup
            .initialize(stt, &ctx, ShutdownPhase::Resources)
            .await?;
        let tts = cleanup
            .initialize(tts, &ctx, ShutdownPhase::Resources)
            .await?;
        let memory_arc = cleanup
            .initialize(memory, &ctx, ShutdownPhase::Resources)
            .await?;
        let effective_memory_config = self.config.policies.memory.clone();
        if effective_memory_config.enabled {
            startup_step(&self.cancel_token, "checking configured memory", async {
                verify_memory_health(memory_arc.as_ref(), &effective_memory_config)
                    .await
                    .map_err(anyhow::Error::msg)
            })
            .await?;
        }
        let memory_for_ctx = Some(memory_arc.clone());
        let ctx = ModuleContext {
            memory: memory_for_ctx,
            ..ctx
        };
        let cache_arc = cleanup
            .initialize(cache, &ctx, ShutdownPhase::Resources)
            .await?;
        let ctx = ModuleContext {
            cache: Some(cache_arc.clone()),
            ..ctx
        };
        let raw_tools = cleanup
            .initialize(tools, &ctx, ShutdownPhase::Resources)
            .await?;
        let execution = crate::modules::execution::ExecutionService::new(
            crate::modules::execution::repository::ExecutionRepository::new(db.clone()),
            raw_tools,
            &self.config.policies.tools,
            &self.cancel_token,
        )
        .await?;
        let effect_owner = execution.clone();
        cleanup.add("execution", ShutdownPhase::Execution, move || {
            let effect_owner = effect_owner.clone();
            async move {
                let mut failure = None;
                loop {
                    let result = effect_owner.shutdown().await;
                    if let Err(error) = result {
                        failure = Some(error);
                    }
                    if !effect_owner.shutdown_pending() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                failure.map_or(Ok(()), Err)
            }
        });
        let tools_arc: Arc<dyn ToolRegistry> = Arc::new(
            crate::modules::execution::ExecutionToolRegistry::new(execution.clone()),
        );
        let delivery = crate::modules::delivery::DeliveryService::new(
            crate::modules::delivery::repository::DeliveryRepository::new(db.clone()),
            &self.cancel_token,
        );
        let delivery_owner = delivery.clone();
        cleanup.add("delivery", ShutdownPhase::Delivery, move || {
            let delivery_owner = delivery_owner.clone();
            async move {
                let mut failure = None;
                loop {
                    if let Err(error) = delivery_owner.shutdown().await {
                        failure = Some(error);
                    }
                    if !delivery_owner.shutdown_pending() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                failure.map_or(Ok(()), Err)
            }
        });
        let delivery_admission = delivery.clone();
        cleanup.add("delivery_admission", ShutdownPhase::Ingress, move || {
            let delivery_admission = delivery_admission.clone();
            async move {
                delivery_admission.close_admission();
                Ok(())
            }
        });
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
        anyhow::ensure!(
            !self.config.plugins.home_assistant.enabled,
            "configured home_assistant requires plugin-home-assistant feature"
        );
        for ext in configured_extensions {
            extensions.push(
                cleanup
                    .initialize(ext, &ctx, ShutdownPhase::Resources)
                    .await?,
            );
        }
        let skills = cleanup
            .initialize(skills, &ctx, ShutdownPhase::Resources)
            .await?;
        let persona = cleanup
            .initialize(persona, &ctx, ShutdownPhase::Resources)
            .await?;
        let router = cleanup
            .initialize(router, &ctx, ShutdownPhase::Resources)
            .await?;
        let insights = cleanup
            .initialize(insights, &ctx, ShutdownPhase::Producers)
            .await?;
        let rule_engine = cleanup
            .initialize(rule_engine, &ctx, ShutdownPhase::Producers)
            .await?;
        let channels = channel_registry.clone();
        cleanup.add("channels", ShutdownPhase::Channels, move || {
            let channels = channels.clone();
            async move { channels.shutdown().await }
        });
        let channels = channel_registry.clone();
        cleanup.add("channel_admission", ShutdownPhase::Ingress, move || {
            let channels = channels.clone();
            async move {
                channels.close_admission();
                Ok(())
            }
        });
        // Initialize channels from config + explicit with_channel() calls
        // Create channels from [[channels]] config
        for ch_cfg in &self.config.channels {
            if !ch_cfg.enabled() {
                tracing::info!(channel_id = ch_cfg.id(), "channel disabled, skipping");
                continue;
            }
            let channel = factory::create_channel(ch_cfg, self.event_bus.clone())?;
            register_initialized_channel(channel, &ctx, channel_registry.as_ref(), &cleanup)
                .await?;
        }
        for channel in self.channels {
            register_initialized_channel(channel, &ctx, channel_registry.as_ref(), &cleanup)
                .await?;
        }

        let voice_initiatives =
            crate::modules::interaction::initiative::VoiceInitiativeHandle::default();
        let companion = Arc::new(
            crate::modules::companion::CompanionService::new(
                db.clone(),
                voice_initiatives.clone(),
                self.cancel_token.clone(),
            )
            .with_live_outputs(crate::modules::companion::LiveOutputs {
                context: context.clone(),
                delivery: delivery.clone(),
                conversation: conversation_arc.clone(),
                tts: tts.clone(),
            }),
        );
        let companion_cleanup = companion.clone();
        cleanup.add("companion", ShutdownPhase::Producers, move || {
            let companion = companion_cleanup.clone();
            async move { companion.stop().await }
        });
        let companion_admission = companion.clone();
        cleanup.add("companion_admission", ShutdownPhase::Ingress, move || {
            let companion = companion_admission.clone();
            async move {
                companion.close();
                Ok(())
            }
        });

        // Build registry
        let registry = ModuleRegistry {
            cleanup: cleanup.clone(),
            llm: default_llm,
            minor_llm,
            major_llm,
            memory_capture_llm,
            sentence_expression_llm,
            stt,
            tts,
            embedding: embedding_arc,
            conversation: conversation_arc,
            memory: memory_arc,
            context,
            companion,
            cache: cache_arc,
            skills,
            persona,
            insights,
            router,
            rule_engine,
            scheduler,
            tools: tools_arc,
            expression: expression_runtime,
            delivery: delivery.clone(),
            extensions,
        };

        // Build JWT keys
        let jwt_keys = JwtKeys::new(&self.config.auth.jwt_secret);

        // Start sensor auto-cleanup
        let sensors = sensor_registry.clone();
        cleanup.add("sensor_cleanup", ShutdownPhase::Producers, move || {
            let sensors = sensors.clone();
            async move { sensors.stop_cleanup_loop().await }
        });
        let interaction_policies = Arc::new(RwLock::new(InteractionPolicies::new(
            self.config.policies.conversation.clone(),
            self.config.policies.tools.clone(),
            self.config.policies.cache.clone(),
            self.config.resources.cache_policy.clone(),
            effective_memory_config,
        )));

        let (voice_input, voice_input_rx) = crate::modules::interaction::voice_input::channel();
        let state = AppState {
            runtime_tasks: wiring.tasks.clone(),
            execution,
            delivery,
            voice_input,
            voice_initiatives,
            interaction_task: Arc::default(),
            turn_detector,
            db,
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
            interaction_policies: interaction_policies.clone(),
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
                anyhow::bail!(
                    "failed to apply persisted settings to {}: {error}",
                    report.module
                );
            }
        }
        let applied_interaction_settings = state
            .interaction_policies
            .write()
            .await
            .apply_settings(&runtime_settings);
        if !applied_interaction_settings.is_empty() {
            tracing::debug!(
                applied = ?applied_interaction_settings,
                "persisted interaction settings applied"
            );
        }

        // Start downstream adapters before any durable execution can dispatch.
        tracing::info!("starting modules...");
        start_module(state.modules.minor_llm.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.major_llm.as_ref(), &state.cancel_token).await?;
        if let Some(llm) = &state.modules.memory_capture_llm {
            start_module(llm.as_ref(), &state.cancel_token).await?;
        }
        if let Some(llm) = &state.modules.sentence_expression_llm {
            start_module(llm.as_ref(), &state.cancel_token).await?;
        }
        start_module(state.modules.stt.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.tts.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.embedding.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.conversation.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.memory.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.cache.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.skills.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.persona.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.router.as_ref(), &state.cancel_token).await?;
        for ext in &state.modules.extensions {
            start_module(ext.as_ref(), &state.cancel_token).await?;
        }
        startup_step(
            &state.cancel_token,
            "starting delivery supervision",
            state.delivery.start(),
        )
        .await?;

        // Start the Interaction owner after its downstream services are ready.
        let interaction_owner = state.interaction_task.clone();
        cleanup.add("interaction", ShutdownPhase::Producers, move || {
            let interaction_owner = interaction_owner.clone();
            async move {
                let mut slot = interaction_owner.lock().await;
                if let Some(task) = slot.as_mut() {
                    task.await.context("joining interaction")?;
                    slot.take();
                }
                Ok(())
            }
        });
        // Acquire the slot before spawning: a cancelled lock wait must not drop
        // the only handle of an already-running Interaction runtime.
        let mut interaction_slot = state.interaction_task.lock().await;
        let interaction_services = Arc::new(InteractionServices {
            llm: state.modules.llm.clone(),
            minor_llm: state.modules.minor_llm.clone(),
            major_llm: state.modules.major_llm.clone(),
            memory_capture_llm: state.modules.memory_capture_llm.clone(),
            sentence_expression_llm: state.modules.sentence_expression_llm.clone(),
            tts: state.modules.tts.clone(),
            embedding: state.modules.embedding.clone(),
            conversation: state.modules.conversation.clone(),
            memory: state.modules.memory.clone(),
            context: state.modules.context.clone(),
            cache: state.modules.cache.clone(),
            skills: state.modules.skills.clone(),
            persona: state.modules.persona.clone(),
            router: state.modules.router.clone(),
            tools: state.modules.tools.clone(),
            expression: state.modules.expression.clone(),
            delivery: state.modules.delivery.clone(),
        });
        let interaction_task = spawn_interaction(
            &state.event_bus,
            interaction_services,
            state.channels.clone(),
            state.sensors.clone(),
            state.cancel_token.clone(),
            state.config.server.debug_output,
            interaction_policies,
            voice_input_rx,
        );
        *interaction_slot = Some(interaction_task);
        drop(interaction_slot);

        for channel in state.channels.list().await {
            startup_step(
                &state.cancel_token,
                "starting channel",
                state.channels.start(channel.channel_id()),
            )
            .await?;
        }
        start_module(state.modules.tools.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.insights.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.scheduler.as_ref(), &state.cancel_token).await?;
        start_module(state.modules.rule_engine.as_ref(), &state.cancel_token).await?;
        startup_step(
            &state.cancel_token,
            "starting companion shadow supervision",
            state.modules.companion.start(),
        )
        .await?;
        state
            .sensors
            .spawn_cleanup_loop(state.cancel_token.clone())
            .await?;

        crate::bootstrap::ServerRuntime::assembled(state, requested_config, cleanup, wiring).await
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

async fn register_initialized_channel(
    channel: Box<dyn ChannelAdapter>,
    ctx: &ModuleContext,
    registry: &ChannelRegistry,
    cleanup: &RuntimeCleanup,
) -> anyhow::Result<()> {
    let (channel, transferred) = cleanup
        .initialize_owned(channel, ctx, ShutdownPhase::Channels)
        .await?;
    registry.register(channel).await?;
    transferred.store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}
