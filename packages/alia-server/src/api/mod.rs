pub mod auth;
pub mod conversations;
pub mod devices;
pub mod health;
pub mod meditation;
pub mod memories;
pub mod personas;
pub mod plugins;
pub mod processes;
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
        .merge(users::routes())
        .merge(devices::routes())
        .merge(voice::routes())
        .merge(conversations::routes())
        .merge(processes::routes())
        .merge(plugins::routes())
        .merge(rules::routes())
        .merge(scheduler::routes())
        .merge(settings::routes())
        .merge(sensors::routes())
        .merge(memories::routes())
        .merge(personas::routes())
        .merge(meditation::routes())
        .merge(skills::routes())
        .merge(tools::routes())
}
