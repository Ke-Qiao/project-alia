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
        // Durable execution
        api::tools::execute_tool,
        api::effects::list,
        api::effects::get_effect,
        api::effects::confirm,
        api::effects::cancel,
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
        // Durable execution
        api::tools::ExecuteToolRequest,
        api::effects::ConfirmRequest,
        api::effects::ExecutionErrorResponse,
        api::effects::ExecutionErrorDetail,
        api::effects::ExecutionErrorCode,
        crate::modules::execution::types::EffectRecord,
        crate::modules::execution::types::EffectKey,
        crate::modules::execution::types::EffectStatus,
        crate::modules::execution::types::Completion,
        crate::modules::execution::types::ApprovalKind,
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
        (name = "execution", description = "Durable effect proposals, owner approval and observed outcomes"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_contract_documents_durable_results_confirmation_and_typed_errors() {
        let spec = serde_json::to_value(ApiDoc::openapi()).unwrap();
        for (path, method) in [
            ("/api/v1/tools/execute", "post"),
            ("/api/v1/effects", "get"),
            ("/api/v1/effects/{id}", "get"),
            ("/api/v1/effects/{id}/confirm", "post"),
            ("/api/v1/effects/{id}/cancel", "post"),
        ] {
            let operation = &spec["paths"][path][method];
            assert!(operation.is_object(), "missing {method} {path}");
            assert_eq!(operation["security"][0]["bearer"], serde_json::json!([]));
            assert_eq!(
                operation["responses"]["503"]["content"]["application/json"]["schema"]["$ref"],
                "#/components/schemas/ExecutionErrorResponse"
            );
        }
        let request = &spec["components"]["schemas"]["ExecuteToolRequest"];
        assert!(
            request["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("request_id"))
        );
        assert!(
            !request["properties"]
                .as_object()
                .unwrap()
                .contains_key("confirmed")
        );
        let status = &spec["components"]["schemas"]["EffectStatus"]["enum"];
        assert!(
            status
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("unknown"))
        );
        assert_eq!(
            spec["paths"]["/api/v1/tools/execute"]["post"]["responses"]["200"]["content"]["application/json"]
                ["schema"]["$ref"],
            "#/components/schemas/EffectRecord"
        );
    }
}
