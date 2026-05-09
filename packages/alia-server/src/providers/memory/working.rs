//! L0 Working Memory - Conversation Context Window
//!
//! In-memory sliding window of recent conversation turns.
//! Provides immediate context for the current dialogue session.
//!
//! Features:
//! - Per-user conversation windows
//! - Configurable window size (default: 20 turns)
//! - Automatic summarization trigger when window is full
//! - Thread-safe concurrent access

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Default maximum number of turns to keep in working memory
const DEFAULT_WINDOW_SIZE: usize = 20;

/// A single turn in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub id: Uuid,
    pub role: String, // "user" | "assistant" | "system"
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

/// Working memory context for a single user
#[derive(Debug, Clone)]
pub struct WorkingContext {
    /// Conversation turns (sliding window)
    pub turns: VecDeque<ConversationTurn>,
    /// Session start time
    pub session_start: DateTime<Utc>,
    /// Last activity timestamp
    pub last_activity: DateTime<Utc>,
    /// Session metadata (device info, etc.)
    pub session_metadata: serde_json::Value,
}

impl Default for WorkingContext {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            turns: VecDeque::new(),
            session_start: now,
            last_activity: now,
            session_metadata: serde_json::Value::Object(serde_json::Map::new()),
        }
    }
}

/// L0 Working Memory Manager
///
/// Manages in-memory conversation context windows for all active users.
pub struct WorkingMemory {
    /// Per-user working contexts
    contexts: Arc<RwLock<HashMap<Uuid, WorkingContext>>>,
    /// Maximum turns per user
    window_size: usize,
}

impl Default for WorkingMemory {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW_SIZE)
    }
}

impl WorkingMemory {
    pub fn new(window_size: usize) -> Self {
        Self {
            contexts: Arc::new(RwLock::new(HashMap::new())),
            window_size,
        }
    }

    /// Add a turn to the user's working memory
    ///
    /// Returns true if the window is now full (trigger summarization)
    pub async fn add_turn(&self, user_id: Uuid, turn: ConversationTurn) -> bool {
        let mut contexts = self.contexts.write().await;
        let ctx = contexts.entry(user_id).or_default();

        ctx.turns.push_back(turn);
        ctx.last_activity = Utc::now();

        // Check if window is full
        let is_full = ctx.turns.len() >= self.window_size;

        // Trim if over capacity
        while ctx.turns.len() > self.window_size {
            ctx.turns.pop_front();
        }

        is_full
    }

    /// Get the current conversation context for a user
    pub async fn get_context(&self, user_id: Uuid) -> Option<WorkingContext> {
        let contexts = self.contexts.read().await;
        contexts.get(&user_id).cloned()
    }

    /// Get recent turns as chat messages (for LLM context)
    pub async fn get_messages(&self, user_id: Uuid, limit: Option<usize>) -> Vec<ConversationTurn> {
        let contexts = self.contexts.read().await;
        match contexts.get(&user_id) {
            Some(ctx) => {
                let limit = limit.unwrap_or(self.window_size);
                ctx.turns.iter().rev().take(limit).rev().cloned().collect()
            }
            None => Vec::new(),
        }
    }

    /// Clear working memory for a user (session end)
    pub async fn clear(&self, user_id: Uuid) -> Option<WorkingContext> {
        let mut contexts = self.contexts.write().await;
        contexts.remove(&user_id)
    }

    /// Get all turns for summarization (before clearing)
    pub async fn drain_for_summary(&self, user_id: Uuid) -> Vec<ConversationTurn> {
        let mut contexts = self.contexts.write().await;
        match contexts.get_mut(&user_id) {
            Some(ctx) => {
                let turns: Vec<_> = ctx.turns.drain(..).collect();
                turns
            }
            None => Vec::new(),
        }
    }

    /// Update session metadata
    pub async fn update_metadata(&self, user_id: Uuid, metadata: serde_json::Value) {
        let mut contexts = self.contexts.write().await;
        let ctx = contexts.entry(user_id).or_default();
        ctx.session_metadata = metadata;
    }

    /// Get number of active sessions
    pub async fn active_sessions(&self) -> usize {
        let contexts = self.contexts.read().await;
        contexts.len()
    }

    /// Cleanup inactive sessions (older than threshold)
    pub async fn cleanup_inactive(&self, max_idle_seconds: i64) -> usize {
        let mut contexts = self.contexts.write().await;
        let now = Utc::now();
        let threshold = chrono::Duration::seconds(max_idle_seconds);

        let inactive: Vec<Uuid> = contexts
            .iter()
            .filter(|(_, ctx)| now.signed_duration_since(ctx.last_activity) > threshold)
            .map(|(id, _)| *id)
            .collect();

        let count = inactive.len();
        for id in inactive {
            contexts.remove(&id);
        }
        count
    }

    /// Get window utilization for a user (0.0 - 1.0)
    pub async fn window_utilization(&self, user_id: Uuid) -> f32 {
        let contexts = self.contexts.read().await;
        match contexts.get(&user_id) {
            Some(ctx) => ctx.turns.len() as f32 / self.window_size as f32,
            None => 0.0,
        }
    }
}
