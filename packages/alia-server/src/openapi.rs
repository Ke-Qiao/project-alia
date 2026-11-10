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
        api::embodiment::list,
        api::embodiment::get_operation,
        api::effects::get_effect,
        api::effects::confirm,
        api::effects::cancel,
        api::deliveries::get_response,
        api::deliveries::list_deliveries,
        api::deliveries::response_events,
        api::deliveries::get_delivery,
        api::deliveries::text_chunks,
        api::motion_deliveries::get_delivery,
        api::motion_deliveries::for_response,
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
        api::auth::logout,
        // Users
        api::users::get_me,
        api::users::update_user_me,
        api::users::list_users,
        api::users::get_user,
        api::users::update_user,
        api::users::delete_user,
        // Conversations
        api::conversations::list_conversations,
        api::conversations::get_conversation,
        api::conversations::delete_conversation,
        api::conversations::get_messages,
        api::conversations::get_events,
        api::conversations::get_expression_traces,
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
        crate::error::ErrorResponse,
        crate::error::ErrorDetail,
        crate::error::ErrorCode,
        crate::error::ErrorReason,
        api::auth::LogoutResult,
        api::conversations::Conversation,
        api::conversations::Message,
        api::conversations::AgentEvent,
        api::conversations::DeleteConversationResult,
        crate::expression::ExpressionTrace,
        crate::expression::ExpressionTraceEventSummary,
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
        crate::modules::embodiment::motion_delivery::MotionDeliveryRecord,
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
        crate::modules::embodiment::sensors::SensorDefinition,
        crate::modules::embodiment::sensors::SensorReading,
        crate::modules::embodiment::sensors::SensorSummary,
        crate::modules::embodiment::sensors::SensorQuery,
        api::sensors::RegisterSensorRequest,
        api::sensors::IngestRequest,
        // Skills
        api::skills::SkillResponse,
    )),
    tags(
        (name = "embodiment", description = "Owner-scoped physical operations; control uses Execution"),
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
    modifiers(&SecurityAddon, &StandardAuthErrors)
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

struct StandardAuthErrors;

impl utoipa::Modify for StandardAuthErrors {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::{Content, Ref, RefOr, Response, Schema, schema::OneOfBuilder};

        let shared = RefOr::Ref(Ref::from_schema_name("ErrorResponse"));
        for item in openapi.paths.paths.values_mut() {
            for operation in [
                &mut item.get,
                &mut item.put,
                &mut item.post,
                &mut item.delete,
                &mut item.options,
                &mut item.head,
                &mut item.patch,
                &mut item.trace,
            ]
            .into_iter()
            .flatten()
            {
                let security = operation.security.as_ref().or(openapi.security.as_ref());
                let secured = security.is_some_and(|requirements| {
                    !requirements.is_empty()
                        && requirements
                            .iter()
                            .all(|requirement| requirement != &Default::default())
                });
                if !secured {
                    continue;
                }
                for (status, description) in [
                    ("401", "Missing or invalid token, or inactive account"),
                    ("503", "Account verification temporarily unavailable"),
                ] {
                    let response = operation
                        .responses
                        .responses
                        .entry(status.into())
                        .or_insert_with(|| Response::new(description).into());
                    let RefOr::T(response) = response else {
                        continue;
                    };
                    let content = response
                        .content
                        .entry("application/json".into())
                        .or_insert_with(|| Content::new(Some(shared.clone())));
                    if status == "401" {
                        content.schema = Some(shared.clone());
                        continue;
                    }
                    content.schema = Some(match content.schema.take() {
                        None => shared.clone(),
                        Some(schema) if schema == shared => schema,
                        Some(RefOr::T(Schema::OneOf(mut union))) => {
                            if !union.items.contains(&shared) {
                                union.items.push(shared.clone());
                            }
                            RefOr::T(Schema::OneOf(union))
                        }
                        Some(schema) => {
                            OneOfBuilder::new().item(schema).item(shared.clone()).into()
                        }
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids_are_unique_and_resource_names_are_stable() {
        let spec = serde_json::to_value(ApiDoc::openapi()).unwrap();
        let mut ids = std::collections::BTreeSet::new();
        for (path, item) in spec["paths"].as_object().unwrap() {
            for method in [
                "get", "put", "post", "delete", "options", "head", "patch", "trace",
            ] {
                let Some(operation) = item.get(method) else {
                    continue;
                };
                let id = operation["operationId"].as_str().expect("operationId");
                assert!(
                    ids.insert(id),
                    "duplicate operationId {id}: {method} {path}"
                );
                assert!(
                    !id.ends_with(|c: char| c.is_ascii_digit()),
                    "numeric suffix: {id}"
                );
            }
        }
    }

    #[test]
    fn secured_operations_document_json_auth_errors_and_public_logout_stays_public() {
        let spec = serde_json::to_value(ApiDoc::openapi()).unwrap();
        for (path, item) in spec["paths"].as_object().unwrap() {
            for method in [
                "get", "put", "post", "delete", "options", "head", "patch", "trace",
            ] {
                let Some(operation) = item.get(method) else {
                    continue;
                };
                if operation["security"].as_array().is_none_or(Vec::is_empty) {
                    continue;
                }
                for status in ["401", "503"] {
                    assert!(
                        operation["responses"][status]["content"]["application/json"]["schema"]
                            .is_object(),
                        "missing JSON error schema for {method} {path}: {status}"
                    );
                }
                assert_eq!(
                    operation["responses"]["401"]["content"]["application/json"]["schema"]["$ref"],
                    "#/components/schemas/ErrorResponse"
                );
            }
        }
        assert!(
            spec["paths"]["/api/v1/auth/logout"]["post"]
                .get("security")
                .is_none()
        );
        for (path, method, id) in [
            ("/api/v1/auth/logout", "post", "logout"),
            ("/api/v1/users/me", "patch", "update_me"),
            ("/api/v1/conversations", "get", "list_conversations"),
            ("/api/v1/conversations/{id}", "get", "get_conversation"),
            (
                "/api/v1/conversations/{id}",
                "delete",
                "delete_conversation",
            ),
            (
                "/api/v1/conversations/{id}/messages",
                "get",
                "list_conversation_messages",
            ),
            (
                "/api/v1/conversations/{id}/events",
                "get",
                "list_conversation_events",
            ),
            (
                "/api/v1/conversations/{id}/expression-traces",
                "get",
                "list_conversation_expression_traces",
            ),
        ] {
            assert_eq!(spec["paths"][path][method]["operationId"], id);
        }
        for (path, method, has_validation_error) in [
            ("/api/v1/auth/login", "post", true),
            ("/api/v1/auth/register", "post", true),
            ("/api/v1/auth/refresh", "post", false),
            ("/api/v1/auth/logout", "post", false),
            ("/api/v1/users/me", "patch", true),
        ] {
            let responses = &spec["paths"][path][method]["responses"];
            for status in ["400", "413", "415", "422"] {
                assert_eq!(
                    responses[status]["content"]["text/plain"]["schema"]["type"], "string",
                    "missing extractor rejection: {method} {path} {status}"
                );
            }
            if has_validation_error {
                assert_eq!(
                    responses["400"]["content"]["application/json"]["schema"]["$ref"],
                    "#/components/schemas/ErrorResponse"
                );
            }
        }
        for suffix in ["", "/messages", "/events", "/expression-traces"] {
            let path = format!("/api/v1/conversations/{{id}}{suffix}");
            let operation = &spec["paths"][path]["get"];
            assert_eq!(
                operation["responses"]["400"]["content"]["text/plain"]["schema"]["type"],
                "string"
            );
            for status in ["403", "404"] {
                assert_eq!(
                    operation["responses"][status]["content"]["application/json"]["schema"]["$ref"],
                    "#/components/schemas/ErrorResponse"
                );
            }
        }
    }

    #[test]
    fn auth_error_injection_preserves_domain_media_is_idempotent_and_handles_every_method() {
        use utoipa::Modify;
        let mut spec: utoipa::openapi::OpenApi = serde_json::from_value(serde_json::json!({
            "openapi": "3.1.0", "info": {"title": "test", "version": "1"},
            "security": [{"bearer": []}],
            "paths": {
                "/secured": {
                    "get": {"responses": {"503": {"description": "domain", "content": {
                        "text/plain": {"schema": {"type": "string"}},
                        "application/json": {"schema": {"$ref": "#/components/schemas/ExecutionErrorResponse"}}
                    }}}},
                    "put": {"responses": {}}, "post": {"responses": {}},
                    "delete": {"responses": {}}, "options": {"responses": {}},
                    "head": {"responses": {}}, "patch": {"responses": {}}, "trace": {"responses": {}}
                },
                "/public": {"get": {"security": [], "responses": {}}},
                "/optional": {"get": {"security": [{"bearer": []}, {}], "responses": {}}}
            }
        })).unwrap();
        StandardAuthErrors.modify(&mut spec);
        let once = serde_json::to_value(&spec).unwrap();
        StandardAuthErrors.modify(&mut spec);
        assert_eq!(once, serde_json::to_value(&spec).unwrap());
        for method in [
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ] {
            for status in ["401", "503"] {
                assert!(once["paths"]["/secured"][method]["responses"][status]["content"]["application/json"]["schema"].is_object());
            }
        }
        assert_eq!(
            once["paths"]["/secured"]["get"]["responses"]["503"]["content"]["text/plain"]["schema"]
                ["type"],
            "string"
        );
        assert_eq!(once["paths"]["/secured"]["get"]["responses"]["503"]["content"]["application/json"]["schema"]["oneOf"].as_array().unwrap().len(), 2);
        for path in ["/public", "/optional"] {
            assert_eq!(
                once["paths"][path]["get"]["responses"],
                serde_json::json!({})
            );
        }
    }

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
                operation["responses"]["503"]["content"]["application/json"]["schema"]["oneOf"],
                serde_json::json!([
                    {"$ref": "#/components/schemas/ExecutionErrorResponse"},
                    {"$ref": "#/components/schemas/ErrorResponse"},
                ])
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
                for status in ["401", "403", "500"] {
                    assert_eq!(
                        op["responses"][status]["content"]["application/json"]["schema"]["$ref"],
                        "#/components/schemas/ErrorResponse"
                    );
                }
                let unavailable = &op["responses"]["503"]["content"]["application/json"]["schema"];
                if method == "get" {
                    assert_eq!(unavailable["$ref"], "#/components/schemas/ErrorResponse");
                } else {
                    assert_eq!(
                        unavailable["oneOf"],
                        json!([
                            {"$ref": "#/components/schemas/CompanionErrorResponse"},
                            {"$ref": "#/components/schemas/ErrorResponse"},
                        ])
                    );
                }
                if method != "get" || path.contains("replay") {
                    for status in ["400", "409"] {
                        assert_eq!(
                            op["responses"][status]["content"]["application/json"]["schema"]["$ref"],
                            "#/components/schemas/ErrorResponse"
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
        assert_eq!(
            variants.len(),
            3,
            "only Companion identity-bearing receipts belong here"
        );
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
                AppError::Unauthenticated(crate::error::AuthFailure::TokenMissing).into_response(),
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
            let mut fields = vec!["code", "message"];
            if let Some((field, expected)) = identity {
                fields.push(field);
                assert_eq!(error[field], expected);
                let variant = variants
                    .iter()
                    .find(|variant| variant["properties"]["code"]["enum"] == json!([code]))
                    .expect("documented companion receipt code");
                required_fields(variant, &fields);
            } else {
                assert!(
                    schemas["ErrorCode"]["enum"]
                        .as_array()
                        .unwrap()
                        .contains(&json!(code))
                );
                assert_eq!(
                    schemas["ErrorDetail"]["required"],
                    json!(["code", "message"])
                );
                if status == 401 {
                    fields.push("reason");
                    assert_eq!(error["reason"], "token_missing");
                } else {
                    assert!(error.get("reason").is_none());
                }
            }
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
