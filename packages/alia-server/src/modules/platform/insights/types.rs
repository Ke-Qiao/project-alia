use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightTask {
    pub user_id: Uuid,
    pub phase: String,
    pub trigger: String,
    pub context: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightStatus {
    pub enabled: bool,
    pub last_phase: Option<String>,
    pub last_executed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub pending_tasks: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightPhase {
    Extraction,
    Consolidation,
    Reflection,
    HabitDetection,
}

impl InsightPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Extraction => "extraction",
            Self::Consolidation => "consolidation",
            Self::Reflection => "reflection",
            Self::HabitDetection => "habit_detection",
        }
    }
}

impl std::fmt::Display for InsightPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
