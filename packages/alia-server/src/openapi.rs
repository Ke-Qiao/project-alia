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
        api::deliveries::get_response,
        api::deliveries::list_deliveries,
        api::deliveries::response_events,
        api::deliveries::get_delivery,
        api::deliveries::text_chunks,
        api::body_deliveries::get_delivery,
        api::body_deliveries::for_response,
        api::deliveries::cancel_response,
        // Companion owner controls
        api::companion::policy,
        api::companion::update_policy,
        api::companion::shadow,
        api::companion::replay,
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
        api::deliveries::DeliveryErrorResponse,
        api::deliveries::DeliveryErrorDetail,
        api::deliveries::DeliveryErrorCode,
        crate::modules::body_delivery::BodyDeliveryRecord,
        crate::modules::delivery::types::ResponseRecord,
        crate::modules::delivery::types::ResponseKey,
        crate::modules::delivery::types::ResponseStatus,
        crate::modules::delivery::types::DeliveryRecord,
        crate::modules::delivery::types::DeliveryEvent,
        crate::modules::delivery::types::DeliveryStatus,
        // Companion owner controls (nested response schemas are collected by ToSchema)
        api::companion::PolicyRequest,
        api::companion::ShadowRequest,
        api::companion::CompanionErrorResponse,
        api::companion::CompanionErrorDetail,
        crate::modules::companion::types::PolicySnapshot,
        crate::modules::companion::repository::ShadowReport,
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
        (name = "companion", description = "Owner policy CAS, bounded shadow decisions and private replay; receipts never grant live output"),
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
        (name = "delivery", description = "Canonical responses and separate accepted, submitted and presented evidence; never an audio replay queue"),
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
    #[tokio::test]
    async fn companion_contract_matches_owner_routes_required_fields_and_real_errors() {
        use axum::body::to_bytes;
        use axum::response::IntoResponse;
        use serde_json::{Value, json};
        use uuid::Uuid;

        use crate::error::AppError;
        use crate::modules::companion::repository::ShadowReport;
        use crate::modules::companion::types::{CompanionPolicy, PolicySnapshot};

        fn required_fields(schema: &Value, fields: &[&str]) {
            let required: std::collections::BTreeSet<_> = schema["required"]
                .as_array()
                .expect("required fields")
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert_eq!(required, fields.iter().copied().collect());
            let properties: std::collections::BTreeSet<_> = schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(properties, required);
        }

        let spec = serde_json::to_value(ApiDoc::openapi()).unwrap();
        let schemas = &spec["components"]["schemas"];
        for (path, methods) in [
            ("/api/v1/companion/policy", vec!["get", "put"]),
            ("/api/v1/companion/shadow", vec!["post"]),
            ("/api/v1/companion/replay/{operation_id}", vec!["get"]),
        ] {
            let item = &spec["paths"][path];
            let published: Vec<_> = item
                .as_object()
                .expect("Companion path")
                .keys()
                .filter(|key| ["get", "put", "post", "delete", "patch"].contains(&key.as_str()))
                .map(String::as_str)
                .collect();
            assert_eq!(published, methods);
            for method in methods {
                let op = &item[method];
                assert_eq!(op["security"], json!([{"bearer": []}]));
                assert_eq!(op["tags"], json!(["companion"]));
                let expected_success = if path.ends_with("policy") {
                    "PolicySnapshot"
                } else {
                    "ShadowReport"
                };
                assert_eq!(
                    op["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
                    format!("#/components/schemas/{expected_success}")
                );
                for status in ["401", "403", "500", "503"] {
                    assert_eq!(
                        op["responses"][status]["content"]["application/json"]["schema"]["$ref"],
                        "#/components/schemas/CompanionErrorResponse"
                    );
                }
                if method != "get" || path.contains("replay") {
                    for status in ["400", "409"] {
                        assert_eq!(
                            op["responses"][status]["content"]["application/json"]["schema"]["$ref"],
                            "#/components/schemas/CompanionErrorResponse"
                        );
                    }
                }
                let unavailable_code = match method {
                    "put" => "COMPANION_POLICY_UNCONFIRMED",
                    "post" => "COMPANION_SHADOW_UNCONFIRMED",
                    _ => "SERVICE_UNAVAILABLE",
                };
                assert!(
                    op["responses"]["503"]["description"]
                        .as_str()
                        .unwrap()
                        .starts_with(unavailable_code)
                );
                if !path.ends_with("policy") {
                    assert_eq!(
                        op["responses"]["410"]["content"]["application/json"]["schema"]["$ref"],
                        "#/components/schemas/CompanionErrorResponse"
                    );
                }
            }
        }
        assert_eq!(
            spec["components"]["securitySchemes"]["bearer"]["scheme"],
            "bearer"
        );
        for (path, method, name, fields) in [
            (
                "/api/v1/companion/policy",
                "put",
                "PolicyRequest",
                vec!["expected_revision", "policy"],
            ),
            (
                "/api/v1/companion/shadow",
                "post",
                "ShadowRequest",
                vec!["operation_id", "appraisal_ids"],
            ),
        ] {
            let body = &spec["paths"][path][method]["requestBody"];
            assert_eq!(body["required"], true);
            assert_eq!(
                body["content"]["application/json"]["schema"]["$ref"],
                format!("#/components/schemas/{name}")
            );
            required_fields(&schemas[name], &fields);
            assert_eq!(schemas[name]["additionalProperties"], false);
        }
        assert_eq!(
            schemas["PolicyRequest"]["properties"]["expected_revision"]["minimum"].as_f64(),
            Some(1.0)
        );
        let policy = serde_json::to_value(CompanionPolicy::default()).unwrap();
        required_fields(
            &schemas["CompanionPolicy"],
            &policy
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        assert_eq!(schemas["CompanionPolicy"]["additionalProperties"], false);
        for (field, min, max) in [
            ("daily_budget", 0, 24),
            ("cooldown_seconds", 30, 86400),
            ("candidate_ttl_seconds", 1, 300),
        ] {
            assert_eq!(
                schemas["CompanionPolicy"]["properties"][field]["minimum"].as_f64(),
                Some(f64::from(min))
            );
            assert_eq!(
                schemas["CompanionPolicy"]["properties"][field]["maximum"].as_f64(),
                Some(f64::from(max))
            );
        }
        assert_eq!(
            schemas["ShadowRequest"]["properties"]["appraisal_ids"]["minItems"],
            1
        );
        assert_eq!(
            schemas["ShadowRequest"]["properties"]["appraisal_ids"]["maxItems"],
            32
        );
        assert_eq!(
            schemas["CompanionMode"]["enum"],
            json!(["disabled", "shadow", "enabled"])
        );
        let operation = Uuid::new_v4();
        for (name, value) in [
            (
                "PolicySnapshot",
                serde_json::to_value(PolicySnapshot {
                    revision: 1,
                    policy: CompanionPolicy::default(),
                })
                .unwrap(),
            ),
            (
                "ShadowReport",
                serde_json::to_value(ShadowReport {
                    operation_id: operation,
                    as_of: chrono::Utc::now(),
                    policy_revision: 1,
                    examined: 0,
                    limited: 0,
                    decisions: vec![],
                })
                .unwrap(),
            ),
        ] {
            required_fields(
                &schemas[name],
                &value
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(
            schemas["ShadowReport"]["properties"]["decisions"]["items"]["$ref"],
            "#/components/schemas/ShadowDecision"
        );
        required_fields(&schemas["ShadowDecision"], &["input", "decision"]);
        assert_eq!(
            schemas["ShadowDecision"]["properties"]["input"]["$ref"],
            "#/components/schemas/DecisionInput"
        );
        assert_eq!(
            schemas["ShadowDecision"]["properties"]["decision"]["$ref"],
            "#/components/schemas/DecisionRecord"
        );
        required_fields(&schemas["CompanionErrorResponse"], &["error"]);
        assert_eq!(
            schemas["CompanionErrorResponse"]["properties"]["error"]["$ref"],
            "#/components/schemas/CompanionErrorDetail"
        );
        let variants = schemas["CompanionErrorDetail"]["oneOf"].as_array().unwrap();
        assert_eq!(variants.len(), 9);
        // Compare schemas to the actual response constructors used by handlers,
        // including the shared AppError path; no database or mirror router needed.
        for (status, code, response, identity) in [
            (
                400,
                "VALIDATION_ERROR",
                AppError::Validation("invalid input".into()).into_response(),
                None,
            ),
            (
                401,
                "AUTH_INVALID_CREDENTIALS",
                AppError::Auth("Missing authorization header".into()).into_response(),
                None,
            ),
            (
                403,
                "AUTH_FORBIDDEN",
                AppError::Forbidden.into_response(),
                None,
            ),
            (
                409,
                "VERSION_CONFLICT",
                AppError::Conflict("companion identity or policy revision changed".into())
                    .into_response(),
                None,
            ),
            (
                500,
                "INTERNAL_DB_ERROR",
                AppError::Database(sea_orm::DbErr::Custom("private database error".into()))
                    .into_response(),
                None,
            ),
            (
                503,
                "SERVICE_UNAVAILABLE",
                AppError::Unavailable("companion storage unavailable").into_response(),
                None,
            ),
            (
                503,
                "COMPANION_POLICY_UNCONFIRMED",
                api::companion::unconfirmed_policy(7),
                Some(("expected_revision", json!(7))),
            ),
            (
                503,
                "COMPANION_SHADOW_UNCONFIRMED",
                api::companion::unconfirmed_shadow(operation),
                Some(("operation_id", json!(operation))),
            ),
            (
                410,
                "COMPANION_EVIDENCE_ERASED",
                api::companion::erased(operation),
                Some(("operation_id", json!(operation))),
            ),
        ] {
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(response.headers()["content-type"], "application/json");
            let value: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            assert_eq!(value.as_object().unwrap().len(), 1);
            let error = &value["error"];
            assert_eq!(error["code"], code);
            assert!(error["message"].is_string());
            match code {
                "COMPANION_POLICY_UNCONFIRMED" => assert_eq!(
                    error["message"],
                    "policy completion is unconfirmed; read current policy before retrying the original revision"
                ),
                "COMPANION_SHADOW_UNCONFIRMED" => assert_eq!(
                    error["message"],
                    "completion is unconfirmed; retry the same operation_id and appraisal_ids"
                ),
                "COMPANION_EVIDENCE_ERASED" => assert_eq!(
                    error["message"],
                    "private decision evidence was erased; it cannot be replayed"
                ),
                "INTERNAL_DB_ERROR" => assert_eq!(error["message"], "internal error"),
                _ => (),
            }
            let variant = variants
                .iter()
                .find(|variant| variant["properties"]["code"]["enum"] == json!([code]))
                .expect("documented error code");
            let mut fields = vec!["code", "message"];
            if let Some((field, expected)) = identity {
                fields.push(field);
                assert_eq!(error[field], expected);
            }
            required_fields(variant, &fields);
            assert_eq!(error.as_object().unwrap().len(), fields.len());
        }
        let replay = &spec["paths"]["/api/v1/companion/replay/{operation_id}"]["get"];
        assert_eq!(replay["parameters"][0]["name"], "operation_id");
        assert_eq!(replay["parameters"][0]["required"], true);
        assert_eq!(replay["parameters"][0]["schema"]["format"], "uuid");
        assert_eq!(
            replay["responses"]["400"]["content"]["text/plain"]["schema"]["type"],
            "string"
        );
    }
}
