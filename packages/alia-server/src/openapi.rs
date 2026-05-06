use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Alia Server API",
        version = "0.1.0",
        description = "Project Alia central backend API"
    ),
    paths(),
    components(schemas()),
    tags(
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
        (name = "skills", description = "Skills"),
    )
)]
pub struct ApiDoc;
