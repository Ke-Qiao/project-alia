pub mod auth;
pub mod channels;
pub mod conversations;
pub mod devices;
pub mod effects;
pub mod expression;
pub mod health;
pub mod insights;
pub mod memories;
pub mod personas;
pub mod plugins;
pub mod rules;
pub mod scheduler;
pub mod sensors;
pub mod settings;
pub mod skills;
pub mod tools;
pub mod users;
pub mod voice;

use axum::Router;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(health::routes())
        .nest("/api/v1", api_routes())
}

fn api_routes() -> Router<AppState> {
    Router::new()
        .merge(auth::routes())
        .merge(channels::routes())
        .merge(users::routes())
        .merge(devices::routes())
        .merge(effects::routes())
        .merge(expression::routes())
        .merge(voice::routes())
        .merge(conversations::routes())
        .merge(plugins::routes())
        .merge(rules::routes())
        .merge(scheduler::routes())
        .merge(settings::routes())
        .merge(sensors::routes())
        .merge(memories::routes())
        .merge(personas::routes())
        .merge(insights::routes())
        .merge(skills::routes())
        .merge(tools::routes())
}
