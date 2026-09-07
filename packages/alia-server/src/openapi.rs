use utoipa::OpenApi;

use crate::api;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Alia Server API",
        version = "0.1.0",
        description = "Project Alia central backend API"
    ),
    paths(
        // Health
        api::health::health_check,
        api::voice::detect_turn,
        // Auth
        api::auth::login,
        api::auth::register,
        api::auth::refresh,
        // Users
        api::users::get_me,
        api::users::list_users,
        api::users::get_user,
        api::users::update_user,
        api::users::delete_user,
        // Sensors
        api::sensors::list_sensors,
        api::sensors::get_sensor,
        api::sensors::register_sensor,
        api::sensors::unregister_sensor,
        api::sensors::ingest_reading,
        api::sensors::ingest_batch,
        api::sensors::get_latest,
        api::sensors::query_readings,
        // Skills
        api::skills::list_skills,
        api::skills::get_skill,
    ),
    components(schemas(
        // Health
        api::health::HealthResponse,
        api::health::ModuleStatus,
        // Auth
        api::auth::LoginRequest,
        api::auth::RegisterRequest,
        api::auth::RefreshRequest,
        api::auth::AuthResponse,
        api::auth::UserInfo,
        api::auth::TokenResponse,
        api::users::UserResponse,
        api::users::UpdateUserRequest,
        // Sensors
        crate::providers::sensor::SensorDefinition,
        crate::providers::sensor::SensorReading,
        crate::providers::sensor::SensorSummary,
        crate::providers::sensor::SensorQuery,
        api::sensors::RegisterSensorRequest,
        api::sensors::IngestRequest,
        // Skills
        api::skills::SkillResponse,
    )),
    tags(
        (name = "health", description = "Health check"),
        (name = "auth", description = "Authentication"),
        (name = "users", description = "User management"),
        (name = "device", description = "Device management"),
        (name = "voice", description = "Voice interaction"),
        (name = "conversations", description = "Conversation history"),
        (name = "plugins", description = "Plugin management"),
        (name = "rules", description = "Automation rules"),
        (name = "settings", description = "User settings"),
        (name = "sensors", description = "Sensor data"),
        (name = "memories", description = "Memory management"),
        (name = "personas", description = "Persona management"),
        (name = "insights", description = "Background insights system"),
        (name = "skills", description = "Skill definitions — extend what the LLM knows how to do well"),
        (name = "scheduler", description = "Task scheduler"),
        (name = "tools", description = "Tool invocations — extend what the LLM can do"),
    ),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            utoipa::openapi::security::SecurityScheme::Http(
                utoipa::openapi::security::HttpBuilder::new()
                    .scheme(utoipa::openapi::security::HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        );
    }
}
