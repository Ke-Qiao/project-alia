//! Builtin Skills Provider
//!
//! Provides a set of built-in skills with keyword-based intent matching.
//! Skills are registered in-memory and matched against user intents using
//! keyword scoring. Each skill has associated keywords and a handler function.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{SkillInfo, SkillMatch, SkillProvider, SkillResult};

/// A registered skill with its metadata and handler
#[derive(Clone)]
pub struct RegisteredSkill {
    pub info: SkillInfo,
    /// Keywords that trigger this skill (lowercase)
    pub keywords: Vec<String>,
    /// Parameter schema (JSON Schema)
    pub param_schema: Option<serde_json::Value>,
    /// Whether the skill requires external service
    pub requires_external: bool,
}

/// Builtin skills provider with keyword-based matching
pub struct BuiltinSkills {
    /// Registered skills by name
    skills: Arc<RwLock<HashMap<String, RegisteredSkill>>>,
    /// Minimum confidence threshold for matching
    min_confidence: f32,
}

impl Default for BuiltinSkills {
    fn default() -> Self {
        Self::new()
    }
}

impl BuiltinSkills {
    pub fn new() -> Self {
        let skills = Arc::new(RwLock::new(HashMap::new()));
        let provider = Self {
            skills,
            min_confidence: 0.3,
        };
        // Register default builtin skills synchronously during construction
        provider.register_builtin_skills_sync();
        provider
    }

    /// Register default builtin skills
    fn register_builtin_skills_sync(&self) {
        // We need to do this synchronously in the constructor
        // Use blocking lock since we're in a sync context during construction
        let mut skills = self.skills.blocking_write();

        // Light control skill
        skills.insert(
            "light_control".into(),
            RegisteredSkill {
                info: SkillInfo {
                    name: "light_control".into(),
                    display_name: "灯光控制".into(),
                    description: "控制智能灯的开关、亮度和颜色".into(),
                    category: "smart_home".into(),
                },
                keywords: vec![
                    "灯".into(), "灯光".into(), "开灯".into(), "关灯".into(),
                    "亮度".into(), "调暗".into(), "调亮".into(), "light".into(),
                    "lights".into(), "lamp".into(), "brightness".into(),
                ],
                param_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["on", "off", "dim", "brighten", "set_color"]},
                        "room": {"type": "string"},
                        "brightness": {"type": "integer", "minimum": 0, "maximum": 100},
                        "color": {"type": "string"}
                    }
                })),
                requires_external: true,
            },
        );

        // Timer skill
        skills.insert(
            "timer".into(),
            RegisteredSkill {
                info: SkillInfo {
                    name: "timer".into(),
                    display_name: "定时器".into(),
                    description: "设置倒计时提醒".into(),
                    category: "utility".into(),
                },
                keywords: vec![
                    "定时".into(), "计时".into(), "提醒".into(), "倒计时".into(),
                    "timer".into(), "remind".into(), "countdown".into(), "alarm".into(),
                    "分钟后".into(), "小时后".into(),
                ],
                param_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "duration_seconds": {"type": "integer", "minimum": 1},
                        "message": {"type": "string"}
                    },
                    "required": ["duration_seconds"]
                })),
                requires_external: false,
            },
        );

        // Weather skill
        skills.insert(
            "weather".into(),
            RegisteredSkill {
                info: SkillInfo {
                    name: "weather".into(),
                    display_name: "天气查询".into(),
                    description: "查询当前天气或天气预报".into(),
                    category: "information".into(),
                },
                keywords: vec![
                    "天气".into(), "气温".into(), "温度".into(), "下雨".into(),
                    "下雪".into(), "预报".into(), "weather".into(), "temperature".into(),
                    "forecast".into(), "rain".into(),
                ],
                param_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "location": {"type": "string"},
                        "days": {"type": "integer", "minimum": 1, "maximum": 7}
                    }
                })),
                requires_external: true,
            },
        );

        // Music control skill
        skills.insert(
            "music_control".into(),
            RegisteredSkill {
                info: SkillInfo {
                    name: "music_control".into(),
                    display_name: "音乐控制".into(),
                    description: "播放、暂停、切换音乐".into(),
                    category: "entertainment".into(),
                },
                keywords: vec![
                    "音乐".into(), "播放".into(), "暂停".into(), "下一首".into(),
                    "上一首".into(), "歌".into(), "music".into(), "play".into(),
                    "pause".into(), "next".into(), "previous".into(), "song".into(),
                ],
                param_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["play", "pause", "next", "previous", "stop"]},
                        "query": {"type": "string"},
                        "volume": {"type": "integer", "minimum": 0, "maximum": 100}
                    }
                })),
                requires_external: true,
            },
        );

        // Device status skill
        skills.insert(
            "device_status".into(),
            RegisteredSkill {
                info: SkillInfo {
                    name: "device_status".into(),
                    display_name: "设备状态".into(),
                    description: "查询智能设备状态".into(),
                    category: "smart_home".into(),
                },
                keywords: vec![
                    "设备".into(), "状态".into(), "查看".into(), "检查".into(),
                    "device".into(), "status".into(), "check".into(), "state".into(),
                ],
                param_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "device_id": {"type": "string"},
                        "device_type": {"type": "string"}
                    }
                })),
                requires_external: true,
            },
        );

        // Scene activation skill
        skills.insert(
            "scene".into(),
            RegisteredSkill {
                info: SkillInfo {
                    name: "scene".into(),
                    display_name: "场景激活".into(),
                    description: "激活预设的智能家居场景".into(),
                    category: "smart_home".into(),
                },
                keywords: vec![
                    "场景".into(), "模式".into(), "回家".into(), "离家".into(),
                    "睡眠".into(), "起床".into(), "scene".into(), "mode".into(),
                    "home".into(), "away".into(), "sleep".into(), "wake".into(),
                ],
                param_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "scene_name": {"type": "string"}
                    },
                    "required": ["scene_name"]
                })),
                requires_external: true,
            },
        );
    }

    /// Calculate match score between intent and skill keywords
    fn calculate_match_score(&self, intent: &str, skill: &RegisteredSkill) -> f32 {
        let intent_lower = intent.to_lowercase();
        let mut matched_keywords = 0;
        let mut total_weight = 0.0;

        for keyword in &skill.keywords {
            if intent_lower.contains(keyword) {
                matched_keywords += 1;
                // Longer keywords get higher weight
                total_weight += keyword.len() as f32 / 10.0;
            }
        }

        if matched_keywords == 0 {
            return 0.0;
        }

        // Combine keyword count and weight
        let count_score = matched_keywords as f32 / skill.keywords.len().max(1) as f32;
        let weight_score = total_weight.min(1.0);

        (count_score * 0.6 + weight_score * 0.4).min(1.0)
    }

    /// Register a custom skill
    pub async fn register_skill(&self, skill: RegisteredSkill) {
        let mut skills = self.skills.write().await;
        skills.insert(skill.info.name.clone(), skill);
    }

    /// Unregister a skill by name
    pub async fn unregister_skill(&self, name: &str) -> bool {
        let mut skills = self.skills.write().await;
        skills.remove(name).is_some()
    }
}

#[async_trait]
impl ModuleLifecycle for BuiltinSkills {
    fn name(&self) -> &'static str {
        "builtin_skills"
    }

    async fn init(&mut self, _ctx: &ModuleContext) -> anyhow::Result<()> {
        let skills = self.skills.read().await;
        tracing::info!(
            skill_count = skills.len(),
            "BuiltinSkills initialized with {} skills",
            skills.len()
        );
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}

#[async_trait]
impl SkillProvider for BuiltinSkills {
    async fn match_skill(&self, intent: &str) -> anyhow::Result<Option<SkillMatch>> {
        let skills = self.skills.read().await;

        let mut best_match: Option<(String, f32)> = None;

        for (name, skill) in skills.iter() {
            let score = self.calculate_match_score(intent, skill);
            if score >= self.min_confidence {
                if best_match.is_none() || score > best_match.as_ref().unwrap().1 {
                    best_match = Some((name.clone(), score));
                }
            }
        }

        match best_match {
            Some((name, confidence)) => {
                tracing::debug!(
                    skill = %name,
                    confidence = %confidence,
                    intent = %intent,
                    "Matched skill"
                );
                Ok(Some(SkillMatch {
                    name,
                    confidence,
                    parameters: serde_json::Value::Object(serde_json::Map::new()),
                }))
            }
            None => {
                tracing::debug!(intent = %intent, "No skill matched");
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

        let registered = match skills.get(&skill.name) {
            Some(s) => s,
            None => {
                return Ok(SkillResult {
                    success: false,
                    output: serde_json::Value::Null,
                    message: Some(format!("Skill '{}' not found", skill.name)),
                });
            }
        };

        // For builtin skills, we return a structured response that indicates
        // the skill was matched but actual execution requires external service
        if registered.requires_external {
            return Ok(SkillResult {
                success: true,
                output: serde_json::json!({
                    "skill": skill.name,
                    "status": "requires_external_execution",
                    "parameters": params,
                    "schema": registered.param_schema,
                }),
                message: Some(format!(
                    "Skill '{}' matched. Execution requires external service integration.",
                    registered.info.display_name
                )),
            });
        }

        // For skills that don't require external service (like timer),
        // we can provide a basic response
        match skill.name.as_str() {
            "timer" => {
                let duration = params.get("duration_seconds")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(60);
                let message = params.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("时间到了");

                Ok(SkillResult {
                    success: true,
                    output: serde_json::json!({
                        "timer_id": uuid::Uuid::new_v4().to_string(),
                        "duration_seconds": duration,
                        "message": message,
                        "status": "scheduled"
                    }),
                    message: Some(format!("定时器已设置，{}秒后提醒: {}", duration, message)),
                })
            }
            _ => Ok(SkillResult {
                success: true,
                output: serde_json::json!({
                    "skill": skill.name,
                    "parameters": params,
                }),
                message: Some(format!("Skill '{}' executed", skill.name)),
            }),
        }
    }

    async fn list_skills(&self) -> anyhow::Result<Vec<SkillInfo>> {
        let skills = self.skills.read().await;
        Ok(skills.values().map(|s| s.info.clone()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_match_light_skill() {
        let provider = BuiltinSkills::new();
        
        let result = provider.match_skill("帮我把客厅的灯打开").await.unwrap();
        assert!(result.is_some());
        let m = result.unwrap();
        assert_eq!(m.name, "light_control");
        assert!(m.confidence > 0.0);
    }

    #[tokio::test]
    async fn test_match_timer_skill() {
        let provider = BuiltinSkills::new();
        
        let result = provider.match_skill("设置一个5分钟的定时器").await.unwrap();
        assert!(result.is_some());
        let m = result.unwrap();
        assert_eq!(m.name, "timer");
    }

    #[tokio::test]
    async fn test_no_match() {
        let provider = BuiltinSkills::new();
        
        let result = provider.match_skill("今天心情不错").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_list_skills() {
        let provider = BuiltinSkills::new();
        
        let skills = provider.list_skills().await.unwrap();
        assert!(!skills.is_empty());
        assert!(skills.iter().any(|s| s.name == "light_control"));
        assert!(skills.iter().any(|s| s.name == "timer"));
    }

    #[tokio::test]
    async fn test_execute_timer() {
        let provider = BuiltinSkills::new();
        
        let skill_match = SkillMatch {
            name: "timer".into(),
            confidence: 0.8,
            parameters: serde_json::Value::Null,
        };

        let params = serde_json::json!({
            "duration_seconds": 300,
            "message": "煮饭时间到"
        });

        let result = provider.execute(&skill_match, params).await.unwrap();
        assert!(result.success);
        assert!(result.output.get("timer_id").is_some());
    }
}
