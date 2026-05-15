use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::entity::{chat_message, chat_session};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{ChatMessage, ConversationBackend};

pub struct DatabaseConversationBackend {
    db: Option<DatabaseConnection>,
    // In-memory tracker for the active session ID per channel_id
    active_sessions: Arc<RwLock<HashMap<String, Uuid>>>,
}

impl DatabaseConversationBackend {
    pub fn new() -> Self {
        Self {
            db: None,
            active_sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn db(&self) -> anyhow::Result<&DatabaseConnection> {
        self.db
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("DatabaseConversationBackend not initialized"))
    }
}

#[async_trait]
impl ModuleLifecycle for DatabaseConversationBackend {
    fn name(&self) -> &'static str {
        "database_conversation"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        self.db = Some(ctx.db.clone());
        tracing::info!("DatabaseConversationBackend initialized");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if let Some(db) = &self.db {
            if db.ping().await.is_ok() {
                return ModuleHealth::Healthy;
            }
        }
        ModuleHealth::Unhealthy("database connection failed".into())
    }
}

#[async_trait]
impl ConversationBackend for DatabaseConversationBackend {
    async fn get_or_create_active_session(
        &self,
        channel_id: &str,
        user_id: &str,
    ) -> anyhow::Result<Uuid> {
        let key = channel_id.to_string();

        // 1. Check in-memory map
        {
            let map = self.active_sessions.read().await;
            if let Some(session_id) = map.get(&key) {
                return Ok(*session_id);
            }
        }

        let db = self.db()?;

        // 2. Check latest from database for this channel
        let latest = chat_session::Entity::find()
            .filter(chat_session::Column::ChannelId.eq(channel_id))
            .order_by_desc(chat_session::Column::UpdatedAt)
            .one(db)
            .await?;

        if let Some(session) = latest {
            let mut map = self.active_sessions.write().await;
            map.insert(key, session.id);
            return Ok(session.id);
        }

        // 3. Create new if none exists
        let new_id = Uuid::new_v4();
        let session = chat_session::ActiveModel {
            id: Set(new_id),
            channel_id: Set(channel_id.to_string()),
            creator_id: Set(Some(user_id.to_string())),
            name: Set(Some("New Chat".to_string())),
            created_at: Set(chrono::Utc::now().into()),
            updated_at: Set(chrono::Utc::now().into()),
        };
        session.insert(db).await?;

        let mut map = self.active_sessions.write().await;
        map.insert(key, new_id);

        Ok(new_id)
    }

    async fn create_session(&self, channel_id: &str, user_id: &str) -> anyhow::Result<Uuid> {
        let db = self.db()?;
        let new_id = Uuid::new_v4();
        let session = chat_session::ActiveModel {
            id: Set(new_id),
            channel_id: Set(channel_id.to_string()),
            creator_id: Set(Some(user_id.to_string())),
            name: Set(Some("New Chat".to_string())),
            created_at: Set(chrono::Utc::now().into()),
            updated_at: Set(chrono::Utc::now().into()),
        };
        session.insert(db).await?;
        Ok(new_id)
    }

    async fn list_sessions(
        &self,
        channel_id: &str,
        _user_id: &str,
    ) -> anyhow::Result<Vec<(Uuid, Option<String>, chrono::DateTime<chrono::Utc>)>> {
        let db = self.db()?;
        let sessions = chat_session::Entity::find()
            .filter(chat_session::Column::ChannelId.eq(channel_id))
            .order_by_desc(chat_session::Column::UpdatedAt)
            .all(db)
            .await?;

        Ok(sessions
            .into_iter()
            .map(|s| (s.id, s.name, s.updated_at.into()))
            .collect())
    }

    async fn set_active_session(
        &self,
        channel_id: &str,
        _user_id: &str,
        session_id: Uuid,
    ) -> anyhow::Result<()> {
        let db = self.db()?;
        // Validate session exists on the channel
        let exists = chat_session::Entity::find_by_id(session_id)
            .filter(chat_session::Column::ChannelId.eq(channel_id))
            .one(db)
            .await?
            .is_some();

        if !exists {
            anyhow::bail!("Session {} not found on this channel", session_id);
        }

        let key = channel_id.to_string();
        let mut map = self.active_sessions.write().await;
        map.insert(key, session_id);

        Ok(())
    }

    async fn get_history(&self, session_id: Uuid, limit: u64) -> anyhow::Result<Vec<ChatMessage>> {
        let db = self.db()?;
        let messages = chat_message::Entity::find()
            .filter(chat_message::Column::SessionId.eq(session_id))
            .order_by_asc(chat_message::Column::CreatedAt)
            .all(db)
            .await?;

        // Take last N
        let start = if messages.len() > limit as usize {
            messages.len() - limit as usize
        } else {
            0
        };

        Ok(messages[start..]
            .iter()
            .map(|m| ChatMessage {
                role: m.role.clone(),
                content: m.content.clone(),
                name: m.user_id.clone(),
                ..ChatMessage::default()
            })
            .collect())
    }

    async fn append_message(&self, session_id: Uuid, message: ChatMessage) -> anyhow::Result<()> {
        let db = self.db()?;

        // Insert message
        let msg = chat_message::ActiveModel {
            id: Set(Uuid::new_v4()),
            session_id: Set(session_id),
            user_id: Set(message.name),
            role: Set(message.role),
            content: Set(message.content),
            created_at: Set(chrono::Utc::now().into()),
        };
        msg.insert(db).await?;

        // We must load it first to update just updated_at safely, or we can use update_many
        chat_session::Entity::update_many()
            .col_expr(
                chat_session::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(sea_orm::Value::ChronoDateTimeUtc(Some(Box::new(
                    chrono::Utc::now(),
                )))),
            )
            .filter(chat_session::Column::Id.eq(session_id))
            .exec(db)
            .await?;

        Ok(())
    }
}
