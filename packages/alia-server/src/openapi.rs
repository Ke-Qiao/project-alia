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
        // Auth
        api::auth::login,
        api::auth::register,
        api::auth::refresh,
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
        (name = "processes", description = "Process management"),
        (name = "plugins", description = "Plugin management"),
        (name = "rules", description = "Automation rules"),
        (name = "settings", description = "User settings"),
        (name = "sensors", description = "Sensor data"),
        (name = "memories", description = "Memory management"),
        (name = "personas", description = "Persona management"),
        (name = "meditation", description = "Meditation system"),
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
