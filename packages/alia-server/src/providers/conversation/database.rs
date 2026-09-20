use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::entity::{chat_message, chat_session, conversation_event};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{ChatMessage, ConversationBackend, ConversationEventEntry};

#[cfg(test)]
mod tests;

pub struct DatabaseConversationBackend {
    db: Option<DatabaseConnection>,
    // In-memory tracker for the active session ID per channel_id + user_id.
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

    fn session_key(channel_id: &str, user_id: &str) -> String {
        format!("{channel_id}\u{1f}{user_id}")
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
        let key = Self::session_key(channel_id, user_id);

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
            .filter(chat_session::Column::CreatorId.eq(Some(user_id.to_string())))
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
        user_id: &str,
    ) -> anyhow::Result<Vec<(Uuid, Option<String>, chrono::DateTime<chrono::Utc>)>> {
        let db = self.db()?;
        let sessions = chat_session::Entity::find()
            .filter(chat_session::Column::ChannelId.eq(channel_id))
            .filter(chat_session::Column::CreatorId.eq(Some(user_id.to_string())))
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
        user_id: &str,
        session_id: Uuid,
    ) -> anyhow::Result<()> {
        let db = self.db()?;
        // Validate session exists on the channel
        let exists = chat_session::Entity::find_by_id(session_id)
            .filter(chat_session::Column::ChannelId.eq(channel_id))
            .filter(chat_session::Column::CreatorId.eq(Some(user_id.to_string())))
            .one(db)
            .await?
            .is_some();

        if !exists {
            anyhow::bail!("Session {} not found on this channel", session_id);
        }

        let key = Self::session_key(channel_id, user_id);
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
        use sea_orm::TransactionTrait;
        let tx = self.db()?.begin().await?;

        // Insert message
        let msg = chat_message::ActiveModel {
            id: Set(Uuid::new_v4()),
            session_id: Set(session_id),
            user_id: Set(message.name),
            role: Set(message.role),
            content: Set(message.content),
            created_at: Set(chrono::Utc::now().into()),
        };
        msg.insert(&tx).await?;

        // We must load it first to update just updated_at safely, or we can use update_many
        chat_session::Entity::update_many()
            .col_expr(
                chat_session::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(sea_orm::Value::ChronoDateTimeUtc(Some(Box::new(
                    chrono::Utc::now(),
                )))),
            )
            .filter(chat_session::Column::Id.eq(session_id))
            .exec(&tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn commit_response(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        response_id: Uuid,
        message: ChatMessage,
    ) -> anyhow::Result<Uuid> {
        use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
        anyhow::ensure!(
            message.role == "assistant" && message.name.is_none(),
            "canonical response history must be an assistant message"
        );
        let db = self.db()?;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let tx = db.begin().await?;
            let repository = crate::modules::delivery::repository::DeliveryRepository::new(db.clone());
            let proposed_message_id = Uuid::new_v4();
            let response = repository.mark_generated_in_transaction(
                &tx,
                user_id,
                response_id,
                proposed_message_id,
                serde_json::json!({ "text": &message.content }),
            ).await?;
            anyhow::ensure!(response.conversation_id == Some(session_id), "response belongs to a different conversation");
            let id = response.assistant_message_id
                .ok_or_else(|| anyhow::anyhow!("committed response lacks its assistant message identity"))?;
            // A retry observes a historical commit. It must never recreate a
            // message that was erased after that commit, even in another process.
            if id != proposed_message_id {
                tx.commit().await?;
                return Ok(id);
            }
            tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "INSERT INTO chat_messages(id,session_id,user_id,role,content,created_at,source_received_at,source_privacy_epoch) VALUES($1,$2,NULL,'assistant',$3,clock_timestamp(),$4,$5) ON CONFLICT(id) DO NOTHING",
                [id.into(), session_id.into(), message.content.clone().into(), response.source_received_at.into(), response.source_privacy_epoch.into()],
            )).await?;
            let existing = tx.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "SELECT session_id,user_id,role,content,source_privacy_epoch FROM chat_messages WHERE id=$1", [id.into()],
            )).await?.ok_or_else(|| anyhow::anyhow!("response message was not persisted"))?;
            anyhow::ensure!(
                existing.try_get::<Uuid>("", "session_id")? == session_id
                    && existing.try_get::<Option<String>>("", "user_id")?.is_none()
                    && existing.try_get::<String>("", "role")? == "assistant"
                    && existing.try_get::<String>("", "content")? == message.content
                    && existing.try_get::<Option<i64>>("", "source_privacy_epoch")? == response.source_privacy_epoch,
                "canonical response message conflicts with an existing record"
            );
            let updated = tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "UPDATE chat_sessions SET updated_at=clock_timestamp() WHERE id=$1 AND creator_id=$2",
                [session_id.into(), user_id.to_string().into()],
            )).await?;
            anyhow::ensure!(updated.rows_affected() == 1, "response conversation owner changed");
            tx.commit().await?;
            Ok(id)
        }).await.map_err(|_| anyhow::anyhow!("canonical response commit acknowledgement timed out; query the existing response before retrying"))?
    }

    async fn append_sourced_message(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        message: ChatMessage,
        received_at: chrono::DateTime<chrono::Utc>,
        source_privacy_epoch: i64,
        source_relationship_epoch: i64,
    ) -> anyhow::Result<Uuid> {
        use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
        anyhow::ensure!(
            message.role == "user",
            "only user input can be a memory source"
        );
        anyhow::ensure!(source_privacy_epoch >= 0, "invalid source privacy epoch");
        anyhow::ensure!(
            source_relationship_epoch >= 0,
            "invalid source relationship epoch"
        );
        let tx = self.db()?.begin().await?;
        let id = Uuid::new_v4();
        let inserted = tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,created_at,source_received_at,source_privacy_epoch,source_relationship_epoch) SELECT $1,$2,$3,'user',$4,clock_timestamp(),$5,$6,$7 FROM chat_sessions WHERE id=$2 AND creator_id=$3",
            vec![id.into(),session_id.into(),user_id.to_string().into(),message.content.into(),received_at.into(),source_privacy_epoch.into(),source_relationship_epoch.into()])).await?;
        anyhow::ensure!(
            inserted.rows_affected() == 1,
            "conversation session is not owned by this account"
        );
        tx.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE chat_sessions SET updated_at=clock_timestamp() WHERE id=$1",
            [session_id.into()],
        ))
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    async fn append_event(
        &self,
        session_id: Uuid,
        event: ConversationEventEntry,
    ) -> anyhow::Result<()> {
        let db = self.db()?;
        let event = conversation_event::ActiveModel {
            id: Set(Uuid::new_v4()),
            session_id: Set(session_id),
            turn_id: Set(event.turn_id),
            sequence: Set(event.sequence),
            event_type: Set(event.event_type),
            actor: Set(event.actor),
            summary: Set(event.summary),
            payload: Set(event.payload),
            created_at: Set(chrono::Utc::now().into()),
        };
        event.insert(db).await?;
        Ok(())
    }
}
