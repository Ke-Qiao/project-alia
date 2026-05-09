//! Builtin Skills Provider
//!
//! DB-backed skill provider that loads skill definitions from PostgreSQL.
//! On first init, seeds default built-in skills if the table is empty.
//! Supports keyword-based intent matching with priority-based conflict resolution.

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
use crate::providers::adapter::{SkillInfo, SkillMatch, SkillProvider, SkillResult};

/// A loaded skill with its metadata for in-memory matching
#[derive(Clone)]
pub struct LoadedSkill {
    pub info: SkillInfo,
    /// Keywords that trigger this skill (from intent_patterns)
    pub keywords: Vec<String>,
    /// Parameter schema (JSON Schema)
    pub param_schema: serde_json::Value,
    /// Internal execution routing config (plugin, action)
    pub execution_config: Option<serde_json::Value>,
    /// Response template for simple confirmations
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
                    execution_config: row.execution_config,
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

            let execution_config = fm.execution_config.as_ref().map(markdown::toml_to_json);

            // Upsert: find by name → update or insert
            let existing = skill::Entity::find()
                .filter(skill::Column::Name.eq(&fm.name))
                .one(db)
                .await?;

            match existing {
                Some(row) => {
                    let mut active: skill::ActiveModel = row.into();
                    active.display_name = Set(fm.display_name.clone());
                    active.description = Set(fm.description.clone());
                    active.category = Set(fm.category.clone());
                    active.intent_patterns = Set(intent_patterns);
                    active.parameters = Set(parameters);
                    active.execution_config = Set(execution_config);
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

    async fn execute(
        &self,
        skill: &SkillMatch,
        params: serde_json::Value,
    ) -> anyhow::Result<SkillResult> {
        let skills = self.skills.read().await;

        let loaded = match skills.get(&skill.name) {
            Some(s) => s,
            None => {
                return Ok(SkillResult {
                    success: false,
                    output: serde_json::Value::Null,
                    message: Some(format!("Skill '{}' not found", skill.name)),
                });
            }
        };

        // If skill has a prompt_template, return it for template-based response
        // (the caller can fill the template or pass to LLM)
        let template = loaded.prompt_template.clone();
        let exec_config = loaded.execution_config.clone();

        Ok(SkillResult {
            success: true,
            output: serde_json::json!({
                "skill": skill.name,
                "category": loaded.info.category,
                "parameters": params,
                "param_schema": loaded.param_schema,
                "execution_config": exec_config,
                "prompt_template": template,
                "cacheable": loaded.cacheable,
            }),
            message: Some(format!(
                "Skill '{}' ready for execution",
                loaded.info.display_name
            )),
        })
    }

    async fn list_skills(&self) -> anyhow::Result<Vec<SkillInfo>> {
        let skills = self.skills.read().await;
        Ok(skills.values().map(|s| s.info.clone()).collect())
    }
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
            execution_config: Set(Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "send_plugin_command",
                    "parameters": {"plugin_name": "zigbee-bridge", "action": "light_control"}
                }
            }))),
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
            execution_config: Set(Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "send_plugin_command",
                    "parameters": {"plugin_name": "ir-bridge", "action": "ac_control"}
                }
            }))),
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
            execution_config: Set(Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "send_plugin_command",
                    "parameters": {"plugin_name": "smart-home", "action": "device_control"}
                }
            }))),
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
            execution_config: Set(Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "schedule_task",
                    "parameters": {"trigger_type": "once"}
                }
            }))),
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
            execution_config: Set(Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "weather_query",
                    "parameters": {}
                }
            }))),
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
            execution_config: Set(Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "device_control",
                    "parameters": {"action": "query_status"}
                }
            }))),
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
            execution_config: Set(Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "memory_search",
                    "parameters": {}
                }
            }))),
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
            execution_config: Set(None),
            prompt_template: Set(None),
            cacheable: Set(false),
            enabled: Set(true),
            priority: Set(5),
            created_at: Set(now),
            updated_at: Set(now),
        },
    ]
}
