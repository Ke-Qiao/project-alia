use std::env;

use axum::Router;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use tower_http::cors::CorsLayer;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
}

async fn connect_db() -> anyhow::Result<DatabaseConnection> {
    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://alia:alia_dev@alia-postgres.orb.local/alia".into());

    let mut opt = ConnectOptions::new(&database_url);
    opt.max_connections(10).min_connections(2);

    let db = Database::connect(opt).await?;
    tracing::info!("connected to database");
    Ok(db)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let db = connect_db().await?;
    let state = AppState { db };

    let app = Router::new()
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = "0.0.0.0:3000";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("alia-server listening on {addr}");
    axum::serve(listener, app).await?;

    Ok(())
}
