//! Shared persistence for immutable descriptors returned by an embedding
//! request. Memory and Cache retain ownership of their own projection rows.
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, Statement};
use serde_json::Value;

use crate::providers::adapter::EmbeddingProfile;

pub(crate) async fn ensure_profile(
    tx: &DatabaseTransaction,
    profile: &EmbeddingProfile,
) -> anyhow::Result<String> {
    let id = profile.id()?;
    let descriptor = serde_json::to_value(profile)?;
    tx.execute(stmt(
        "INSERT INTO embedding_profiles(id,provider,model,dimensions,distance,vector_type,request_descriptor) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(id) DO NOTHING",
        vec![id.clone().into(),profile.provider.clone().into(),profile.model.clone().into(),
            (profile.dimensions as i32).into(),profile.distance.clone().into(),
            profile.vector_type.clone().into(),descriptor.clone().into()],
    )).await?;
    let row = tx.query_one(stmt(
        "SELECT provider,model,dimensions,distance,vector_type,request_descriptor FROM embedding_profiles WHERE id=$1",
        vec![id.clone().into()],
    )).await?.ok_or_else(|| anyhow::anyhow!("embedding profile missing"))?;
    anyhow::ensure!(
        row.try_get::<Value>("", "request_descriptor")? == descriptor
            && row.try_get::<String>("", "provider")? == profile.provider
            && row.try_get::<String>("", "model")? == profile.model
            && row.try_get::<i32>("", "dimensions")? == profile.dimensions as i32
            && row.try_get::<String>("", "distance")? == profile.distance
            && row.try_get::<String>("", "vector_type")? == profile.vector_type,
        "embedding profile identity conflict"
    );
    Ok(id)
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}
