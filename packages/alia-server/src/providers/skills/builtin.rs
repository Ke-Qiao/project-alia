//! Builtin Skills Provider
//!
//! DB-backed skill provider that loads skill definitions from PostgreSQL.
//! On first init, seeds default built-in skills if the table is empty.
//! Supports keyword-based intent matching with priority-based conflict resolution.
//!
//! Skills extend what the LLM **knows how to do well**. Each skill carries:
//! - Intent patterns for matching user messages
//! - A prompt template providing domain-specific LLM instructions
//! - Explicit tool bindings through the `skill_tools` junction table

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use tokio::sync::RwLock;
use uuid::Uuid;

use super::markdown;
use crate::entity::{skill, skill_tool, tool};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    PluginSkillDefinition, SkillInfo, SkillMatch, SkillProvider, SkillResolution,
};
use crate::registry::{ArtifactRef, ArtifactRegistry, KIND_SKILL};

/// A loaded skill with its metadata for in-memory matching.
///
/// A skill is not callable. It is routing/context metadata plus a list of
/// tool names resolved from `skill_tools`.
#[derive(Clone)]
pub struct LoadedSkill {
    pub info: SkillInfo,
    /// Keywords that trigger this skill (from intent_patterns)
    pub keywords: Vec<String>,
    /// Tool names to make available to the LLM during this skill's execution.
    /// Empty means this skill has no bound tools.
    pub tools: Vec<String>,
    /// Response template / system prompt for LLM context
    pub prompt_template: Option<String>,
    /// Whether the response is cacheable
    pub cacheable: bool,
    /// Priority for conflict resolution (higher wins)
    pub priority: i32,
}

/// DB-backed skills provider
pub struct BuiltinSkills {
    db: Option<DatabaseConnection>,
    /// In-memory skill index (loaded from DB)
    skills: Arc<RwLock<HashMap<String, LoadedSkill>>>,
    /// Minimum confidence threshold for matching
    min_confidence: f32,
    /// Optional directory for markdown skill files
    skills_dir: Option<PathBuf>,
    /// Unified artifact/dependency graph manager.
    artifacts: Option<Arc<ArtifactRegistry>>,
}

impl Default for BuiltinSkills {
    fn default() -> Self {
        Self::new()
    }
}

impl BuiltinSkills {
    pub fn new() -> Self {
        Self {
            db: None,
            skills: Arc::new(RwLock::new(HashMap::new())),
            min_confidence: 0.3,
            skills_dir: None,
            artifacts: None,
        }
    }

    fn db(&self) -> anyhow::Result<&DatabaseConnection> {
        self.db
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("BuiltinSkills not initialized"))
    }

    fn artifacts(&self) -> anyhow::Result<&ArtifactRegistry> {
        self.artifacts
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("BuiltinSkills not initialized"))
    }

    /// Load all enabled skills from DB into memory
    async fn load_skills(&self) -> anyhow::Result<()> {
        let db = self.db()?;

        let rows = skill::Entity::find()
            .filter(skill::Column::Enabled.eq(true))
            .order_by_desc(skill::Column::Priority)
            .all(db)
            .await?;

        let mut skills = self.skills.write().await;
        skills.clear();

        for row in rows {
            let keywords = row
                .intent_patterns
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let tools = tool_names_for_skill(db, row.id).await?;

            skills.insert(
                row.name.clone(),
                LoadedSkill {
                    info: SkillInfo {
                        name: row.name,
                        display_name: row.display_name,
                        description: row.description.unwrap_or_default(),
                        category: row.category,
                    },
                    keywords,
                    tools,
                    prompt_template: row.prompt_template,
                    cacheable: row.cacheable,
                    priority: row.priority,
                },
            );
        }

        tracing::info!(count = skills.len(), "loaded skills from database");
        Ok(())
    }

    /// Ensure default built-in skills are present.
    async fn seed_defaults(&self) -> anyhow::Result<()> {
        let db = self.db()?;
        let defaults = builtin_skill_seeds();
        tracing::info!(count = defaults.len(), "ensuring default built-in skills");
        skill::Entity::insert_many(defaults)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(skill::Column::Name)
                    .update_columns([
                        skill::Column::DisplayName,
                        skill::Column::Description,
                        skill::Column::Category,
                        skill::Column::IntentPatterns,
                        skill::Column::PromptTemplate,
                        skill::Column::Cacheable,
                        skill::Column::Enabled,
                        skill::Column::Priority,
                        skill::Column::Source,
                        skill::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(db)
            .await?;
        self.artifacts()?.sync_existing_skill_artifacts().await?;

        Ok(())
    }

    /// Calculate match score between intent and skill keywords
    pub fn calculate_match_score(intent: &str, skill: &LoadedSkill) -> f32 {
        let intent_lower = intent.to_lowercase();
        let mut matched_keywords = 0;
        let mut total_weight = 0.0;

        for keyword in &skill.keywords {
            if intent_lower.contains(keyword) {
                matched_keywords += 1;
                // Longer keywords get higher weight (CJK chars count more)
                total_weight += keyword.chars().count() as f32 / 10.0;
            }
        }

        if matched_keywords == 0 {
            return 0.0;
        }

        let count_score = matched_keywords as f32 / skill.keywords.len().max(1) as f32;
        let weight_score = total_weight.min(1.0);

        (count_score * 0.6 + weight_score * 0.4).min(1.0)
    }

    /// Reload skills from DB into memory
    pub async fn reload(&self) -> anyhow::Result<()> {
        self.load_skills().await
    }

    /// Sync markdown skill files into DB (upsert by name).
    ///
    /// File-defined skills overwrite DB rows with the same name on every init,
    /// making the files the authoritative source. Skills created via API that
    /// don't collide with file names are unaffected.
    async fn sync_markdown_skills(&self, dir: &std::path::Path) -> anyhow::Result<()> {
        let db = self.db()?;
        let files = markdown::load_skill_files(dir).await?;

        for parsed in files {
            let fm = &parsed.frontmatter;
            let now = Utc::now();
            let tool_ids = resolve_tool_ids(db, self.artifacts()?, &fm.tools).await?;

            let intent_patterns = serde_json::Value::Array(
                fm.intent_patterns
                    .iter()
                    .map(|s| serde_json::json!(s))
                    .collect(),
            );

            // Upsert: find by name → update or insert
            let existing = skill::Entity::find()
                .filter(skill::Column::Name.eq(&fm.name))
                .one(db)
                .await?;

            let skill_id = match existing {
                Some(row) => {
                    let skill_id = row.id;
                    let mut active: skill::ActiveModel = row.into();
                    active.display_name = Set(fm.display_name.clone());
                    active.description = Set(fm.description.clone());
                    active.category = Set(fm.category.clone());
                    active.intent_patterns = Set(intent_patterns);
                    active.source = Set("file".to_string());
                    active.prompt_template = Set(parsed.prompt_template);
                    active.cacheable = Set(fm.cacheable);
                    active.enabled = Set(fm.enabled);
                    active.priority = Set(fm.priority);
                    active.updated_at = Set(now);
                    active.update(db).await?;
                    tracing::debug!(skill = %fm.name, "updated skill from markdown file");
                    skill_id
                }
                None => {
                    let skill_id = Uuid::new_v4();
                    let active = skill::ActiveModel {
                        id: Set(skill_id),
                        name: Set(fm.name.clone()),
                        display_name: Set(fm.display_name.clone()),
                        description: Set(fm.description.clone()),
                        category: Set(fm.category.clone()),
                        intent_patterns: Set(intent_patterns),
                        source: Set("file".to_string()),
                        prompt_template: Set(parsed.prompt_template),
                        cacheable: Set(fm.cacheable),
                        enabled: Set(fm.enabled),
                        priority: Set(fm.priority),
                        created_at: Set(now),
                        updated_at: Set(now),
                    };
                    active.insert(db).await?;
                    tracing::debug!(skill = %fm.name, "inserted skill from markdown file");
                    skill_id
                }
            };

            self.artifacts()?
                .upsert_artifact(&ArtifactRef::skill(fm.name.clone()), "file")
                .await?;
            replace_skill_tool_ids(db, self.artifacts()?, skill_id, &fm.name, &tool_ids).await?;
        }

        Ok(())
    }

    /// Get a skill by name from DB
    pub async fn get_by_name(&self, name: &str) -> anyhow::Result<Option<skill::Model>> {
        let db = self.db()?;
        Ok(skill::Entity::find()
            .filter(skill::Column::Name.eq(name))
            .one(db)
            .await?)
    }

    /// Create a new skill in DB
    pub async fn create(&self, model: skill::ActiveModel) -> anyhow::Result<skill::Model> {
        let db = self.db()?;
        let result = model.insert(db).await?;
        self.artifacts()?
            .upsert_artifact(&ArtifactRef::skill(result.name.clone()), &result.source)
            .await?;
        self.load_skills().await?;
        Ok(result)
    }

    /// Update a skill in DB
    pub async fn update(&self, model: skill::ActiveModel) -> anyhow::Result<skill::Model> {
        let db = self.db()?;
        let result = model.update(db).await?;
        self.artifacts()?
            .upsert_artifact(&ArtifactRef::skill(result.name.clone()), &result.source)
            .await?;
        self.load_skills().await?;
        Ok(result)
    }

    /// Delete a skill by ID
    pub async fn delete(&self, id: Uuid) -> anyhow::Result<bool> {
        let db = self.db()?;
        let Some(existing) = skill::Entity::find_by_id(id).one(db).await? else {
            return Ok(false);
        };
        let artifact = ArtifactRef::skill(existing.name);
        self.artifacts()?
            .ensure_no_external_dependents(std::slice::from_ref(&artifact))
            .await?;
        let result = skill::Entity::delete_by_id(id).exec(db).await?;
        self.artifacts()?.delete_artifact(&artifact).await?;
        self.load_skills().await?;
        Ok(result.rows_affected > 0)
    }

    /// List all skills from DB (including disabled)
    pub async fn list_all_db(&self) -> anyhow::Result<Vec<skill::Model>> {
        let db = self.db()?;
        Ok(skill::Entity::find()
            .order_by_desc(skill::Column::Priority)
            .all(db)
            .await?)
    }
}

#[async_trait]
impl ModuleLifecycle for BuiltinSkills {
    fn name(&self) -> &'static str {
        "builtin_skills"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        self.db = Some(ctx.db.clone());
        self.artifacts = Some(ctx.artifacts.clone());
        self.skills_dir = ctx
            .config
            .resources
            .skills_dir
            .as_ref()
            .map(|d| ctx.config.resolve_path(d));
        self.seed_defaults().await?;

        // Sync markdown skill files into DB (if configured)
        if let Some(dir) = &self.skills_dir {
            if let Err(e) = self.sync_markdown_skills(dir).await {
                tracing::warn!(error = %e, dir = %dir.display(), "failed to sync markdown skills");
            }
        }

        if let Err(e) = ensure_builtin_skill_tool_bindings(self.db()?, self.artifacts()?).await {
            tracing::warn!(error = %e, "failed to ensure builtin skill tool bindings");
        }

        self.load_skills().await?;
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if self.db.is_some() {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Unhealthy("Database not initialized".into())
        }
    }
}

#[async_trait]
impl SkillProvider for BuiltinSkills {
    async fn match_skill(&self, intent: &str) -> anyhow::Result<Option<SkillMatch>> {
        let skills = self.skills.read().await;

        // Find best matching skill, using priority to break ties
        let mut best: Option<(&str, f32, i32)> = None; // (name, score, priority)

        for (name, skill) in skills.iter() {
            let score = Self::calculate_match_score(intent, skill);
            if score >= self.min_confidence {
                let dominated = match best {
                    Some((_, best_score, best_prio)) => {
                        score > best_score || (score == best_score && skill.priority > best_prio)
                    }
                    None => true,
                };
                if dominated {
                    best = Some((name, score, skill.priority));
                }
            }
        }

        match best {
            Some((name, confidence, _)) => {
                tracing::debug!(skill = %name, confidence, intent = %intent, "matched skill");
                Ok(Some(SkillMatch {
                    name: name.to_string(),
                    confidence,
                }))
            }
            None => {
                tracing::debug!(intent = %intent, "no skill matched");
                Ok(None)
            }
        }
    }

    async fn resolve(&self, skill: &SkillMatch) -> anyhow::Result<SkillResolution> {
        let skills = self.skills.read().await;

        let loaded = match skills.get(&skill.name) {
            Some(s) => s,
            None => {
                anyhow::bail!("Skill '{}' not found", skill.name);
            }
        };

        Ok(SkillResolution {
            skill_name: skill.name.clone(),
            system_prompt: loaded.prompt_template.clone(),
            tools: loaded.tools.clone(),
            cacheable: loaded.cacheable,
        })
    }

    async fn list_skills(&self) -> anyhow::Result<Vec<SkillInfo>> {
        let skills = self.skills.read().await;
        Ok(skills.values().map(|s| s.info.clone()).collect())
    }

    async fn register_plugin_skill(
        &self,
        plugin_name: &str,
        def: PluginSkillDefinition,
    ) -> anyhow::Result<()> {
        let db = self.db()?;
        let now = Utc::now();
        let tool_ids = resolve_tool_ids(db, self.artifacts()?, &def.tools).await?;

        let intent_patterns = serde_json::Value::Array(
            def.intent_patterns
                .iter()
                .map(|s| serde_json::json!(s))
                .collect(),
        );

        // Upsert by name
        let existing = skill::Entity::find()
            .filter(skill::Column::Name.eq(&def.name))
            .one(db)
            .await?;

        let skill_id = match existing {
            Some(row) => {
                let skill_id = row.id;
                let mut active: skill::ActiveModel = row.into();
                active.display_name = Set(def.display_name);
                active.description = Set(def.description);
                active.category = Set(def.category);
                active.intent_patterns = Set(intent_patterns);
                active.source = Set(plugin_name.to_string());
                active.prompt_template = Set(def.prompt_template);
                active.cacheable = Set(def.cacheable);
                active.enabled = Set(true);
                active.priority = Set(def.priority);
                active.updated_at = Set(now);
                active.update(db).await?;
                skill_id
            }
            None => {
                let skill_id = Uuid::new_v4();
                let active = skill::ActiveModel {
                    id: Set(skill_id),
                    name: Set(def.name.clone()),
                    display_name: Set(def.display_name),
                    description: Set(def.description),
                    category: Set(def.category),
                    intent_patterns: Set(intent_patterns),
                    source: Set(plugin_name.to_string()),
                    prompt_template: Set(def.prompt_template),
                    cacheable: Set(def.cacheable),
                    enabled: Set(true),
                    priority: Set(def.priority),
                    created_at: Set(now),
                    updated_at: Set(now),
                };
                active.insert(db).await?;
                skill_id
            }
        };

        self.artifacts()?
            .upsert_artifact(&ArtifactRef::skill(def.name.clone()), plugin_name)
            .await?;
        replace_skill_tool_ids(db, self.artifacts()?, skill_id, &def.name, &tool_ids).await?;

        tracing::info!(plugin = %plugin_name, skill = %def.name, "registered plugin skill");
        self.load_skills().await?;
        Ok(())
    }

    async fn unregister_plugin_skills(&self, plugin_name: &str) -> anyhow::Result<u64> {
        let db = self.db()?;
        let tx = db.begin().await?;
        let artifacts = self
            .artifacts()?
            .owned_artifacts_in(&tx, "plugin", plugin_name, Some(KIND_SKILL))
            .await?;

        self.artifacts()?
            .ensure_no_external_dependents_in(&tx, &artifacts)
            .await?;

        let mut removed = 0;
        for artifact in &artifacts {
            let result = skill::Entity::delete_many()
                .filter(skill::Column::Name.eq(&artifact.name))
                .exec(&tx)
                .await?;
            removed += result.rows_affected;
            self.artifacts()?.delete_artifact_in(&tx, artifact).await?;
        }
        tx.commit().await?;

        if removed > 0 {
            tracing::info!(
                plugin = %plugin_name,
                count = removed,
                "unregistered plugin skills"
            );
            self.load_skills().await?;
        }

        Ok(removed)
    }
}

async fn tool_names_for_skill(
    db: &DatabaseConnection,
    skill_id: Uuid,
) -> anyhow::Result<Vec<String>> {
    let bindings = skill_tool::Entity::find()
        .filter(skill_tool::Column::SkillId.eq(skill_id))
        .all(db)
        .await?;
    let mut names = Vec::with_capacity(bindings.len());

    for binding in bindings {
        if let Some(row) = tool::Entity::find_by_id(binding.tool_id).one(db).await? {
            if row.enabled {
                names.push(row.name);
            }
        }
    }

    Ok(names)
}

async fn replace_skill_tools(
    db: &DatabaseConnection,
    artifacts: &ArtifactRegistry,
    skill_id: Uuid,
    skill_name: &str,
    tool_names: &[String],
) -> anyhow::Result<()> {
    let tool_ids = resolve_tool_ids(db, artifacts, tool_names).await?;
    replace_skill_tool_ids(db, artifacts, skill_id, skill_name, &tool_ids).await
}

async fn resolve_tool_ids(
    db: &DatabaseConnection,
    artifacts: &ArtifactRegistry,
    tool_names: &[String],
) -> anyhow::Result<Vec<Uuid>> {
    let mut seen = HashSet::new();
    let mut tool_ids = Vec::new();

    for name in tool_names {
        if !seen.insert(name.as_str()) {
            tracing::warn!(tool = %name, "duplicate skill tool binding ignored");
            continue;
        }
        let Some(tool) = tool::Entity::find()
            .filter(tool::Column::Name.eq(name))
            .one(db)
            .await?
        else {
            anyhow::bail!("skill references unknown tool '{name}'");
        };
        artifacts
            .upsert_artifact(&ArtifactRef::tool(tool.name.clone()), &tool.source)
            .await?;
        tool_ids.push(tool.id);
    }

    Ok(tool_ids)
}

async fn replace_skill_tool_ids(
    db: &DatabaseConnection,
    artifacts: &ArtifactRegistry,
    skill_id: Uuid,
    skill_name: &str,
    tool_ids: &[Uuid],
) -> anyhow::Result<()> {
    skill_tool::Entity::delete_many()
        .filter(skill_tool::Column::SkillId.eq(skill_id))
        .exec(db)
        .await?;

    let now = Utc::now();
    for tool_id in tool_ids {
        skill_tool::ActiveModel {
            skill_id: Set(skill_id),
            tool_id: Set(*tool_id),
            created_at: Set(now),
        }
        .insert(db)
        .await?;
    }

    let mut dependencies = Vec::with_capacity(tool_ids.len());
    for tool_id in tool_ids {
        if let Some(row) = tool::Entity::find_by_id(*tool_id).one(db).await? {
            dependencies.push(ArtifactRef::tool(row.name));
        }
    }
    artifacts
        .replace_dependencies(&ArtifactRef::skill(skill_name.to_string()), &dependencies)
        .await?;

    Ok(())
}

async fn ensure_builtin_skill_tool_bindings(
    db: &DatabaseConnection,
    artifacts: &ArtifactRegistry,
) -> anyhow::Result<()> {
    for (skill_name, tool_names) in builtin_skill_tool_bindings() {
        let Some(skill) = skill::Entity::find()
            .filter(skill::Column::Name.eq(*skill_name))
            .one(db)
            .await?
        else {
            continue;
        };
        let tool_names = tool_names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        artifacts
            .upsert_artifact(&ArtifactRef::skill(*skill_name), "builtin")
            .await?;
        replace_skill_tools(db, artifacts, skill.id, skill_name, &tool_names).await?;
    }
    Ok(())
}

pub fn builtin_skill_tool_bindings() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        ("smart_light_control", &["device_control"]),
        ("smart_ac_control", &["device_control"]),
        ("smart_appliance", &["device_control"]),
        ("timer_alarm", &["schedule_task"]),
        ("weather_query", &["sensor_read"]),
        ("device_status", &["sensor_read", "sensor_list"]),
        ("memory_query", &["memory_search"]),
        ("rule_management", &["schedule_task"]),
    ]
}

/// Generate seed data for all default built-in skills
pub fn builtin_skill_seeds() -> Vec<skill::ActiveModel> {
    let now = Utc::now();

    vec![
        // smart_light_control
        skill::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set("smart_light_control".into()),
            display_name: Set("智能灯光控制".into()),
            description: Set(Some("控制智能灯的开关、亮度、色温".into())),
            category: Set("smart_home".into()),
            intent_patterns: Set(serde_json::json!([
                "灯",
                "灯光",
                "开灯",
                "关灯",
                "亮度",
                "调暗",
                "调亮",
                "色温",
                "light",
                "lights",
                "lamp",
                "brightness",
                "把灯"
            ])),
            source: Set("builtin".to_string()),
            prompt_template: Set(Some(
                "You are in the smart-light skill context. Use the bound device_control tool for lights, brightness, and color temperature. Do not invent device state; use tools when action or state is needed.".into(),
            )),
            cacheable: Set(false),
            enabled: Set(true),
            priority: Set(10),
            created_at: Set(now),
            updated_at: Set(now),
        },
        // smart_ac_control
        skill::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set("smart_ac_control".into()),
            display_name: Set("空调控制".into()),
            description: Set(Some("控制空调的开关、温度、模式".into())),
            category: Set("smart_home".into()),
            intent_patterns: Set(serde_json::json!([
                "空调",
                "冷气",
                "暖气",
                "温度",
                "制冷",
                "制热",
                "除湿",
                "ac",
                "air conditioner",
                "调温",
                "度"
            ])),
            source: Set("builtin".to_string()),
            prompt_template: Set(Some(
                "You are in the smart-AC skill context. Use the bound device_control tool for AC power, temperature, mode, fan, heat, cool, or dehumidify actions. Ask a short clarification only when the target or value is missing.".into(),
            )),
            cacheable: Set(false),
            enabled: Set(true),
            priority: Set(10),
            created_at: Set(now),
            updated_at: Set(now),
        },
        // smart_appliance
        skill::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set("smart_appliance".into()),
            display_name: Set("通用家电控制".into()),
            description: Set(Some("控制通用智能家电（插座、窗帘、风扇等）".into())),
            category: Set("smart_home".into()),
            intent_patterns: Set(serde_json::json!([
                "窗帘",
                "风扇",
                "插座",
                "电视",
                "扫地机",
                "加湿器",
                "curtain",
                "fan",
                "plug",
                "tv",
                "vacuum",
                "humidifier",
                "打开",
                "关闭",
                "开",
                "关"
            ])),
            source: Set("builtin".to_string()),
            prompt_template: Set(Some(
                "You are in the smart-appliance skill context. Use the bound device_control tool for curtains, fans, plugs, TV, vacuums, humidifiers, and similar controllable devices. Keep confirmations concise.".into(),
            )),
            cacheable: Set(false),
            enabled: Set(true),
            priority: Set(5),
            created_at: Set(now),
            updated_at: Set(now),
        },
        // timer_alarm
        skill::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set("timer_alarm".into()),
            display_name: Set("定时器/闹钟".into()),
            description: Set(Some("设置倒计时定时器或闹钟提醒".into())),
            category: Set("utility".into()),
            intent_patterns: Set(serde_json::json!([
                "定时",
                "计时",
                "提醒",
                "倒计时",
                "闹钟",
                "timer",
                "remind",
                "countdown",
                "alarm",
                "分钟后",
                "小时后",
                "秒后"
            ])),
            source: Set("builtin".to_string()),
            prompt_template: Set(Some(
                "You are in the timer/reminder skill context. Use the bound schedule_task tool for timers, alarms, countdowns, and reminders. Convert relative times into structured schedule arguments; ask only if the time is ambiguous.".into(),
            )),
            cacheable: Set(false),
            enabled: Set(true),
            priority: Set(8),
            created_at: Set(now),
            updated_at: Set(now),
        },
        // weather_query
        skill::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set("weather_query".into()),
            display_name: Set("天气查询".into()),
            description: Set(Some("查询当前天气或天气预报".into())),
            category: Set("information".into()),
            intent_patterns: Set(serde_json::json!([
                "天气",
                "气温",
                "温度",
                "下雨",
                "下雪",
                "预报",
                "湿度",
                "weather",
                "temperature",
                "forecast",
                "rain",
                "snow"
            ])),
            source: Set("builtin".to_string()),
            prompt_template: Set(Some(
                "You are in the weather/sensor information skill context. Use bound sensor tools when live or local readings are needed. If weather data is unavailable from sensors, say so instead of inventing it.".into(),
            )),
            cacheable: Set(true),
            enabled: Set(true),
            priority: Set(8),
            created_at: Set(now),
            updated_at: Set(now),
        },
        // device_status
        skill::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set("device_status".into()),
            display_name: Set("设备状态查询".into()),
            description: Set(Some("查询智能设备的当前状态".into())),
            category: Set("information".into()),
            intent_patterns: Set(serde_json::json!([
                "设备",
                "状态",
                "查看",
                "检查",
                "多少度",
                "开着",
                "device",
                "status",
                "check",
                "state"
            ])),
            source: Set("builtin".to_string()),
            prompt_template: Set(Some(
                "You are in the device-status skill context. Use bound sensor tools to inspect available devices and readings. Do not claim a state unless it came from a tool result or known context.".into(),
            )),
            cacheable: Set(true),
            enabled: Set(true),
            priority: Set(6),
            created_at: Set(now),
            updated_at: Set(now),
        },
        // memory_query
        skill::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set("memory_query".into()),
            display_name: Set("记忆查询".into()),
            description: Set(Some("查询自己的记忆（\"我之前说过什么\"）".into())),
            category: Set("memory".into()),
            intent_patterns: Set(serde_json::json!([
                "记得",
                "记忆",
                "之前说",
                "上次",
                "以前",
                "remember",
                "memory",
                "previously",
                "last time",
                "我说过",
                "你记得",
                "你还记得"
            ])),
            source: Set("builtin".to_string()),
            prompt_template: Set(Some(
                "You are in the memory-query skill context. Use the bound memory_search tool to retrieve relevant memories before answering questions about prior preferences, conversations, or remembered facts.".into(),
            )),
            cacheable: Set(false),
            enabled: Set(true),
            priority: Set(7),
            created_at: Set(now),
            updated_at: Set(now),
        },
        // rule_management
        skill::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set("rule_management".into()),
            display_name: Set("自动化规则管理".into()),
            description: Set(Some("语音管理自动化规则（创建/修改/删除）".into())),
            category: Set("automation".into()),
            intent_patterns: Set(serde_json::json!([
                "规则",
                "自动化",
                "自动",
                "每天",
                "当",
                "rule",
                "automation",
                "automate",
                "schedule",
                "如果",
                "触发",
                "条件"
            ])),
            source: Set("builtin".to_string()),
            prompt_template: Set(Some(
                "You are in the automation-rules skill context. Use bound scheduling/automation tools for creating or changing rules. For destructive changes, ask for confirmation before claiming completion.".into(),
            )),
            cacheable: Set(false),
            enabled: Set(true),
            priority: Set(5),
            created_at: Set(now),
            updated_at: Set(now),
        },
    ]
}
