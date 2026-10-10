//! Owns the atomic canonical Response/history commit across domain participants.
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::providers::adapter::ChatMessage;

pub struct ResponseCommitter {
    db: DatabaseConnection,
}

impl ResponseCommitter {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// A retry returns the historical message identity without recreating erased history.
    /// Committing a response does not establish device presentation evidence.
    pub async fn commit_response(
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
        let db = &self.db;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let tx = db.begin().await?;
            let proposed_message_id = Uuid::new_v4();
            let response = crate::modules::delivery::repository::DeliveryRepository::mark_generated_in_transaction(
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
}
