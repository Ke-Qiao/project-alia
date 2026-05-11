//! Builtin Skills Provider
//!
//! DB-backed skill provider that loads skill definitions from PostgreSQL.
//! On first init, seeds default built-in skills if the table is empty.
//! Supports keyword-based intent matching with priority-based conflict resolution.
//!
//! Skills extend what the LLM **knows how to do well**. Each skill carries:
//! - Intent patterns for matching user messages
//! - A prompt template providing domain-specific LLM instructions
//! - A list of tool names to make available during LLM execution

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};
use tokio::sync::RwLock;
use uuid::Uuid;

use super::markdown;
use crate::entity::skill;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    PluginSkillDefinition, SkillInfo, SkillMatch, SkillProvider, SkillResolution,
};

/// A loaded skill with its metadata for in-memory matching
#[derive(Clone)]
pub struct LoadedSkill {
    pub info: SkillInfo,
    /// Keywords that trigger this skill (from intent_patterns)
    pub keywords: Vec<String>,
    /// Parameter schema (JSON Schema)
    pub param_schema: serde_json::Value,
    /// Tool names to make available to the LLM during this skill's execution.
    /// If empty, all tools are available.
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
        }
    }

    fn db(&self) -> anyhow::Result<&DatabaseConnection> {
        self.db
            .as_ref()
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

            // Prefer native `tools` column; fall back to parsing `execution_config`
            let tools = parse_tools_from_json_column(&row.tools)
                .or_else(|| {
                    let t = parse_tools_from_execution_config(&row.execution_config);
                    if t.is_empty() { None } else { Some(t) }
                })
                .unwrap_or_default();

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
                    param_schema: row.parameters,
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

    /// Seed default skills if the table is empty
    async fn seed_defaults(&self) -> anyhow::Result<()> {
        let db = self.db()?;
        let count = skill::Entity::find().count(db).await?;
        if count > 0 {
            tracing::debug!(existing = count, "skills table not empty, skipping seed");
            return Ok(());
        }

        tracing::info!("seeding default built-in skills");

        let defaults = builtin_skill_seeds();
        for model in defaults {
            model.insert(db).await?;
        }

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

            let intent_patterns = serde_json::Value::Array(
                fm.intent_patterns
                    .iter()
                    .map(|s| serde_json::json!(s))
                    .collect(),
            );

            let parameters = fm
                .parameters
                .as_ref()
                .map(markdown::toml_to_json)
                .unwrap_or_else(|| serde_json::json!({}));

            // Convert tools list to execution_config (stored as JSON array in DB)
            let execution_config = if fm.tools.is_empty() {
                None
            } else {
                Some(serde_json::Value::Array(
                    fm.tools.iter().map(|t| serde_json::json!(t)).collect(),
                ))
            };

            // Upsert: find by name → update or insert
            let existing = skill::Entity::find()
                .filter(skill::Column::Name.eq(&fm.name))
                .one(db)
                .await?;

            let tools_col = if fm.tools.is_empty() {
                None
            } else {
                Some(serde_json::Value::Array(
                    fm.tools.iter().map(|t| serde_json::json!(t)).collect(),
                ))
            };

            match existing {
                Some(row) => {
                    let mut active: skill::ActiveModel = row.into();
                    active.display_name = Set(fm.display_name.clone());
                    active.description = Set(fm.description.clone());
                    active.category = Set(fm.category.clone());
                    active.intent_patterns = Set(intent_patterns);
                    active.parameters = Set(parameters);
                    active.execution_config = Set(execution_config);
                    active.tools = Set(tools_col);
                    active.source = Set("file".to_string());
                    active.prompt_template = Set(parsed.prompt_template);
                    active.cacheable = Set(fm.cacheable);
                    active.enabled = Set(fm.enabled);
                    active.priority = Set(fm.priority);
                    active.updated_at = Set(now);
                    active.update(db).await?;
                    tracing::debug!(skill = %fm.name, "updated skill from markdown file");
                }
                None => {
                    let active = skill::ActiveModel {
                        id: Set(Uuid::new_v4()),
                        name: Set(fm.name.clone()),
                        display_name: Set(fm.display_name.clone()),
                        description: Set(fm.description.clone()),
                        category: Set(fm.category.clone()),
                        intent_patterns: Set(intent_patterns),
                        parameters: Set(parameters),
                        execution_config: Set(execution_config),
                        tools: Set(tools_col),
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
                }
            }
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
        self.load_skills().await?;
        Ok(result)
    }

    /// Update a skill in DB
    pub async fn update(&self, model: skill::ActiveModel) -> anyhow::Result<skill::Model> {
        let db = self.db()?;
        let result = model.update(db).await?;
        self.load_skills().await?;
        Ok(result)
    }

    /// Delete a skill by ID
    pub async fn delete(&self, id: Uuid) -> anyhow::Result<bool> {
        let db = self.db()?;
        let result = skill::Entity::delete_by_id(id).exec(db).await?;
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
        self.skills_dir = ctx
            .config
            .skills
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
                    parameters: serde_json::Value::Object(serde_json::Map::new()),
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

        let intent_patterns = serde_json::Value::Array(
            def.intent_patterns
                .iter()
                .map(|s| serde_json::json!(s))
                .collect(),
        );

        let execution_config = if def.tools.is_empty() {
            None
        } else {
            Some(serde_json::Value::Array(
                def.tools.iter().map(|t| serde_json::json!(t)).collect(),
            ))
        };

        let tools_col = if def.tools.is_empty() {
            None
        } else {
            Some(serde_json::Value::Array(
                def.tools.iter().map(|t| serde_json::json!(t)).collect(),
            ))
        };

        // Upsert by name
        let existing = skill::Entity::find()
            .filter(skill::Column::Name.eq(&def.name))
            .one(db)
            .await?;

        match existing {
            Some(row) => {
                let mut active: skill::ActiveModel = row.into();
                active.display_name = Set(def.display_name);
                active.description = Set(def.description);
                active.category = Set(def.category);
                active.intent_patterns = Set(intent_patterns);
                active.parameters = Set(def.parameters);
                active.execution_config = Set(execution_config);
                active.tools = Set(tools_col);
                active.source = Set(plugin_name.to_string());
                active.prompt_template = Set(def.prompt_template);
                active.cacheable = Set(def.cacheable);
                active.enabled = Set(true);
                active.priority = Set(def.priority);
                active.updated_at = Set(now);
                active.update(db).await?;
            }
            None => {
                let active = skill::ActiveModel {
                    id: Set(Uuid::new_v4()),
                    name: Set(def.name.clone()),
                    display_name: Set(def.display_name),
                    description: Set(def.description),
                    category: Set(def.category),
                    intent_patterns: Set(intent_patterns),
                    parameters: Set(def.parameters),
                    execution_config: Set(execution_config),
                    tools: Set(tools_col),
                    source: Set(plugin_name.to_string()),
                    prompt_template: Set(def.prompt_template),
                    cacheable: Set(def.cacheable),
                    enabled: Set(true),
                    priority: Set(def.priority),
                    created_at: Set(now),
                    updated_at: Set(now),
                };
                active.insert(db).await?;
            }
        }

        tracing::info!(plugin = %plugin_name, skill = %def.name, "registered plugin skill");
        self.load_skills().await?;
        Ok(())
    }

    async fn unregister_plugin_skills(&self, plugin_name: &str) -> anyhow::Result<u64> {
        let db = self.db()?;
        let result = skill::Entity::delete_many()
            .filter(skill::Column::Source.eq(plugin_name))
            .exec(db)
            .await?;

        if result.rows_affected > 0 {
            tracing::info!(
                plugin = %plugin_name,
                count = result.rows_affected,
                "unregistered plugin skills"
            );
            self.load_skills().await?;
        }

        Ok(result.rows_affected)
    }
}

/// Parse tool names from the `execution_config` JSON column.
///
/// Supports two formats for backward compatibility:
/// - New format (JSON array of strings): `["device_control", "sensor_read"]`
/// - Legacy format (nested object): `{"type": "function", "function": {"name": "send_plugin_command", ...}}`
fn parse_tools_from_execution_config(config: &Option<serde_json::Value>) -> Vec<String> {
    let Some(config) = config else {
        return Vec::new();
    };

    // New format: JSON array of tool name strings
    if let Some(arr) = config.as_array() {
        return arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }

    // Legacy format: {"type": "function", "function": {"name": "..."}}
    if let Some(name) = config
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
    {
        return vec![name.to_string()];
    }

    Vec::new()
}

/// Parse tool names from the native `tools` JSONB column.
/// Returns `None` if the column is NULL or empty, `Some(vec)` otherwise.
fn parse_tools_from_json_column(tools: &Option<serde_json::Value>) -> Option<Vec<String>> {
    let tools = tools.as_ref()?;
    let arr = tools.as_array()?;
    if arr.is_empty() {
        return None;
    }
    Some(
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
    )
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
            parameters: Set(serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["on", "off", "set_brightness", "set_color_temp"]},
                    "device_name": {"type": "string", "description": "设备名称或位置"},
                    "value": {"type": "number", "description": "亮度(0-100)或色温(2700-6500K)"}
                }
            })),
            execution_config: Set(Some(serde_json::json!(["device_control"]))),
            tools: Set(Some(serde_json::json!(["device_control"]))),
            source: Set("builtin".to_string()),
            prompt_template: Set(Some(
                "用户想要{action}{device_name}。请执行操作并用自然语言确认。".into(),
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
            parameters: Set(serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["on", "off", "set_temperature", "set_mode"]},
                    "device_name": {"type": "string"},
                    "temperature": {"type": "number", "minimum": 16, "maximum": 30},
                    "mode": {"type": "string", "enum": ["cool", "heat", "auto", "dehumidify", "fan"]}
                }
            })),
            execution_config: Set(Some(serde_json::json!(["device_control"]))),
            tools: Set(Some(serde_json::json!(["device_control"]))),
            source: Set("builtin".to_string()),
            prompt_template: Set(Some(
                "用户想要{action}空调。温度: {temperature}°C，模式: {mode}。".into(),
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
            parameters: Set(serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["on", "off", "toggle"]},
                    "device_name": {"type": "string"},
                    "device_type": {"type": "string"}
                }
            })),
            execution_config: Set(Some(serde_json::json!(["device_control"]))),
            tools: Set(Some(serde_json::json!(["device_control"]))),
            source: Set("builtin".to_string()),
            prompt_template: Set(None),
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
            parameters: Set(serde_json::json!({
                "type": "object",
                "properties": {
                    "duration_seconds": {"type": "integer", "minimum": 1},
                    "message": {"type": "string"},
                    "type": {"type": "string", "enum": ["timer", "alarm"]}
                },
                "required": ["duration_seconds"]
            })),
            execution_config: Set(Some(serde_json::json!(["schedule_task"]))),
            tools: Set(Some(serde_json::json!(["schedule_task"]))),
            source: Set("builtin".to_string()),
            prompt_template: Set(Some("定时器已设置，{duration}后提醒: {message}".into())),
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
            parameters: Set(serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {"type": "string"},
                    "days": {"type": "integer", "minimum": 1, "maximum": 7}
                }
            })),
            execution_config: Set(Some(serde_json::json!(["sensor_read"]))),
            tools: Set(Some(serde_json::json!(["sensor_read"]))),
            source: Set("builtin".to_string()),
            prompt_template: Set(None),
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
            parameters: Set(serde_json::json!({
                "type": "object",
                "properties": {
                    "device_name": {"type": "string"},
                    "device_type": {"type": "string"}
                }
            })),
            execution_config: Set(Some(serde_json::json!(["sensor_read", "sensor_list"]))),
            tools: Set(Some(serde_json::json!(["sensor_read", "sensor_list"]))),
            source: Set("builtin".to_string()),
            prompt_template: Set(None),
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
            parameters: Set(serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "搜索关键词"},
                    "time_range": {"type": "string", "enum": ["today", "this_week", "this_month", "all"]}
                }
            })),
            execution_config: Set(Some(serde_json::json!(["memory_search"]))),
            tools: Set(Some(serde_json::json!(["memory_search"]))),
            source: Set("builtin".to_string()),
            prompt_template: Set(None),
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
            parameters: Set(serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["create", "update", "delete", "list"]},
                    "rule_description": {"type": "string"}
                }
            })),
            execution_config: Set(Some(serde_json::json!(["schedule_task"]))),
            tools: Set(Some(serde_json::json!(["schedule_task"]))),
            source: Set("builtin".to_string()),
            prompt_template: Set(None),
            cacheable: Set(false),
            enabled: Set(true),
            priority: Set(5),
            created_at: Set(now),
            updated_at: Set(now),
        },
    ]
}
